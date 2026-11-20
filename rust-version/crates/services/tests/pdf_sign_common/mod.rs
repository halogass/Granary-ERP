//! Shared helpers for the pdf_sign tests: fixture paths, settings env, a wiremock RFC 3161 TSA (the
//! equivalent of pyHanko's DummyTimeStamper behind HTTP) and the normaliser that compares incremental
//! updates with Python's output (dates, IDs, signature bytes, offsets and uuids masked).
#![allow(dead_code)]

use std::path::PathBuf;

use regex::bytes::Regex;
use rsa::pkcs8::DecodePrivateKey;
use sha2::{Digest, Sha256};

pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../compat-tests/golden/pdf_sign")
}

pub fn read(rel: &str) -> Vec<u8> {
    std::fs::read(dir().join(rel)).unwrap()
}

pub fn cases() -> serde_json::Value {
    serde_json::from_slice(&read("cases.json")).unwrap()
}

/// Test env (same as the Python env.sh) plus the seal settings; must run before the first settings() use.
pub fn init_env(p12_path: &str, p12_password: &str, tsa_url: &str) {
    for (k, v) in [
        (
            "JWT_SECRET_KEY",
            "test-jwt-secret-key-at-least-32-characters-long",
        ),
        (
            "APP_SECRET_KEY",
            "test-app-secret-key-at-least-32-characters-long",
        ),
        (
            "SECRET_KEY",
            "test-app-secret-key-at-least-32-characters-long",
        ),
        ("POSTGRES_PASSWORD", "granarytest"),
        ("SUPERADMIN_PASSWORD", "test-superadmin-password-1234"),
        ("MINIO_SECRET_KEY", "test-minio-secret"),
        ("WHATSAPP_SERVICE_TOKEN", "test-wa-token"),
        (
            "DATABASE_URL",
            "postgresql://granary:granarytest@127.0.0.1:55433/unused",
        ),
        ("ENVIRONMENT", "test"),
    ] {
        if std::env::var_os(k).is_none() {
            std::env::set_var(k, v);
        }
    }
    std::env::set_var("SIGN_CERT_P12_PATH", p12_path);
    std::env::set_var("SIGN_CERT_P12_PASSWORD", p12_password);
    std::env::set_var("SIGN_TSA_URL", tsa_url);
}

