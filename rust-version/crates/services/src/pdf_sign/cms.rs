//! CMS / X.509 / RFC 3161 pieces of pyHanko + asn1crypto, hand-encoded so every byte is under control
//! (asn1crypto keeps SET OF members in the order they were given, except that it DER-sorts signed
//! attributes; signature verification uses the signed attributes exactly as they were received).

use sha2::{Digest, Sha256};

pub type CmsResult<T> = Result<T, String>;

// ─── DER writing ─────────────────────────────────────────────────────────────

pub fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let n = content.len();
    if n < 0x80 {
        out.push(n as u8);
    } else {
        let bytes: Vec<u8> = n
            .to_be_bytes()
            .iter()
            .copied()
            .skip_while(|b| *b == 0)
            .collect();
        out.push(0x80 | bytes.len() as u8);
        out.extend_from_slice(&bytes);
    }
    out.extend_from_slice(content);
    out
}

pub fn seq(items: &[Vec<u8>]) -> Vec<u8> {
    tlv(0x30, &items.concat())
}

/// SET OF in the given order (asn1crypto SetOf without sorting).
pub fn set_unsorted(items: &[Vec<u8>]) -> Vec<u8> {
    tlv(0x31, &items.concat())
}

/// SET OF with DER ordering (sorted by encoding).
pub fn set_sorted(items: &[Vec<u8>]) -> Vec<u8> {
    let mut v = items.to_vec();
    v.sort();
    tlv(0x31, &v.concat())
}

pub fn integer_from_bytes(be: &[u8]) -> Vec<u8> {
    let mut b: Vec<u8> = be.iter().copied().skip_while(|x| *x == 0).collect();
    if b.is_empty() {
        b.push(0);
    }
    if b[0] & 0x80 != 0 {
        b.insert(0, 0);
    }
    tlv(0x02, &b)
}

pub fn integer_u64(v: u64) -> Vec<u8> {
    integer_from_bytes(&v.to_be_bytes())
}

pub fn octet(b: &[u8]) -> Vec<u8> {
    tlv(0x04, b)
}

pub fn null() -> Vec<u8> {
    vec![0x05, 0x00]
}

pub fn oid(dotted: &str) -> Vec<u8> {
    let parts: Vec<u64> = dotted.split('.').map(|p| p.parse().expect("oid")).collect();
    let mut body = vec![(parts[0] * 40 + parts[1]) as u8];
    for &p in &parts[2..] {
        let mut stack = vec![(p & 0x7F) as u8];
        let mut v = p >> 7;
        while v > 0 {
            stack.push(0x80 | (v & 0x7F) as u8);
            v >>= 7;
        }
        stack.reverse();
        body.extend(stack);
    }
    tlv(0x06, &body)
}

pub fn explicit(n: u8, content: &[u8]) -> Vec<u8> {
    tlv(0xA0 | n, content)
}

pub const OID_DATA: &str = "1.2.840.113549.1.7.1";
pub const OID_SIGNED_DATA: &str = "1.2.840.113549.1.7.2";
pub const OID_CONTENT_TYPE: &str = "1.2.840.113549.1.9.3";
pub const OID_MESSAGE_DIGEST: &str = "1.2.840.113549.1.9.4";
pub const OID_SIGNING_CERT: &str = "1.2.840.113549.1.9.16.2.12";
pub const OID_SIGNING_CERT_V2: &str = "1.2.840.113549.1.9.16.2.47";
pub const OID_TST_TOKEN: &str = "1.2.840.113549.1.9.16.2.14";
pub const OID_SHA256: &str = "2.16.840.1.101.3.4.2.1";
pub const OID_SHA1: &str = "1.3.14.3.2.26";
pub const OID_SHA384: &str = "2.16.840.1.101.3.4.2.2";
pub const OID_SHA512: &str = "2.16.840.1.101.3.4.2.3";
pub const OID_RSA: &str = "1.2.840.113549.1.1.1";
pub const OID_SHA256_RSA: &str = "1.2.840.113549.1.1.11";
pub const OID_SHA384_RSA: &str = "1.2.840.113549.1.1.12";
pub const OID_SHA512_RSA: &str = "1.2.840.113549.1.1.13";
pub const OID_SHA1_RSA: &str = "1.2.840.113549.1.1.5";
pub const OID_EC_PUBKEY: &str = "1.2.840.10045.2.1";
pub const OID_ECDSA_SHA256: &str = "1.2.840.10045.4.3.2";
pub const OID_ECDSA_SHA384: &str = "1.2.840.10045.4.3.3";
pub const OID_ECDSA_SHA512: &str = "1.2.840.10045.4.3.4";

