//! routers/messaging.py routes that call sidecars, against fakes: `/upload` (MinIO) and
//! `/memos/{id}/pdf` (the PDF service). One test: the fakes' addresses must be in the environment before
//! the app starts.

mod support;

use serde_json::{json, Value};
use support::UserSpec;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const M: &str = "/api/v1/messaging";

#[tokio::test]
async fn upload_goes_to_minio_and_the_memo_pdf_to_the_pdf_service() {
    let minio = MockServer::start().await;
    let pdf = MockServer::start().await;
    std::env::set_var("MINIO_ENDPOINT", minio.address().to_string());
    std::env::set_var("PDF_SERVICE_URL", pdf.uri());
    let app = support::app("messaging_sidecars");

    Mock::given(method("HEAD"))
        .and(path("/granary-assets"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&minio)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(
            r"^/granary-assets/messaging/[0-9a-f-]{36}/report\.pdf$",
        ))
        .respond_with(ResponseTemplate::new(200))
        .mount(&minio)
        .await;
    Mock::given(method("POST"))
        .and(path("/render"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"%PDF-fake".to_vec()))
        .mount(&pdf)
        .await;

    // ── upload ──
    let a = app.make_user(UserSpec::new().name("Pyt Sender")).await;
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(b"%PDF-1.4 hello".to_vec())
            .file_name("dir/report.pdf")
            .mime_str("application/pdf")
            .unwrap(),
    );
    let res = app
        .send(
            app.client()
                .post(app.url(&format!("{M}/upload")))
                .bearer_auth(&a.token)
                .multipart(form),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let body = res.json();
    let url = body["url"].as_str().unwrap().to_string();
    assert!(
        url.starts_with("http://localhost:9000/granary-assets/messaging/")
            && url.contains("/report.pdf?X-Amz-Algorithm=")
            && url.contains("&X-Amz-Expires=900&"),
        "{url}"
    );
    assert_eq!(
        body,
        json!({"url": url, "file_name": "report.pdf", "file_size": 14, "mime_type": "application/pdf",
               "message_type": "file"})
    );
    let puts: Vec<_> = minio
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method.as_str() == "PUT")
        .collect();
    assert_eq!(puts.len(), 1);
    assert_eq!(puts[0].body, b"%PDF-1.4 hello");
    assert_eq!(
        puts[0]
            .headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/pdf"
    );

    // ── memo pdf ──
    let b = app.make_user(UserSpec::new().name("Pyt Reader")).await;
    let mut db = app.db().await;
    sqlx::query("UPDATE users SET position = 'Analyst' WHERE id = $1")
        .bind(b.id)
        .execute(&mut db)
        .await
        .unwrap();
    let memo = app
        .post(
            &a.token,
            &format!("{M}/memos"),
            json!({"subject": "Subj", "body": "Body", "memo_type": "odd_kind", "recipient_id": b.id}),
        )
        .await
        .json()["memo"]
        .clone();
    let memo_id = memo["id"].as_str().unwrap();
    assert_eq!(
        app.post(
            &b.token,
            &format!("{M}/memos/{memo_id}/acknowledge"),
            json!(null)
        )
        .await
        .status,
        200
    );
    let res = app.get(&b.token, &format!("{M}/memos/{memo_id}/pdf")).await;
    assert_eq!(res.status, 200, "{}", res.text());
    assert_eq!(res.body, b"%PDF-fake");
    assert_eq!(res.header("content-type").unwrap(), "application/pdf");
    let number = memo["memo_number"].as_str().unwrap();
    assert_eq!(
        res.header("content-disposition").unwrap(),
        format!("attachment; filename=\"{}.pdf\"", number.replace('/', "-"))
    );
    let renders = pdf.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&renders.last().unwrap().body).unwrap();
    assert_eq!(sent["template"], json!("memo"));
    let data = &sent["data"];
    let keys: Vec<&str> = data
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    assert_eq!(
        keys,
        vec![
            "memo_number",
            "subject",
            "memo_type",
            "memo_type_label",
            "body",
            "sender_name",
            "sender_position",
            "sender_department",
            "sender_signed_date",
            "to_line",
            "formatted_date",
            "status",
            "status_label",
            "recalled_at",
            "recalled_date",
            "acknowledgements",
            "total_recipients",
            "single_ack",
            "letterhead_url",
            "company_name",
            "company_legal",
            "company_trade",
            "company_email",
            "company_phone",
            "company_website",
            "company_address",
        ]
    );
    assert_eq!(data["memo_type_label"], json!("Odd_Kind"));
    assert_eq!(data["status_label"], json!("Acknowledged"));
    assert_eq!(data["to_line"], json!("Pyt Reader, Analyst"));
    assert_eq!(data["total_recipients"], json!(1));
    assert_eq!(data["recalled_at"], Value::Null);
    assert_eq!(data["recalled_date"], json!(""));
    let ack = &data["acknowledgements"][0];
    assert_eq!(ack["name"], json!("Pyt Reader"));
    assert_eq!(ack["position"], json!("Analyst"));
    assert_eq!(&data["single_ack"], ack);
    // "{day} {Month} {year}, {HH:MM TZ}" in the company time zone.
    let signed = data["sender_signed_date"].as_str().unwrap();
    let re_ok = signed.split(", ").count() == 2 && signed.split(' ').count() == 5;
    assert!(re_ok, "{signed}");
}
