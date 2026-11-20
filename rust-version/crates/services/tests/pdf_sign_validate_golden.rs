//! validate_pdf / cert_fingerprint on documents sealed here: a good seal, tampered bytes, a corrupted
//! signature value, an unsigned PDF and garbage.

mod pdf_sign_common;
use pdf_sign_common as c;

use granary_services::pdf_sign;
use sha2::{Digest, Sha256};

fn report(pdf: &[u8]) -> (Option<bool>, Option<bool>, Option<String>) {
    let r = pdf_sign::validate_pdf(pdf);
    (r.intact, r.valid, r.signer)
}

#[tokio::test]
async fn validate_pdf_reports() {
    c::init_env(&c::fresh_seal(), "", "");
    let input = c::read("inputs/fixed.pdf");
    let (sealed, _) = pdf_sign::sign_pdf(&input).await.unwrap();
    let signer = Some("Your Company Ltd".to_string());
    assert_eq!(report(&sealed), (Some(true), Some(true), signer.clone()));

    // a byte of the original content changed: no longer intact, the signature itself is still good
    let mut tampered = sealed.clone();
    tampered[input.len() / 2] ^= 0x01;
    assert_eq!(report(&tampered).0, Some(false));

    // the signature value corrupted (the last hex digits of /Contents before its padding)
    // byte offsets (a lossy UTF-8 view of the binary PDF would shift them)
    let key = b"/Contents <";
    let start = sealed
        .windows(key.len())
        .rposition(|w| w == key)
        .expect("a signature container")
        + key.len();
    let hex_end = start + sealed[start..].iter().position(|b| *b == b'>').unwrap();
    let hex = &sealed[start..hex_end];
    let digits = hex.len() - hex.iter().rev().take_while(|b| **b == b'0').count();
    let mut badsig = sealed.clone();
    let at = start + digits - 4;
    badsig[at] = if badsig[at] == b'1' { b'2' } else { b'1' };
    assert_ne!(report(&badsig).1, Some(true));

    // unsigned and not a PDF at all
    assert_eq!(
        report(&c::read("py/unsigned_fixed.pdf")),
        (None, None, None)
    );
    assert_eq!(report(&c::read("py/garbage.bin")), (None, None, None));

    // the fingerprint is the SHA-256 of the seal's certificate, stable for the process
    let fp = pdf_sign::cert_fingerprint().expect("generated seal");
    assert_eq!(fp.len(), 64);
    assert_eq!(pdf_sign::cert_fingerprint().as_deref(), Some(fp.as_str()));
    assert_ne!(fp, hex::encode(Sha256::digest(b"")));
    assert_eq!(pdf_sign::configured_seal_level(), "pades-b-b");
    assert!(!pdf_sign::is_qualified());
}