/// AlgorithmIdentifier as asn1crypto dumps a DigestAlgorithm: sha* digests carry an explicit NULL.
pub fn digest_alg_id(name: &str) -> Vec<u8> {
    let o = match name {
        "sha1" => OID_SHA1,
        "sha384" => OID_SHA384,
        "sha512" => OID_SHA512,
        _ => OID_SHA256,
    };
    seq(&[oid(o), null()])
}

// ─── DER reading ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct Tlv<'a> {
    pub tag: u8,
    pub content: &'a [u8],
    pub raw: &'a [u8],
}

pub fn read_tlv(data: &[u8]) -> CmsResult<(Tlv<'_>, &[u8])> {
    if data.len() < 2 {
        return Err("truncated DER".into());
    }
    let tag = data[0];
    let (len, hl) = if data[1] < 0x80 {
        (data[1] as usize, 2)
    } else if data[1] == 0x80 {
        return Err("indefinite length not supported".into());
    } else {
        let n = (data[1] & 0x7F) as usize;
        if n > 8 || data.len() < 2 + n {
            return Err("bad DER length".into());
        }
        (
            data[2..2 + n]
                .iter()
                .fold(0usize, |a, &b| (a << 8) | b as usize),
            2 + n,
        )
    };
    let end = hl
        .checked_add(len)
        .filter(|e| *e <= data.len())
        .ok_or("DER length overruns")?;
    Ok((
        Tlv {
            tag,
            content: &data[hl..end],
            raw: &data[..end],
        },
        &data[end..],
    ))
}

pub fn children(content: &[u8]) -> CmsResult<Vec<Tlv<'_>>> {
    let mut out = Vec::new();
    let mut rest = content;
    while !rest.is_empty() {
        let (t, r) = read_tlv(rest)?;
        out.push(t);
        rest = r;
    }
    Ok(out)
}

