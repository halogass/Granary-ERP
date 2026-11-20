//! PAdES-B-LTA with a CA-issued seal (password-protected .p12 with its chain) and a TSA under an injected
//! trust root: DSS revision with the certificates of both paths and VRI entries, a document timestamp,
//! then the DSS update for the timestamp's VRI. Three TSA requests.

mod pdf_sign_common;
use pdf_sign_common as c;

use granary_services::pdf_sign;

#[tokio::test]
async fn sign_pdf_b_lta_with_a_trusted_chain() {
    let tsa = c::start_tsa("pki/tsa").await;
    c::init_env(
        &c::temp_copy("pki/seal.p12"),
        "pw123",
        &format!("{}/", tsa.uri()),
    );
    pdf_sign::_set_extra_trust_roots(vec![c::read("pki/root.der")]);
    let input = c::read("inputs/fixed.pdf");
    let (out, level) = pdf_sign::sign_pdf(&input).await.unwrap();
    c::dump("rs_lta_fixed.pdf", &out);
    assert_eq!(level, "pades-b-lta");
    assert_eq!(tsa.received_requests().await.unwrap().len(), 3);
    assert!(out.starts_with(&input));
    let r = pdf_sign::validate_pdf(&out);
    assert_eq!((r.intact, r.valid), (Some(true), Some(true)));

    // without the trust root the seal's chain does not validate: B-T
    pdf_sign::_set_extra_trust_roots(vec![]);
    let (_, level) = pdf_sign::sign_pdf(&input).await.unwrap();
    assert_eq!(level, "pades-b-t");
}
