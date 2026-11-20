//! PAdES-B-B seal (no TSA configured): every input layout (PyMuPDF-written, fixed, with an /Info
//! dictionary, with an xref stream) takes an incremental certification seal that validates, and a sealed
//! document cannot be sealed again.

mod pdf_sign_common;
use pdf_sign_common as c;

use granary_services::pdf_sign;

#[tokio::test]
async fn sign_pdf_b_b_seals_every_input_layout() {
    c::init_env(&c::fresh_seal(), "", "");
    let cases = c::cases();
    for case in cases["sign"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s.get("tsa").is_none())
    {
        let name = case["input"].as_str().unwrap();
        let input = c::read(&format!("inputs/{name}"));
        let (out, level) = pdf_sign::sign_pdf(&input).await.unwrap();
        c::dump(&format!("rs_bb_{name}"), &out);
        assert_eq!(level, case["level"].as_str().unwrap());
        assert!(out.starts_with(&input), "{name}: an incremental update");
        assert_eq!(pdf_sign::docmdp_permission(&out), Some(1), "{name}");
        let r = pdf_sign::validate_pdf(&out);
        // PyMuPDF's "65536" xref quirk (fitz.pdf): the validator cannot read the result, as pyHanko could not
        let want = if case["validates"] == false {
            (None, None, None)
        } else {
            (Some(true), Some(true), Some("Your Company Ltd"))
        };
        assert_eq!((r.intact, r.valid, r.signer.as_deref()), want, "{name}");
    }
}

#[tokio::test]
async fn a_sealed_document_cannot_be_sealed_again() {
    c::init_env(&c::fresh_seal(), "", "");
    let (sealed, _) = pdf_sign::sign_pdf(&c::read("inputs/fixed.pdf"))
        .await
        .unwrap();
    let e = pdf_sign::sign_pdf(&sealed).await.unwrap_err().to_string();
    assert_eq!(
        e,
        "Signature field with name GranarySeal appears to be filled already."
    );
}
