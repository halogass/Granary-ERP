//! PKCS#12 handling: `_generate_self_signed_p12` (cryptography's CertificateBuilder +
//! `pkcs12.serialize_key_and_certificates(..., NoEncryption())`) and `SimpleSigner.load_pkcs12`
//! (OpenSSL `PKCS12_parse` semantics: the signing cert is the first one matching the key).

use super::cms::{
    self, explicit, integer_from_bytes, integer_u64, null, octet, oid, seq, tlv, Cert, SigningKey,
};
use sha2::{Digest, Sha256};

/// The signer pyHanko keeps per process.
pub struct Signer {
    pub key: SigningKey,
    pub cert: Cert,
    pub other_certs: Vec<Cert>,
}

const OID_CERT_BAG: &str = "1.2.840.113549.1.12.10.1.3";
const OID_KEY_BAG: &str = "1.2.840.113549.1.12.10.1.1";
const OID_X509_CERT: &str = "1.2.840.113549.1.9.22.1";
const OID_FRIENDLY_NAME: &str = "1.2.840.113549.1.9.20";

/// RFC 7292 appendix B key derivation (`pass_bmp` is the BMPString password incl. terminator, or empty
/// for OpenSSL's NULL password).
fn pkcs12_kdf_sha256(pass_bmp: &[u8], salt: &[u8], id: u8, iterations: u32, n: usize) -> Vec<u8> {
    const U: usize = 32;
    const V: usize = 64;
    let fill = |src: &[u8]| -> Vec<u8> {
        if src.is_empty() {
            return Vec::new();
        }
        let len = V * src.len().div_ceil(V);
        (0..len).map(|i| src[i % src.len()]).collect()
    };
    let d = vec![id; V];
    let mut i_buf = fill(salt);
    i_buf.extend(fill(pass_bmp));
    let mut out = Vec::new();
    while out.len() < n {
        let mut a = Sha256::new()
            .chain_update(&d)
            .chain_update(&i_buf)
            .finalize()
            .to_vec();
        for _ in 1..iterations {
            a = Sha256::digest(&a).to_vec();
        }
        out.extend_from_slice(&a[..U.min(n - out.len())]);
        if out.len() >= n {
            break;
        }
        let b: Vec<u8> = (0..V).map(|k| a[k % U]).collect();
        for chunk in i_buf.chunks_mut(V) {
            let mut carry = 1u16;
            for k in (0..V).rev() {
                let s = chunk[k] as u16 + b[k] as u16 + carry;
                chunk[k] = s as u8;
                carry = s >> 8;
            }
        }
    }
    out
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    let mut m = Hmac::<Sha256>::new_from_slice(key).expect("hmac key");
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

fn bmp(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
}

fn utc_time(ts: chrono::DateTime<chrono::Utc>) -> Vec<u8> {
    // cryptography writes UTCTime for years before 2050, GeneralizedTime from 2050 on
    use chrono::Datelike;
    if ts.year() < 2050 {
        tlv(0x17, ts.format("%y%m%d%H%M%SZ").to_string().as_bytes())
    } else {
        tlv(0x18, ts.format("%Y%m%d%H%M%SZ").to_string().as_bytes())
    }
}

/// x509.Name([CN, O]) as cryptography encodes it (UTF8String). No country: the seal name says who it is.
fn seal_name(name: &str) -> Vec<u8> {
    let rdn = |o: &str, v: Vec<u8>| cms::set_sorted(&[seq(&[oid(o), v])]);
    seq(&[
        rdn("2.5.4.3", tlv(0x0C, name.as_bytes())),
        rdn("2.5.4.10", tlv(0x0C, name.as_bytes())),
    ])
}

/// Self-signed RSA-2048 sealing certificate, written as an unencrypted PKCS#12 (atomic rename).
pub fn generate_self_signed_p12(path: &str, seal_name_str: &str) -> Result<(), String> {
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey};
    let mut rng = rand::rngs::OsRng;
    let key = rsa::RsaPrivateKey::new(&mut rng, 2048).map_err(|e| e.to_string())?;
    let spki = key
        .to_public_key()
        .to_public_key_der()
        .map_err(|e| e.to_string())?
        .as_bytes()
        .to_vec();
    let name = seal_name(seal_name_str);
    let now = chrono::Utc::now();
    let now = now - chrono::Duration::nanoseconds(now.timestamp_subsec_nanos() as i64);
    // x509.random_serial_number(): 20 random bytes >> 1
    let mut serial: [u8; 20] = rand::random();
    let mut carry = 0u8;
    for b in serial.iter_mut() {
        let nb = (*b >> 1) | (carry << 7);
        carry = *b & 1;
        *b = nb;
    }
    let sig_alg = seq(&[oid(cms::OID_SHA256_RSA), null()]);
    let ext = |o: &str, critical: bool, value: Vec<u8>| {
        let mut parts = vec![oid(o)];
        if critical {
            parts.push(tlv(0x01, &[0xFF]));
        }
        parts.push(octet(&value));
        seq(&parts)
    };
    let extensions = seq(&[
        ext("2.5.29.19", true, seq(&[])),
        ext("2.5.29.15", true, tlv(0x03, &[0x06, 0xC0])),
        ext(
            "2.5.29.37",
            false,
            seq(&[oid("1.3.6.1.5.5.7.3.36"), oid("1.3.6.1.5.5.7.3.4")]),
        ),
    ]);
    let tbs = seq(&[
        explicit(0, &integer_u64(2)),
        integer_from_bytes(&serial),
        sig_alg.clone(),
        name.clone(),
        seq(&[
            utc_time(now - chrono::Duration::days(1)),
            utc_time(now + chrono::Duration::days(3650)),
        ]),
        name,
        spki,
        explicit(3, &extensions),
    ]);
    let signature = SigningKey::Rsa(key.clone()).sign_sha256(&tbs)?;
    let mut bits = vec![0u8];
    bits.extend_from_slice(&signature);
    let cert_der = seq(&[tbs, sig_alg, tlv(0x03, &bits)]);

    let pkcs8 = key
        .to_pkcs8_der()
        .map_err(|e| e.to_string())?
        .as_bytes()
        .to_vec();
    let friendly = cms::set_sorted(&[seq(&[
        oid(OID_FRIENDLY_NAME),
        cms::set_sorted(&[tlv(0x1E, &bmp("granary-seal"))]),
    ])]);
    let cert_bag = seq(&[
        oid(OID_CERT_BAG),
        explicit(
            0,
            &seq(&[oid(OID_X509_CERT), explicit(0, &octet(&cert_der))]),
        ),
        friendly.clone(),
    ]);
    let key_bag = seq(&[oid(OID_KEY_BAG), explicit(0, &pkcs8), friendly]);
    let data_ci = |content: Vec<u8>| seq(&[oid(cms::OID_DATA), explicit(0, &octet(&content))]);
    let auth_safe = seq(&[data_ci(seq(&[cert_bag])), data_ci(seq(&[key_bag]))]);
    // MAC: HMAC-SHA256, 2048 iterations, 8-byte salt, OpenSSL's NULL password (empty)
    let salt: [u8; 8] = rand::random();
    let mac_key = pkcs12_kdf_sha256(&[], &salt, 3, 2048, 32);
    let mac = hmac_sha256(&mac_key, &auth_safe);
    let mac_data = seq(&[
        seq(&[cms::digest_alg_id("sha256"), octet(&mac)]),
        octet(&salt),
        integer_u64(2048),
    ]);
    let pfx = seq(&[integer_u64(3), data_ci(auth_safe), mac_data]);

    let p = std::path::Path::new(path);
    if let Some(dir) = p.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
    }
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, &pfx).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?; // atomic: avoids races across workers
    tracing::info!("Generated self-signed signing certificate at {}", path);
    Ok(())
}