pub fn oid_to_string(content: &[u8]) -> String {
    if content.is_empty() {
        return String::new();
    }
    let mut parts = vec![(content[0] / 40) as u64, (content[0] % 40) as u64];
    let mut v = 0u64;
    for &b in &content[1..] {
        v = (v << 7) | (b & 0x7F) as u64;
        if b & 0x80 == 0 {
            parts.push(v);
            v = 0;
        }
    }
    parts
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

// ─── certificates ────────────────────────────────────────────────────────────

/// The parts of an X.509 certificate the seal needs (raw DER slices kept for exact re-encoding).
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Cert {
    pub der: Vec<u8>,
    pub tbs: Vec<u8>,
    pub serial: Vec<u8>,
    /// INTEGER TLV of the serial.
    pub serial_tlv: Vec<u8>,
    pub issuer: Vec<u8>,
    pub subject: Vec<u8>,
    pub spki: Vec<u8>,
    pub sig_alg: String,
    pub signature: Vec<u8>,
    pub not_before: i64,
    pub not_after: i64,
    /// (oid, critical, value octets)
    pub extensions: Vec<(String, bool, Vec<u8>)>,
}

fn parse_time(t: &Tlv) -> CmsResult<i64> {
    let s = std::str::from_utf8(t.content).map_err(|e| e.to_string())?;
    let s = s.trim_end_matches('Z');
    let full = if t.tag == 0x17 {
        let yy: i32 = s[..2].parse().map_err(|_| "bad time")?;
        format!(
            "{}{}",
            if yy >= 50 { 1900 + yy } else { 2000 + yy },
            &s[2..]
        )
    } else {
        s.to_string()
    };
    let dt = chrono::NaiveDateTime::parse_from_str(&full[..14.min(full.len())], "%Y%m%d%H%M%S")
        .map_err(|e| e.to_string())?;
    Ok(dt.and_utc().timestamp())
}

impl Cert {
    pub fn parse(der: &[u8]) -> CmsResult<Cert> {
        let (c, _) = read_tlv(der)?;
        let parts = children(c.content)?;
        if parts.len() < 3 {
            return Err("bad certificate".into());
        }
        let tbs = parts[0];
        let mut f = children(tbs.content)?.into_iter().peekable();
        if f.peek().map(|t| t.tag == 0xA0).unwrap_or(false) {
            f.next();
        }
        let serial = f.next().ok_or("cert: serial")?;
        let _sigalg = f.next().ok_or("cert: sigalg")?;
        let issuer = f.next().ok_or("cert: issuer")?;
        let validity = f.next().ok_or("cert: validity")?;
        let subject = f.next().ok_or("cert: subject")?;
        let spki = f.next().ok_or("cert: spki")?;
        let mut extensions = Vec::new();
        for t in f {
            if t.tag == 0xA3 {
                let (exts, _) = read_tlv(t.content)?;
                for e in children(exts.content)? {
                    let ef = children(e.content)?;
                    let id = oid_to_string(ef[0].content);
                    let (crit, val) = if ef.len() == 3 {
                        (
                            ef[1].content.first().copied().unwrap_or(0) != 0,
                            ef[2].content.to_vec(),
                        )
                    } else {
                        (false, ef[1].content.to_vec())
                    };
                    extensions.push((id, crit, val));
                }
            }
        }
        let v = children(validity.content)?;
        let sig_alg_parts = children(parts[1].content)?;
        let sig = parts[2].content;
        Ok(Cert {
            der: c.raw.to_vec(),
            tbs: tbs.raw.to_vec(),
            serial: serial.content.to_vec(),
            serial_tlv: serial.raw.to_vec(),
            issuer: issuer.raw.to_vec(),
            subject: subject.raw.to_vec(),
            spki: spki.raw.to_vec(),
            sig_alg: oid_to_string(sig_alg_parts[0].content),
            signature: sig.get(1..).unwrap_or_default().to_vec(),
            not_before: parse_time(&v[0])?,
            not_after: parse_time(&v[1])?,
            extensions,
        })
    }

    pub fn ext(&self, id: &str) -> Option<&(String, bool, Vec<u8>)> {
        self.extensions.iter().find(|(o, _, _)| o == id)
    }

    pub fn self_issued(&self) -> bool {
        self.issuer == self.subject
    }

    /// keyUsage bit `n` (0 = digitalSignature, 1 = nonRepudiation/contentCommitment ...); `None` without the extension.
    pub fn key_usage_bit(&self, n: usize) -> Option<bool> {
        let (_, _, v) = self.ext("2.5.29.15")?;
        let (bs, _) = read_tlv(v).ok()?;
        let bits = bs.content.get(1..).unwrap_or_default();
        Some(
            bits.get(n / 8)
                .map(|b| b & (0x80 >> (n % 8)) != 0)
                .unwrap_or(false),
        )
    }
}

// ─── keys and signing ────────────────────────────────────────────────────────

pub enum SigningKey {
    Rsa(rsa::RsaPrivateKey),
    P256(p256::ecdsa::SigningKey),
    P384(p384::ecdsa::SigningKey),
}

impl SigningKey {
    pub fn from_pkcs8(der: &[u8]) -> CmsResult<SigningKey> {
        use rsa::pkcs8::DecodePrivateKey;
        if let Ok(k) = rsa::RsaPrivateKey::from_pkcs8_der(der) {
            return Ok(SigningKey::Rsa(k));
        }
        if let Ok(k) = p256::SecretKey::from_pkcs8_der(der) {
            return Ok(SigningKey::P256(p256::ecdsa::SigningKey::from(k)));
        }
        if let Ok(k) = p384::SecretKey::from_pkcs8_der(der) {
            return Ok(SigningKey::P384(p384::ecdsa::SigningKey::from(k)));
        }
        Err("unsupported private key type".into())
    }

    /// signerInfo signatureAlgorithm as pyHanko picks it for SHA-256 (RSA: sha256_rsa with NULL params).
    pub fn signature_alg_id(&self) -> Vec<u8> {
        match self {
            SigningKey::Rsa(_) => seq(&[oid(OID_SHA256_RSA), null()]),
            _ => seq(&[oid(OID_ECDSA_SHA256)]),
        }
    }

    pub fn sign_sha256(&self, data: &[u8]) -> CmsResult<Vec<u8>> {
        match self {
            SigningKey::Rsa(k) => {
                let digest = Sha256::digest(data);
                k.sign(rsa::Pkcs1v15Sign::new::<Sha256>(), &digest)
                    .map_err(|e| e.to_string())
            }
            SigningKey::P256(k) => {
                use p256::ecdsa::signature::Signer;
                let s: p256::ecdsa::Signature = k.sign(data);
                Ok(s.to_der().as_bytes().to_vec())
            }
            SigningKey::P384(k) => {
                // pyHanko signs with the requested digest (SHA-256) on every curve
                use p384::ecdsa::signature::hazmat::PrehashSigner;
                let digest = Sha256::digest(data);
                let s: p384::ecdsa::Signature =
                    k.sign_prehash(&digest).map_err(|e| e.to_string())?;
                Ok(s.to_der().as_bytes().to_vec())
            }
        }
    }
}

/// Verify `signature` over `data` with the public key in `spki` (DER SubjectPublicKeyInfo).
pub fn verify_signature(
    spki: &[u8],
    sig_alg_oid: &str,
    digest_name: &str,
    data: &[u8],
    signature: &[u8],
) -> bool {
    use rsa::pkcs8::DecodePublicKey;
    let hash = |name: &str| -> Option<(Vec<u8>, &str)> {
        Some(match name {
            "sha1" => (sha1::Sha1::digest(data).to_vec(), "sha1"),
            "sha256" => (Sha256::digest(data).to_vec(), "sha256"),
            "sha384" => (sha2::Sha384::digest(data).to_vec(), "sha384"),
            "sha512" => (sha2::Sha512::digest(data).to_vec(), "sha512"),
            _ => return None,
        })
    };
    let rsa_digest = match sig_alg_oid {
        OID_SHA256_RSA => Some("sha256"),
        OID_SHA384_RSA => Some("sha384"),
        OID_SHA512_RSA => Some("sha512"),
        OID_SHA1_RSA => Some("sha1"),
        // rsassa_pkcs1v15: the digest comes from the context (signerInfo digestAlgorithm)
        OID_RSA => Some(digest_name),
        _ => None,
    };
    if let Some(dn) = rsa_digest {
        let Ok(pk) = rsa::RsaPublicKey::from_public_key_der(spki) else {
            return false;
        };
        let Some((h, n)) = hash(dn) else { return false };
        let scheme = match n {
            // (the sha1 crate is built without its `oid` feature here: DigestInfo prefix spelled out)
            "sha1" => rsa::Pkcs1v15Sign {
                hash_len: Some(20),
                prefix: vec![
                    0x30, 0x21, 0x30, 0x09, 0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a, 0x05, 0x00,
                    0x04, 0x14,
                ]
                .into_boxed_slice(),
            },
            "sha384" => rsa::Pkcs1v15Sign::new::<sha2::Sha384>(),
            "sha512" => rsa::Pkcs1v15Sign::new::<sha2::Sha512>(),
            _ => rsa::Pkcs1v15Sign::new::<Sha256>(),
        };
        return pk.verify(scheme, &h, signature).is_ok();
    }
    let ec_digest = match sig_alg_oid {
        OID_ECDSA_SHA256 => "sha256",
        OID_ECDSA_SHA384 => "sha384",
        OID_ECDSA_SHA512 => "sha512",
        OID_EC_PUBKEY => digest_name,
        _ => return false,
    };
    let Some((h, _)) = hash(ec_digest) else {
        return false;
    };
    use p256::ecdsa::signature::hazmat::PrehashVerifier;
    if let Ok(pk) = p256::PublicKey::from_public_key_der(spki) {
        let vk = p256::ecdsa::VerifyingKey::from(pk);
        let Ok(sig) = p256::ecdsa::Signature::from_der(signature) else {
            return false;
        };
        return vk.verify_prehash(&h, &sig).is_ok();
    }
    if let Ok(pk) = p384::PublicKey::from_public_key_der(spki) {
        let vk = p384::ecdsa::VerifyingKey::from(pk);
        let Ok(sig) = p384::ecdsa::Signature::from_der(signature) else {
            return false;
        };
        return vk.verify_prehash(&h, &sig).is_ok();
    }
    false
}

// ─── signed attributes & SignedData ──────────────────────────────────────────

pub fn attribute(type_oid: &str, values: &[Vec<u8>]) -> Vec<u8> {
    seq(&[oid(type_oid), set_sorted(values)])
}

/// `as_signing_certificate_v2(cert)` with SHA-256 (the DEFAULT hash algorithm, so it is omitted).
pub fn signing_certificate_v2(cert: &Cert) -> Vec<u8> {
    let cert_hash = Sha256::digest(&cert.der);
    let issuer_serial = seq(&[seq(&[tlv(0xA4, &cert.issuer)]), cert.serial_tlv.clone()]);
    let ess = seq(&[octet(&cert_hash), issuer_serial]);
    seq(&[seq(&[ess])])
}

/// PAdES signed attributes (no signing-time): content-type, message-digest, signing-certificate-v2, DER-sorted.
pub fn pades_signed_attrs(document_digest: &[u8], cert: &Cert) -> Vec<Vec<u8>> {
    vec![
        attribute(OID_CONTENT_TYPE, &[oid(OID_DATA)]),
        attribute(OID_MESSAGE_DIGEST, &[octet(document_digest)]),
        attribute(OID_SIGNING_CERT_V2, &[signing_certificate_v2(cert)]),
    ]
}

/// Build the CMS ContentInfo of a detached signature. `tst` is an optional RFC 3161 token to embed as the
/// signature-time-stamp-token unsigned attribute (computed over the signature value by the caller).
pub fn signed_data(
    key: &SigningKey,
    cert: &Cert,
    extra_certs: &[Cert],
    document_digest: &[u8],
    tst: Option<&[u8]>,
) -> CmsResult<(Vec<u8>, Vec<u8>)> {
    let attrs = pades_signed_attrs(document_digest, cert);
    let signed_set = set_sorted(&attrs);
    let signature = key.sign_sha256(&signed_set)?;
    let der = assemble_signed_data(key, cert, extra_certs, &signed_set, &signature, tst);
    Ok((der, signature))
}

pub fn assemble_signed_data(
    key: &SigningKey,
    cert: &Cert,
    extra_certs: &[Cert],
    signed_set: &[u8],
    signature: &[u8],
    tst: Option<&[u8]>,
) -> Vec<u8> {
    let sid = seq(&[cert.issuer.clone(), cert.serial_tlv.clone()]);
    let mut si = vec![
        integer_u64(1),
        sid,
        digest_alg_id("sha256"),
        tlv(
            0xA0,
            read_tlv(signed_set)
                .map(|(t, _)| t.content)
                .unwrap_or_default(),
        ),
        key.signature_alg_id(),
        octet(signature),
    ];
    if let Some(token) = tst {
        si.push(tlv(0xA1, &attribute(OID_TST_TOKEN, &[token.to_vec()])));
    }
    let signer_info = seq(&si);
    let mut certs = vec![cert.der.clone()];
    for c in extra_certs {
        if c.der != cert.der {
            certs.push(c.der.clone());
        }
    }
    let sd = seq(&[
        integer_u64(1),
        set_sorted(&[digest_alg_id("sha256")]),
        seq(&[oid(OID_DATA)]),
        tlv(0xA0, &certs.concat()),
        set_unsorted(&[signer_info]),
    ]);
    seq(&[oid(OID_SIGNED_DATA), explicit(0, &sd)])
}

// ─── RFC 3161 client (pyhanko.sign.timestamps.HTTPTimeStamper) ───────────────

/// `get_nonce()`: a fixed-width 8-byte positive integer starting with 0x01.
pub fn new_nonce() -> [u8; 8] {
    let mut n: [u8; 8] = rand::random();
    n[0] = 0x01;
    n
}

pub fn timestamp_request(message_digest: &[u8], nonce: &[u8; 8]) -> Vec<u8> {
    seq(&[
        integer_u64(1),
        seq(&[digest_alg_id("sha256"), octet(message_digest)]),
        integer_from_bytes(nonce),
        tlv(0x01, &[0xFF]),
    ])
}

/// `handle_tsp_response`: the token (ContentInfo DER) when the status is `granted` and the nonce matches.
pub fn handle_tsp_response(resp: &[u8], nonce: &[u8; 8]) -> CmsResult<Vec<u8>> {
    let (top, _) = read_tlv(resp)?;
    let parts = children(top.content)?;
    let status_info = parts.first().ok_or("empty timestamp response")?;
    let si = children(status_info.content)?;
    let status = si
        .first()
        .map(|t| t.content.iter().fold(0i64, |a, &b| (a << 8) | b as i64))
        .unwrap_or(-1);
    if status != 0 {
        let mut strings = Vec::new();
        let mut fail_infos = Vec::new();
        for t in &si[1..] {
            if t.tag == 0x30 {
                for s in children(t.content)? {
                    strings.push(String::from_utf8_lossy(s.content).into_owned());
                }
            } else if t.tag == 0x03 {
                fail_infos.push(fail_info_names(t.content));
            }
        }
        return Err(format!(
            "Timestamp server refused our request: statusString \"{}\", failInfo \"{}\"",
            strings.join("; "),
            fail_infos.concat().join("; ")
        ));
    }
    let token = parts.get(1).ok_or("timestamp response without token")?;
    let tst_info = tst_info_of(token.raw)?;
    let received = tst_nonce(&tst_info)?;
    let expected = u64::from_be_bytes(*nonce);
    if received != Some(expected) {
        return Err(format!(
            "Time stamping authority sent back bad nonce value. Expected {}, but got {}.",
            hex::encode(nonce),
            received
                .map(|r| format!("{r:#x}"))
                .unwrap_or_else(|| "None".into())
        ));
    }
    Ok(token.raw.to_vec())
}

fn fail_info_names(bits: &[u8]) -> Vec<String> {
    const NAMES: [(usize, &str); 8] = [
        (0, "bad_alg"),
        (2, "bad_request"),
        (5, "bad_data_format"),
        (14, "time_not_available"),
        (15, "unaccepted_policy"),
        (16, "unaccepted_extension"),
        (17, "add_info_not_available"),
        (25, "system_failure"),
    ];
    let b = bits.get(1..).unwrap_or_default();
    NAMES
        .iter()
        .filter(|(n, _)| {
            b.get(n / 8)
                .map(|x| x & (0x80 >> (n % 8)) != 0)
                .unwrap_or(false)
        })
        .map(|(_, s)| s.to_string())
        .collect()
}

/// The encapsulated TSTInfo DER of a timestamp token.
pub fn tst_info_of(token: &[u8]) -> CmsResult<Vec<u8>> {
    let sd = signed_data_of(token)?;
    let f = children(sd.content)?;
    let eci = f.get(2).ok_or("token: encapContentInfo")?;
    let e = children(eci.content)?;
    let content = e.get(1).ok_or("token: no content")?;
    let (oct, _) = read_tlv(content.content)?;
    Ok(oct.content.to_vec())
}

/// TSTInfo.nonce (u64 when it fits).
pub fn tst_nonce(tst_info: &[u8]) -> CmsResult<Option<u64>> {
    let (t, _) = read_tlv(tst_info)?;
    let f = children(t.content)?;
    // version, policy, messageImprint, serialNumber, genTime, [accuracy], [ordering], [nonce]
    for x in f.iter().skip(5) {
        if x.tag == 0x02 {
            let c: Vec<u8> = x.content.iter().copied().skip_while(|b| *b == 0).collect();
            if c.len() > 8 {
                return Ok(None);
            }
            return Ok(Some(c.iter().fold(0u64, |a, &b| (a << 8) | b as u64)));
        }
    }
    Ok(None)
}

/// The SignedData TLV inside a ContentInfo.
pub fn signed_data_of(content_info: &[u8]) -> CmsResult<Tlv<'_>> {
    let (ci, _) = read_tlv(content_info)?;
    let p = children(ci.content)?;
    if p.len() < 2 || oid_to_string(p[0].content) != OID_SIGNED_DATA {
        return Err("not a SignedData ContentInfo".into());
    }
    let (sd, _) = read_tlv(p[1].content)?;
    Ok(sd)
}

