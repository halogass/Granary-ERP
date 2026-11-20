//! 3 Nov 2026: the outbound seal. `sign_pdf_with(.., FillAndSign)` is a certification seal with DocMDP P=2
//! that validates like the e-sign seal; `sign_pdf` stays P=1; a customer's own signature appended to a P=2
//! copy keeps the company's seal intact; a sealed copy is never sealed twice.

mod pdf_sign_common;
use pdf_sign_common as c;

use granary_services::pdf_sign::{self, DocMdp};

#[tokio::test]
async fn the_outbound_seal_is_a_p2_certification_that_still_takes_a_signature() {
    c::init_env(&c::fresh_seal(), "", "");
    for name in ["fixed.pdf", "info.pdf", "xrefstream.pdf"] {
        let input = c::read(&format!("inputs/{name}"));
        assert_eq!(pdf_sign::docmdp_permission(&input), None, "{name}");

        let (sealed, level) = pdf_sign::sign_pdf_with(&input, DocMdp::FillAndSign)
            .await
            .unwrap();
        assert_eq!(level, "pades-b-b");
        assert!(sealed.starts_with(&input), "{name}: an incremental update");
        assert_eq!(pdf_sign::docmdp_permission(&sealed), Some(2), "{name}");
        let r = pdf_sign::validate_pdf(&sealed);
        assert_eq!((r.intact, r.valid), (Some(true), Some(true)), "{name}");

        // the e-sign final seal is unchanged: P=1
        let (final_seal, _) = pdf_sign::sign_pdf(&input).await.unwrap();
        assert_eq!(pdf_sign::docmdp_permission(&final_seal), Some(1), "{name}");
        let r = pdf_sign::validate_pdf(&final_seal);
        assert_eq!((r.intact, r.valid), (Some(true), Some(true)), "{name}");

        // a second (approval) signature on the P=2 copy: the company's seal still validates
        let signed = pdf_sign::_append_approval_signature(&sealed, "CustomerSignature")
            .await
            .unwrap();
        assert!(signed.starts_with(&sealed), "{name}");
        assert_eq!(pdf_sign::docmdp_permission(&signed), Some(2), "{name}");
        let r = pdf_sign::validate_pdf(&signed);
        assert_eq!((r.intact, r.valid), (Some(true), Some(true)), "{name}");

        // a sealed copy is never sealed again (e-sign keeps reading the unsealed master for that reason)
        let e = pdf_sign::sign_pdf_with(&sealed, DocMdp::FillAndSign)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("GranarySeal"), "{name}: {e}");
    }
    // an edit after sealing breaks it
    let input = c::read("inputs/fixed.pdf");
    let (mut sealed, _) = pdf_sign::sign_pdf_with(&input, DocMdp::FillAndSign)
        .await
        .unwrap();
    let at = input.len() / 2;
    sealed[at] ^= 0x01;
    assert_ne!(pdf_sign::validate_pdf(&sealed).intact, Some(true));
}
