//! services/pdf.rs against a fake PDF sidecar (wiremock). One test function: the settings are process-wide.

mod support;

use serde_json::json;
use wiremock::matchers::{body_bytes, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use granary_services::pdf;
use granary_services::whatsapp::HttpxError;

#[tokio::test]
async fn pdf_sidecar_contract() {
    let server = MockServer::builder().start().await;
    std::env::set_var("PDF_SERVICE_URL", server.uri());
    std::env::set_var("PDF_SERVICE_TOKEN", "pdf-token");
    support::init();
    if std::env::var_os("DATABASE_URL").is_none() {
        std::env::set_var(
            "DATABASE_URL",
            "postgres://granary:granarytest@127.0.0.1:55433/postgres",
        ); // settings need one; unused
    }

    // footer/header only when truthy; body as httpx json= (compact, UTF-8, Python floats).
    Mock::given(method("POST"))
        .and(path("/render"))
        .and(header("authorization", "Bearer pdf-token"))
        .and(header("content-type", "application/json"))
        .and(body_bytes(&b"{\"template\":\"quotation\",\"data\":{\"total\":1e+16,\"name\":\"Caf\xc3\xa9\"}}"[..]))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"%PDF-1.7 a".to_vec()))
        .expect(3)
        .mount(&server)
        .await;
    let data = json!({"total": 1e16, "name": "Café"});
    assert_eq!(
        pdf::generate_quotation_pdf(&data).await.unwrap(),
        b"%PDF-1.7 a"
    );
    assert_eq!(
        pdf::generate_pdf("quotation", &data, Some(&json!({})), None)
            .await
            .unwrap(),
        b"%PDF-1.7 a"
    );
    assert_eq!(
        pdf::generate_pdf("quotation", &data, None, Some(&json!(null)))
            .await
            .unwrap(),
        b"%PDF-1.7 a"
    );
    server.verify().await;
    server.reset().await;

    Mock::given(method("POST"))
        .and(path("/render"))
        .and(body_bytes(
            &br#"{"template":"sop","data":{},"footer":{"qr_data_uri":"","title":"T","line":"L","code":"C"},"header":{"text":"Acme Ltd | Confidential"}}"#[..],
        ))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"%PDF-sop".to_vec()))
        .expect(1)
        .mount(&server)
        .await;
    let footer = json!({"qr_data_uri": "", "title": "T", "line": "L", "code": "C"});
    let header_v = json!({"text": "Acme Ltd | Confidential"});
    assert_eq!(
        pdf::generate_pdf("sop", &json!({}), Some(&footer), Some(&header_v))
            .await
            .unwrap(),
        b"%PDF-sop"
    );
    server.verify().await;
    server.reset().await;

    for (tpl, f) in [("invoice", 0), ("proforma", 1)] {
        Mock::given(method("POST"))
            .and(path("/render"))
            .and(body_bytes(
                format!("{{\"template\":\"{tpl}\",\"data\":{{\"n\":1}}}}").into_bytes(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(tpl.as_bytes().to_vec()))
            .mount(&server)
            .await;
        let out = if f == 0 {
            pdf::generate_invoice_pdf(&json!({"n": 1})).await
        } else {
            pdf::generate_proforma_pdf(&json!({"n": 1})).await
        };
        assert_eq!(out.unwrap(), tpl.as_bytes());
    }
    server.reset().await;

    Mock::given(method("POST"))
        .and(path("/render"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let err = pdf::generate_pdf("memo", &json!({}), None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, HttpxError::Status { status: 500, .. }));
    assert_eq!(
        err.to_string(),
        format!(
            "Server error '500 Internal Server Error' for url '{}/render'\n\
             For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/500",
            server.uri()
        )
    );
}