/// Certificates embedded in a SignedData (raw DER, in order).
pub fn signed_data_certs(content_info: &[u8]) -> CmsResult<Vec<Vec<u8>>> {
    let sd = signed_data_of(content_info)?;
    let mut out = Vec::new();
    for f in children(sd.content)? {
        if f.tag == 0xA0 {
            for c in children(f.content)? {
                if c.tag == 0x30 {
                    out.push(c.raw.to_vec());
                }
            }
        }
    }
    Ok(out)
}

/// POST a TimeStampReq the way HTTPTimeStamper does (5 s timeout; the reply's Content-Type must be exactly
/// `application/timestamp-reply`; the HTTP status is not checked, like `requests` without raise_for_status).
pub async fn request_timestamp(url: &str, message_digest: &[u8]) -> CmsResult<Vec<u8>> {
    let nonce = new_nonce();
    let req = timestamp_request(message_digest, &nonce);
    let resp = crate::http::client()
        .post(url)
        .header("Content-Type", "application/timestamp-query")
        .header("Accept", "application/timestamp-reply")
        .timeout(std::time::Duration::from_secs(5))
        .body(req)
        .send()
        .await
        .map_err(|_| "Error in communication with timestamp server".to_string())?;
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    if ct.as_deref() != Some("application/timestamp-reply") {
        return Err("Timestamp server response is malformed.".into());
    }
    let body = resp
        .bytes()
        .await
        .map_err(|_| "Error in communication with timestamp server".to_string())?;
    handle_tsp_response(&body, &nonce)
}

