//! TSA configured, self-signed seal: B-LTA fails pre-sign validation (self-signed certificate), the
//! seal falls back to PAdES-B-T: a dummy timestamp sizes the container, a second one stamps the signature
//! (two TSA requests).

mod pdf_sign_common;
use pdf_sign_common as c;

use granary_services::pdf_sign;

#[tokio::test]
async fn sign_pdf_falls_back_to_b_t() {
    let tsa = c::start_tsa("selfsigned_tsa").await;
    c::init_env(&c::fresh_seal(), "", &format!("{}/", tsa.uri()));
    let input = c::read("inputs/fixed.pdf");
    let (out, level) = pdf_sign::sign_pdf(&input).await.unwrap();
    c::dump("rs_bt_fixed.pdf", &out);
    assert_eq!(level, "pades-b-t");
    assert_eq!(tsa.received_requests().await.unwrap().len(), 2);
    assert!(out.starts_with(&input));
    let r = pdf_sign::validate_pdf(&out);
    assert_eq!(
        (r.intact, r.valid, r.signer.as_deref()),
        (Some(true), Some(true), Some("Your Company Ltd"))
    );
    assert_eq!(pdf_sign::configured_seal_level(), "pades-b-lta");
}