/// Drop the MacData of a PFX after verifying it with the NULL (empty) password, so the bags can be read
/// with p12-keystore (which only knows `&str` passwords). Returns `None` when the MAC does not verify that way.
fn strip_mac_if_null_password(data: &[u8]) -> Option<Vec<u8>> {
    let (pfx, _) = cms::read_tlv(data).ok()?;
    let parts = cms::children(pfx.content).ok()?;
    if parts.len() < 3 {
        return Some(data.to_vec());
    }
    let auth = cms::children(parts[1].content).ok()?;
    let (oct, _) = cms::read_tlv(auth.get(1)?.content).ok()?;
    let mac_data = cms::children(parts[2].content).ok()?;
    let digest_info = cms::children(mac_data[0].content).ok()?;
    let alg = cms::oid_to_string(cms::children(digest_info[0].content).ok()?[0].content);
    if alg != cms::OID_SHA256 {
        return None;
    }
    let salt = mac_data.get(1)?.content;
    let iters = mac_data
        .get(2)
        .map(|t| t.content.iter().fold(0u32, |a, &b| (a << 8) | b as u32))
        .unwrap_or(1);
    let key = pkcs12_kdf_sha256(&[], salt, 3, iters, 32);
    if hmac_sha256(&key, oct.content) != digest_info[1].content {
        return None;
    }
    Some(seq(&[parts[0].raw.to_vec(), parts[1].raw.to_vec()]))
}