// ─── parsing a signature for validation ──────────────────────────────────────

pub struct ParsedSignerInfo {
    pub sid_issuer: Vec<u8>,
    pub sid_serial: Vec<u8>,
    pub digest_alg: String,
    pub signed_attrs_raw: Option<Vec<u8>>,
    pub sig_alg: String,
    pub signature: Vec<u8>,
    pub encap_content_type: String,
    pub certs: Vec<Vec<u8>>,
}

pub fn digest_name(oid_str: &str) -> Option<&'static str> {
    Some(match oid_str {
        OID_SHA256 => "sha256",
        OID_SHA1 => "sha1",
        OID_SHA384 => "sha384",
        OID_SHA512 => "sha512",
        _ => return None,
    })
}

pub fn parse_signer_info(content_info: &[u8]) -> CmsResult<ParsedSignerInfo> {
    let sd = signed_data_of(content_info)?;
    let f = children(sd.content)?;
    let eci = children(f.get(2).ok_or("SignedData: encapContentInfo")?.content)?;
    let encap_content_type = oid_to_string(eci[0].content);
    let certs = signed_data_certs(content_info)?;
    let sis = f.last().ok_or("SignedData: signerInfos")?;
    let si = children(sis.content)?
        .into_iter()
        .next()
        .ok_or("no signerInfo")?;
    let mut p = children(si.content)?.into_iter();
    let _version = p.next();
    let sid = p.next().ok_or("signerInfo: sid")?;
    let (sid_issuer, sid_serial) = if sid.tag == 0x30 {
        let s = children(sid.content)?;
        (s[0].raw.to_vec(), s[1].content.to_vec())
    } else {
        return Err("signerInfo: subjectKeyIdentifier sid not supported".into());
    };
    let dalg = p.next().ok_or("signerInfo: digestAlgorithm")?;
    let digest_alg = oid_to_string(children(dalg.content)?[0].content);
    let mut nxt = p.next().ok_or("signerInfo: truncated")?;
    let mut signed_attrs_raw = None;
    if nxt.tag == 0xA0 {
        let mut retagged = nxt.raw.to_vec();
        retagged[0] = 0x31;
        signed_attrs_raw = Some(retagged);
        nxt = p.next().ok_or("signerInfo: truncated")?;
    }
    let sig_alg = oid_to_string(children(nxt.content)?[0].content);
    let sig = p.next().ok_or("signerInfo: signature")?;
    Ok(ParsedSignerInfo {
        sid_issuer,
        sid_serial,
        digest_alg,
        signed_attrs_raw,
        sig_alg,
        signature: sig.content.to_vec(),
        encap_content_type,
        certs,
    })
}

/// Values of every attribute of `type_oid` in a (re-tagged) attribute SET.
pub fn find_attrs<'a>(attrs_set: &'a [u8], type_oid: &str) -> CmsResult<Vec<Vec<Tlv<'a>>>> {
    let (set, _) = read_tlv(attrs_set)?;
    let mut out = Vec::new();
    for a in children(set.content)? {
        let parts = children(a.content)?;
        if parts.len() == 2 && oid_to_string(parts[0].content) == type_oid {
            out.push(children(parts[1].content)?);
        }
    }
    Ok(out)
}
