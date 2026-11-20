//! _generate_self_signed_p12: a missing SIGN_CERT_P12_PATH gets a self-signed RSA-2048 seal (CN and
//! O = SIGN_SEAL_NAME, no country, 10 years, keyUsage digitalSignature + contentCommitment critical, EKU
//! documentSigning + emailProtection), written as an unencrypted PKCS#12 that then signs.

mod pdf_sign_common;
use pdf_sign_common as c;

use granary_services::pdf_sign;

#[tokio::test]
async fn bootstrap_certificate_is_generated_and_used() {
    let d = std::env::temp_dir().join(format!("pdf_sign_boot_{}", std::process::id()));
    let path = d.join("keys/platform.p12");
    let _ = std::fs::remove_dir_all(&d);
    c::init_env(path.to_str().unwrap(), "", "");
    let fp = pdf_sign::cert_fingerprint().expect("generated");
    assert!(path.exists() && !d.join("keys/platform.p12.tmp").exists());
    assert_eq!(fp.len(), 64);
    // the certificate inside: subject, validity, extensions
    let p12 = std::fs::read(&path).unwrap();
    let hexs = hex::encode(&p12);
    assert!(
        hexs.contains(&hex::encode("Your Company")),
        "CN/O = SIGN_SEAL_NAME"
    );
    assert!(
        !hexs.contains("0603550406"),
        "no countryName in the subject"
    );
    assert!(
        hexs.contains("300e0603551d0f0101ff0404030206c0"),
        "keyUsage digitalSignature + contentCommitment, critical"
    );
    assert!(
        hexs.contains("301d0603551d250416301406082b0601050507032406082b06010505070304"),
        "EKU"
    );
    assert!(
        hexs.contains("300c0603551d130101ff04023000"),
        "basicConstraints CA:false, critical"
    );
    let input = c::read("inputs/fixed.pdf");
    let (out, level) = pdf_sign::sign_pdf(&input).await.unwrap();
    assert_eq!(level, "pades-b-b");
    let r = pdf_sign::validate_pdf(&out);
    assert_eq!((r.intact, r.valid), (Some(true), Some(true)));
    let _ = std::fs::remove_dir_all(&d);
}