/// `SimpleSigner.load_pkcs12(pfx_file=path, passphrase=...)`.
pub fn load_pkcs12(path: &str, passphrase: Option<&str>) -> Result<Signer, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let archive = match passphrase {
        Some(pw) => {
            p12_keystore::Pkcs12Archive::from_pkcs12(&data, pw).map_err(|e| format!("{e:?}"))?
        }
        None => match strip_mac_if_null_password(&data) {
            Some(stripped) => p12_keystore::Pkcs12Archive::from_pkcs12(&stripped, "")
                .map_err(|e| format!("{e:?}"))?,
            None => {
                p12_keystore::Pkcs12Archive::from_pkcs12(&data, "").map_err(|e| format!("{e:?}"))?
            }
        },
    };
    let key_bag = archive
        .keys
        .first()
        .ok_or("PKCS#12 archive has no private key")?;
    let key = SigningKey::from_pkcs8(key_bag.key.as_der())?;
    let key_spki = public_spki(&key)?;
    let mut cert = None;
    let mut others = Vec::new();
    for c in &archive.certs {
        let parsed = Cert::parse(c.cert.as_der())?;
        if cert.is_none() && spki_matches(&parsed.spki, &key_spki) {
            cert = Some(parsed);
        } else {
            others.push(parsed);
        }
    }
    let cert = cert.ok_or("PKCS#12 archive has no certificate for its key")?;
    Ok(Signer {
        key,
        cert,
        other_certs: others,
    })
}

fn public_spki(key: &SigningKey) -> Result<Vec<u8>, String> {
    use rsa::pkcs8::EncodePublicKey;
    Ok(match key {
        SigningKey::Rsa(k) => k
            .to_public_key()
            .to_public_key_der()
            .map_err(|e| e.to_string())?
            .as_bytes()
            .to_vec(),
        SigningKey::P256(k) => k
            .verifying_key()
            .to_public_key_der()
            .map_err(|e| e.to_string())?
            .as_bytes()
            .to_vec(),
        SigningKey::P384(k) => k
            .verifying_key()
            .to_public_key_der()
            .map_err(|e| e.to_string())?
            .as_bytes()
            .to_vec(),
    })
}

/// Compare the subjectPublicKey BIT STRINGs (AlgorithmIdentifier parameters may be encoded differently).
fn spki_matches(a: &[u8], b: &[u8]) -> bool {
    let bits = |x: &[u8]| -> Option<Vec<u8>> {
        let (t, _) = cms::read_tlv(x).ok()?;
        Some(cms::children(t.content).ok()?.get(1)?.content.to_vec())
    };
    matches!((bits(a), bits(b)), (Some(x), Some(y)) if x == y)
}