/// The path of a seal this process generates on first use (the bootstrap self-signed seal, CN and O =
/// SIGN_SEAL_NAME): one per test process, so every test of a binary signs with the same certificate.
pub fn fresh_seal() -> String {
    let d = std::env::temp_dir().join(format!("pdf_sign_seal_{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join("seal.p12").to_string_lossy().into_owned()
}

/// A private copy of a fixture file in a temp dir (so nothing writes into the fixtures).
pub fn temp_copy(rel: &str) -> String {
    let d = std::env::temp_dir().join(format!(
        "pdf_sign_test_{}_{}",
        std::process::id(),
        rand::random::<u32>()
    ));
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join(std::path::Path::new(rel).file_name().unwrap());
    std::fs::write(&p, read(rel)).unwrap();
    p.to_string_lossy().into_owned()
}

// ─── minimal DER ─────────────────────────────────────────────────────────────

pub fn tlv(tag: u8, c: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    if c.len() < 0x80 {
        out.push(c.len() as u8);
    } else {
        let b: Vec<u8> = c
            .len()
            .to_be_bytes()
            .iter()
            .copied()
            .skip_while(|x| *x == 0)
            .collect();
        out.push(0x80 | b.len() as u8);
        out.extend(b);
    }
    out.extend_from_slice(c);
    out
}

pub fn seq(items: &[Vec<u8>]) -> Vec<u8> {
    tlv(0x30, &items.concat())
}

pub fn set_sorted(items: &[Vec<u8>]) -> Vec<u8> {
    let mut v = items.to_vec();
    v.sort();
    tlv(0x31, &v.concat())
}

pub fn oid(d: &str) -> Vec<u8> {
    let p: Vec<u64> = d.split('.').map(|x| x.parse().unwrap()).collect();
    let mut b = vec![(p[0] * 40 + p[1]) as u8];
    for &x in &p[2..] {
        let mut s = vec![(x & 0x7f) as u8];
        let mut v = x >> 7;
        while v > 0 {
            s.push(0x80 | (v & 0x7f) as u8);
            v >>= 7;
        }
        s.reverse();
        b.extend(s);
    }
    tlv(0x06, &b)
}

/// (tag, content, raw, rest)
pub fn read_tlv(d: &[u8]) -> (u8, &[u8], &[u8], &[u8]) {
    let (len, hl) = if d[1] < 0x80 {
        (d[1] as usize, 2)
    } else {
        let n = (d[1] & 0x7f) as usize;
        (
            d[2..2 + n]
                .iter()
                .fold(0usize, |a, &b| (a << 8) | b as usize),
            2 + n,
        )
    };
    (d[0], &d[hl..hl + len], &d[..hl + len], &d[hl + len..])
}

pub fn children(mut c: &[u8]) -> Vec<(u8, Vec<u8>, Vec<u8>)> {
    let mut out = Vec::new();
    while !c.is_empty() {
        let (t, content, raw, rest) = read_tlv(c);
        out.push((t, content.to_vec(), raw.to_vec()));
        c = rest;
    }
    out
}

// ─── the TSA ─────────────────────────────────────────────────────────────────

pub struct Tsa {
    key: rsa::RsaPrivateKey,
    cert: Vec<u8>,
    issuer: Vec<u8>,
    serial: Vec<u8>,
}

impl Tsa {
    pub fn load(prefix: &str) -> Tsa {
        let key = rsa::RsaPrivateKey::from_pkcs8_der(&read(&format!("{prefix}_key.der"))).unwrap();
        let cert = read(&format!("{prefix}_cert.der"));
        let (_, c, _, _) = read_tlv(&cert);
        let parts = children(c);
        let tbs = children(&parts[0].1);
        let off = if tbs[0].0 == 0xA0 { 1 } else { 0 };
        Tsa {
            key,
            issuer: tbs[off + 2].2.clone(),
            serial: tbs[off].2.clone(),
            cert,
        }
    }

    /// TimeStampResp for a TimeStampReq (granted; messageImprint and nonce echoed).
    pub fn respond(&self, req: &[u8]) -> Vec<u8> {
        let (_, c, _, _) = read_tlv(req);
        let f = children(c);
        let imprint = f[1].2.clone();
        let nonce = f.iter().skip(2).find(|x| x.0 == 0x02).map(|x| x.2.clone());
        let now = chrono::Utc::now().format("%Y%m%d%H%M%SZ").to_string();
        let serial: [u8; 8] = rand::random();
        let mut tst = vec![
            tlv(0x02, &[1]),
            oid("1.3.6.1.4.1.4146.2.2"),
            imprint,
            tlv(0x02, &[0x01, serial[1], serial[2]]),
            tlv(0x18, now.as_bytes()),
        ];
        if let Some(n) = nonce {
            tst.push(n);
        }
        let tst = seq(&tst);
        let sha256 = seq(&[oid("2.16.840.1.101.3.4.2.1"), vec![0x05, 0x00]]);
        let ess = seq(&[seq(&[seq(&[tlv(0x04, &Sha256::digest(&self.cert))])])]);
        let attrs = vec![
            seq(&[
                oid("1.2.840.113549.1.9.3"),
                set_sorted(&[oid("1.2.840.113549.1.9.16.1.4")]),
            ]),
            seq(&[
                oid("1.2.840.113549.1.9.4"),
                set_sorted(&[tlv(0x04, &Sha256::digest(&tst))]),
            ]),
            seq(&[oid("1.2.840.113549.1.9.16.2.47"), set_sorted(&[ess])]),
        ];
        let signed = set_sorted(&attrs);
        let sig = self
            .key
            .sign(rsa::Pkcs1v15Sign::new::<Sha256>(), &Sha256::digest(&signed))
            .unwrap();
        let (_, attrs_content, _, _) = read_tlv(&signed);
        let si = seq(&[
            tlv(0x02, &[1]),
            seq(&[self.issuer.clone(), self.serial.clone()]),
            sha256.clone(),
            tlv(0xA0, attrs_content),
            seq(&[oid("1.2.840.113549.1.1.11"), vec![0x05, 0x00]]),
            tlv(0x04, &sig),
        ]);
        let sd = seq(&[
            tlv(0x02, &[3]),
            set_sorted(&[sha256]),
            seq(&[
                oid("1.2.840.113549.1.9.16.1.4"),
                tlv(0xA0, &tlv(0x04, &tst)),
            ]),
            tlv(0xA0, &self.cert),
            tlv(0x31, &si),
        ]);
        let token = seq(&[oid("1.2.840.113549.1.7.2"), tlv(0xA0, &sd)]);
        seq(&[seq(&[tlv(0x02, &[0])]), token])
    }
}

pub struct TsaResponder(pub std::sync::Arc<Tsa>);

impl wiremock::Respond for TsaResponder {
    fn respond(&self, req: &wiremock::Request) -> wiremock::ResponseTemplate {
        wiremock::ResponseTemplate::new(200)
            .insert_header("Content-Type", "application/timestamp-reply")
            .set_body_bytes(self.0.respond(&req.body))
    }
}

pub async fn start_tsa(prefix: &str) -> wiremock::MockServer {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::header(
            "Content-Type",
            "application/timestamp-query",
        ))
        .respond_with(TsaResponder(std::sync::Arc::new(Tsa::load(prefix))))
        .mount(&server)
        .await;
    server
}

// ─── comparing incremental updates ───────────────────────────────────────────

/// The bytes appended to `input`, with every run-dependent value masked.
pub fn normalized_increment(signed: &[u8], input: &[u8]) -> String {
    assert!(
        signed.starts_with(input),
        "the original bytes are kept unchanged"
    );
    let mut inc = signed[input.len()..].to_vec();
    let subs: &[(&str, &[u8])] = &[
        (r"(?s-u)stream\n.*?\nendstream", b"stream<BIN>endstream"),
        (r"<[0-9A-Fa-f]{100,}>", b"<HEX>"),
        (r"<[0-9a-f]{32}> <[0-9a-f]{32}>", b"<ID>"),
        (r"\(D\\072[^)]*\)", b"(DATE)"),
        (r"\(Timestamp\\055[^)]*\)", b"(TIMESTAMP-UUID)"),
        (r"/[0-9A-F]{40} ", b"/VRIKEY "),
        (r"\[0 \d+ \d+ \d+\] *", b"[BYTERANGE]"),
        (r"\d{10} 00000 n", b"OFFSET n"),
        (r"startxref\n\d+", b"startxref N"),
        (r"/Prev \d+", b"/Prev N"),
        (r"/Length \d+", b"/Length N"),
    ];
    for (re, rep) in subs {
        inc = Regex::new(re).unwrap().replace_all(&inc, *rep).into_owned();
    }
    String::from_utf8_lossy(&inc).into_owned()
}

/// Proof runs: with PDF_SIGN_DUMP_DIR set, sealed outputs are written there for pyHanko's validator.
pub fn dump(name: &str, data: &[u8]) {
    if let Ok(d) = std::env::var("PDF_SIGN_DUMP_DIR") {
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(std::path::Path::new(&d).join(name), data).unwrap();
    }
}
