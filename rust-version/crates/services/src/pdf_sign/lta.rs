//! PAdES-B-LTA, as pyHanko does it with `embed_validation_info=True, use_pades_lta=True,
//! validation_context=ValidationContext(allow_fetching=True)`:
//!
//! 1. pre-sign validation: the signer's certificate must chain to a trust root (the system CA bundle, as
//!    oscrypto loads it) with keyUsage nonRepudiation; the TSA's certificate (taken from a dummy timestamp,
//!    which is also what sizes the signature) must chain to a trust root with EKU timeStamping. Revocation
//!    is soft-fail: OCSP from the AIA when declared, the CRL distribution point when OCSP did not answer
//!    "good"; whatever was fetched is embedded. A self-signed seal (the bootstrap certificate) fails here,
//!    so Python, and this port, fall back to B-T.
//! 2. after signing: one revision holding the DSS (certificates of every path, roots included, the
//!    revocation data and a VRI entry keyed by SHA-1 of the signature /Contents incl. padding) plus a
//!    document timestamp field `Timestamp-<uuid4>`; then one more revision adding the VRI entry of the
//!    document timestamp.

use std::collections::HashSet;

use sha2::{Digest, Sha256};

use super::cms::{self, Cert};
use super::p12::Signer;
use super::pdf::{Dict, IncrementalWriter, Obj, Reader};

pub struct LtaContext {
    /// Validation paths, each trust-root first (pyHanko `ValidationPath` iteration order).
    pub paths: Vec<Vec<Cert>>,
    pub ocsps: Vec<Vec<u8>>,
    pub crls: Vec<Vec<u8>>,
    /// The dummy timestamp token fetched during validation (reused to size the containers).
    pub dummy_tst: Vec<u8>,
}

static EXTRA_TRUST_ROOTS: once_cell::sync::Lazy<std::sync::Mutex<Vec<Vec<u8>>>> =
    once_cell::sync::Lazy::new(|| std::sync::Mutex::new(Vec::new()));

/// Test hook (the Python tests inject `extra_trust_roots` into ValidationContext the same way).
pub fn set_extra_trust_roots(roots: Vec<Vec<u8>>) {
    *EXTRA_TRUST_ROOTS.lock().unwrap_or_else(|e| e.into_inner()) = roots;
}

/// oscrypto's Linux trust list: SSL_CERT_FILE first, then the common bundle paths.
fn system_trust_roots() -> Vec<Cert> {
    let mut paths: Vec<String> = Vec::new();
    if let Ok(p) = std::env::var("SSL_CERT_FILE") {
        paths.push(p);
    }
    for p in [
        "/usr/lib/ssl/certs/ca-certificates.crt",
        "/etc/ssl/certs/ca-certificates.crt",
        "/etc/ssl/certs/ca-bundle.crt",
        "/etc/pki/tls/certs/ca-bundle.crt",
        "/etc/ssl/ca-bundle.pem",
        "/usr/local/share/certs/ca-root-nss.crt",
        "/etc/ssl/cert.pem",
    ] {
        paths.push(p.to_string());
    }
    let mut out = Vec::new();
    for p in paths {
        if let Ok(text) = std::fs::read_to_string(&p) {
            for der in pem_certs(&text) {
                if let Ok(c) = Cert::parse(&der) {
                    out.push(c);
                }
            }
            break;
        }
    }
    for der in EXTRA_TRUST_ROOTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
    {
        if let Ok(c) = Cert::parse(der) {
            out.push(c);
        }
    }
    out
}

fn pem_certs(text: &str) -> Vec<Vec<u8>> {
    use base64::Engine;
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    for line in text.lines() {
        let l = line.trim();
        if l == "-----BEGIN CERTIFICATE-----" {
            cur = Some(String::new());
        } else if l == "-----END CERTIFICATE-----" {
            if let Some(b) = cur.take() {
                if let Ok(der) = base64::engine::general_purpose::STANDARD.decode(b) {
                    out.push(der);
                }
            }
        } else if let Some(b) = cur.as_mut() {
            b.push_str(l);
        }
    }
    out
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

fn is_trusted(c: &Cert, roots: &[Cert]) -> bool {
    roots
        .iter()
        .any(|r| r.der == c.der || (r.subject == c.subject && r.spki == c.spki))
}

fn signed_by(c: &Cert, issuer: &Cert) -> bool {
    if c.issuer != issuer.subject {
        return false;
    }
    let digest = match c.sig_alg.as_str() {
        cms::OID_SHA384_RSA | cms::OID_ECDSA_SHA384 => "sha384",
        cms::OID_SHA512_RSA | cms::OID_ECDSA_SHA512 => "sha512",
        cms::OID_SHA1_RSA => "sha1",
        _ => "sha256",
    };
    cms::verify_signature(&issuer.spki, &c.sig_alg, digest, &c.tbs, &c.signature)
}

fn is_ca(c: &Cert) -> bool {
    let bc_ok = match c.ext("2.5.29.19") {
        Some((_, _, v)) => cms::read_tlv(v)
            .ok()
            .and_then(|(t, _)| cms::children(t.content).ok())
            .map(|f| {
                f.first()
                    .map(|x| x.tag == 0x01 && x.content.first().copied().unwrap_or(0) != 0)
                    .unwrap_or(false)
            })
            .unwrap_or(false),
        None => false,
    };
    // keyCertSign (bit 5) when keyUsage is present
    bc_ok && c.key_usage_bit(5).unwrap_or(true)
}

/// Build and check a path for `leaf` (pyhanko-certvalidator's checks that matter here: signatures,
/// validity at the current time, CA basic constraints / keyCertSign on issuers).
fn build_path(leaf: &Cert, intermediates: &[Cert], roots: &[Cert]) -> Result<Vec<Cert>, String> {
    fn rec(
        cur: &Cert,
        inter: &[Cert],
        roots: &[Cert],
        depth: usize,
        seen: &mut HashSet<Vec<u8>>,
    ) -> Option<Vec<Cert>> {
        if is_trusted(cur, roots) {
            return Some(vec![cur.clone()]);
        }
        if depth > 10 || !seen.insert(cur.der.clone()) {
            return None;
        }
        for cand in roots.iter().chain(inter.iter()) {
            if cand.der == cur.der || !signed_by(cur, cand) || !is_ca(cand) {
                continue;
            }
            if let Some(mut p) = rec(cand, inter, roots, depth + 1, seen) {
                p.push(cur.clone());
                return Some(p);
            }
        }
        None
    }
    let mut seen = HashSet::new();
    let path = match rec(leaf, intermediates, roots, 0, &mut seen) {
        Some(p) => p,
        None => {
            if leaf.self_issued() {
                return Err("The X.509 certificate provided is self-signed".into());
            }
            return Err("Unable to build a validation path for the certificate".into());
        }
    };
    let now = now_ts();
    for c in path.iter().skip(1) {
        if now < c.not_before || now > c.not_after {
            return Err("The path could not be validated because a certificate is not valid at the validation time".into());
        }
    }
    Ok(path)
}

/// URLs from the authorityInfoAccess extension for an access method (OCSP = .48.1, caIssuers = .48.2).
fn aia_urls(c: &Cert, method: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Some((_, _, v)) = c.ext("1.3.6.1.5.5.7.1.1") else {
        return out;
    };
    let Ok((seq, _)) = cms::read_tlv(v) else {
        return out;
    };
    for ad in cms::children(seq.content).unwrap_or_default() {
        let f = cms::children(ad.content).unwrap_or_default();
        if f.len() == 2 && cms::oid_to_string(f[0].content) == method && f[1].tag == 0x86 {
            out.push(String::from_utf8_lossy(f[1].content).into_owned());
        }
    }
    out
}

fn crl_urls(c: &Cert) -> Vec<String> {
    let mut out = Vec::new();
    let Some((_, _, v)) = c.ext("2.5.29.31") else {
        return out;
    };
    fn walk(data: &[u8], out: &mut Vec<String>) {
        let mut rest = data;
        while let Ok((t, r)) = cms::read_tlv(rest) {
            if t.tag == 0x86 {
                out.push(String::from_utf8_lossy(t.content).into_owned());
            } else if t.tag & 0x20 != 0 {
                walk(t.content, out);
            }
            rest = r;
            if rest.is_empty() {
                break;
            }
        }
    }
    walk(v, &mut out);
    out
}

fn ocsp_request(cert: &Cert, issuer: &Cert) -> Vec<u8> {
    let issuer_key_bits = cms::read_tlv(&issuer.spki)
        .ok()
        .and_then(|(t, _)| cms::children(t.content).ok())
        .and_then(|f| {
            f.get(1)
                .map(|b| b.content.get(1..).unwrap_or_default().to_vec())
        })
        .unwrap_or_default();
    use sha1::Digest as _;
    let cert_id = cms::seq(&[
        cms::digest_alg_id("sha1"),
        cms::octet(&sha1::Sha1::digest(&issuer.subject)),
        cms::octet(&sha1::Sha1::digest(&issuer_key_bits)),
        cert.serial_tlv.clone(),
    ]);
    let nonce: [u8; 16] = rand::random();
    let ext = cms::seq(&[cms::seq(&[
        cms::oid("1.3.6.1.5.5.7.48.1.2"),
        cms::octet(&cms::octet(&nonce)),
    ])]);
    let tbs = cms::seq(&[cms::seq(&[cms::seq(&[cert_id])]), cms::explicit(2, &ext)]);
    cms::seq(&[tbs])
}

/// OCSPResponse status (0 = successful) and the first SingleResponse certStatus tag.
fn ocsp_status(resp: &[u8]) -> Option<(i64, Option<u8>)> {
    let (top, _) = cms::read_tlv(resp).ok()?;
    let f = cms::children(top.content).ok()?;
    let status = f
        .first()?
        .content
        .iter()
        .fold(0i64, |a, &b| (a << 8) | b as i64);
    let cert_status = (|| {
        let rb = f.get(1)?;
        let (rbs, _) = cms::read_tlv(rb.content).ok()?;
        let parts = cms::children(rbs.content).ok()?;
        let (basic, _) = cms::read_tlv(parts.get(1)?.content).ok()?;
        let bparts = cms::children(basic.content).ok()?;
        let tbs = cms::children(bparts.first()?.content).ok()?;
        let responses = tbs.iter().find(|t| {
            t.tag == 0x30
                && cms::children(t.content)
                    .ok()
                    .map(|c| c.first().map(|x| x.tag == 0x30).unwrap_or(false))
                    .unwrap_or(false)
        })?;
        let single = cms::children(responses.content).ok()?.into_iter().next()?;
        let sf = cms::children(single.content).ok()?;
        Some(sf.get(1)?.tag)
    })();
    Some((status, cert_status))
}

/// Soft-fail revocation fetching for every non-root certificate of a path.
async fn fetch_revinfo(
    path: &[Cert],
    ocsps: &mut Vec<Vec<u8>>,
    crls: &mut Vec<Vec<u8>>,
) -> Result<(), String> {
    for i in 1..path.len() {
        let (cert, issuer) = (&path[i], &path[i - 1]);
        let mut good = false;
        for url in aia_urls(cert, "1.3.6.1.5.5.7.48.1") {
            let req = ocsp_request(cert, issuer);
            let resp = crate::http::client()
                .post(&url)
                .header("Content-Type", "application/ocsp-request")
                .header("Accept", "application/ocsp-response")
                .timeout(std::time::Duration::from_secs(10))
                .body(req)
                .send()
                .await;
            let Ok(resp) = resp else { continue };
            let Ok(body) = resp.bytes().await else {
                continue;
            };
            match ocsp_status(&body) {
                Some((0, Some(0x80))) => {
                    good = true;
                    ocsps.push(body.to_vec());
                    break;
                }
                Some((0, Some(0xA1))) => {
                    return Err(
                        "The path could not be validated because the certificate is revoked".into(),
                    )
                }
                Some((0, _)) => ocsps.push(body.to_vec()),
                _ => {}
            }
        }
        if good {
            continue;
        }
        for url in crl_urls(cert) {
            let resp = crate::http::client()
                .get(&url)
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await;
            let Ok(resp) = resp else { continue };
            let Ok(body) = resp.bytes().await else {
                continue;
            };
            if cms::read_tlv(&body)
                .map(|(t, _)| t.tag == 0x30)
                .unwrap_or(false)
            {
                crls.push(body.to_vec());
                break;
            }
        }
    }
    Ok(())
}

/// `perform_presign_validation` for B-LTA.
pub async fn presign_validation(
    signer: &Signer,
    tsa_url: Option<&str>,
) -> Result<LtaContext, String> {
    let roots = system_trust_roots();
    let signer_path = build_path(&signer.cert, &signer.other_certs, &roots)
        .map_err(|e| format!("The signer's certificate could not be validated: {e}"))?;
    // signer_key_usage default {"non_repudiation"}
    if signer.cert.key_usage_bit(1) != Some(true) {
        return Err(
            "The signer's certificate could not be validated: key usage non_repudiation required"
                .into(),
        );
    }
    let mut ocsps = Vec::new();
    let mut crls = Vec::new();
    fetch_revinfo(&signer_path, &mut ocsps, &mut crls).await?;
    let url = tsa_url.ok_or("no timestamper")?;
    let dummy = cms::request_timestamp(url, &Sha256::digest(b"")).await?;
    // TSA certificate(s): the token's signer, validated for EKU timeStamping
    let token_certs: Vec<Cert> = cms::signed_data_certs(&dummy)?
        .iter()
        .filter_map(|d| Cert::parse(d).ok())
        .collect();
    let si = cms::parse_signer_info(&dummy)?;
    let tsa = token_certs
        .iter()
        .find(|c| c.issuer == si.sid_issuer && c.serial == si.sid_serial)
        .ok_or("timestamp token without its TSA certificate")?;
    let tsa_path = build_path(tsa, &token_certs, &roots)?;
    if !has_eku(tsa, "1.3.6.1.5.5.7.3.8") {
        return Err("The TSA certificate is not valid for the purpose of time_stamping".into());
    }
    fetch_revinfo(&tsa_path, &mut ocsps, &mut crls).await?;
    Ok(LtaContext {
        paths: vec![signer_path, tsa_path],
        ocsps,
        crls,
        dummy_tst: dummy,
    })
}

fn has_eku(c: &Cert, eku: &str) -> bool {
    let Some((_, _, v)) = c.ext("2.5.29.37") else {
        return false;
    };
    let Ok((s, _)) = cms::read_tlv(v) else {
        return false;
    };
    cms::children(s.content)
        .unwrap_or_default()
        .iter()
        .any(|o| cms::oid_to_string(o.content) == eku)
}

// ─── DSS (pyhanko.sign.validation.dss.DocumentSecurityStore) ─────────────────

struct Dss {
    /// (issuer DER + serial) -> ref, insertion ordered.
    certs: Vec<(Vec<u8>, (u32, u32))>,
    ocsps: Vec<(Vec<u8>, (u32, u32))>,
    crls: Vec<(Vec<u8>, (u32, u32))>,
    vri: Vec<(Vec<u8>, Obj)>,
    backing: Dict,
    backing_ref: Option<(u32, u32)>,
    created: bool,
    modified: bool,
}

fn stream_data(reader: &Reader, o: &Obj) -> Result<Vec<u8>, String> {
    match reader.resolve(o)? {
        Obj::Stream(d, raw) => super::pdf::decode_stream(&d, &raw),
        _ => Err("DSS entry is not a stream".into()),
    }
}

fn read_dss(w: &IncrementalWriter) -> Result<Dss, String> {
    let reader = w.reader;
    let dss_obj = w.root_obj.as_dict().and_then(|d| d.get("DSS")).cloned();
    let Some(dss_obj) = dss_obj else {
        return Ok(Dss {
            certs: vec![],
            ocsps: vec![],
            crls: vec![],
            vri: vec![],
            backing: Dict::new(),
            backing_ref: None,
            created: true,
            modified: false,
        });
    };
    let backing_ref = match dss_obj {
        Obj::Ref(n, g) => Some((n, g)),
        _ => None,
    };
    let backing = reader
        .resolve(&dss_obj)?
        .as_dict()
        .cloned()
        .ok_or("DSS is not a dictionary")?;
    let refs = |key: &str| -> Result<Vec<(Vec<u8>, (u32, u32))>, String> {
        let mut out = Vec::new();
        if let Some(a) = backing.get(key) {
            for r in reader.resolve(a)?.as_array().cloned().unwrap_or_default() {
                if let Obj::Ref(n, g) = r {
                    out.push((stream_data(reader, &Obj::Ref(n, g))?, (n, g)));
                }
            }
        }
        Ok(out)
    };
    let mut certs = Vec::new();
    for (der, r) in refs("Certs")? {
        let c = Cert::parse(&der)?;
        certs.push(([c.issuer, c.serial].concat(), r));
    }
    let vri = match backing.get("VRI") {
        Some(v) => reader
            .resolve(v)?
            .as_dict()
            .map(|d| d.0.clone())
            .unwrap_or_default(),
        None => vec![],
    };
    Ok(Dss {
        certs,
        ocsps: refs("OCSPs")?,
        crls: refs("CRLs")?,
        vri,
        backing,
        backing_ref,
        created: false,
        modified: false,
    })
}

/// `supply_dss_in_writer(pdf_out, sig_contents, paths=..., ocsps=..., crls=..., embed_roots=True)`.
pub fn supply_dss_in_writer(
    w: &mut IncrementalWriter,
    sig_contents: Option<&[u8]>,
    ctx: &LtaContext,
) -> Result<bool, String> {
    let mut dss = read_dss(w)?;
    let mut cert_refs: Vec<(u32, u32)> = Vec::new();
    let embed = |w: &mut IncrementalWriter, dss: &mut Dss, c: &Cert| -> (u32, u32) {
        let key = [c.issuer.clone(), c.serial.clone()].concat();
        if let Some((_, r)) = dss.certs.iter().find(|(k, _)| *k == key) {
            return *r;
        }
        let r = w.add_object(Obj::Stream(Dict::new(), c.der.clone()));
        dss.modified = true;
        dss.certs.push((key, r));
        r
    };
    for path in &ctx.paths {
        for c in path {
            let r = embed(w, &mut dss, c);
            if !cert_refs.contains(&r) {
                cert_refs.push(r);
            }
        }
    }
    let to_streams = |w: &mut IncrementalWriter,
                      items: &[Vec<u8>],
                      seen: &mut Vec<(Vec<u8>, (u32, u32))>,
                      modified: &mut bool|
     -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for it in items {
            let r = match seen.iter().find(|(b, _)| b == it) {
                Some((_, r)) => *r,
                None => {
                    let r = w.add_object(Obj::Stream(Dict::new(), it.clone()));
                    *modified = true;
                    seen.push((it.clone(), r));
                    r
                }
            };
            if !out.contains(&r) {
                out.push(r);
            }
        }
        out
    };
    let mut modified = dss.modified;
    let ocsp_refs = to_streams(w, &ctx.ocsps, &mut dss.ocsps, &mut modified);
    let crl_refs = to_streams(w, &ctx.crls, &mut dss.crls, &mut modified);
    dss.modified = modified;
    if let Some(contents) = sig_contents {
        use sha1::Digest as _;
        let ident = hex::encode_upper(sha1::Sha1::digest(contents));
        let mut vri = Dict::new();
        vri.set("Type", Obj::name("VRI"));
        let sorted = |mut v: Vec<(u32, u32)>| {
            v.sort();
            Obj::Array(v.into_iter().map(|(n, g)| Obj::Ref(n, g)).collect())
        };
        if !ocsp_refs.is_empty() {
            vri.set("OCSP", sorted(ocsp_refs));
        }
        if !crl_refs.is_empty() {
            vri.set("CRL", sorted(crl_refs));
        }
        vri.set("Cert", sorted(cert_refs));
        let r = w.add_object(Obj::Dict(vri));
        match dss.vri.iter_mut().find(|(k, _)| *k == ident.as_bytes()) {
            Some(slot) => slot.1 = Obj::Ref(r.0, r.1),
            None => dss.vri.push((ident.into_bytes(), Obj::Ref(r.0, r.1))),
        }
        dss.modified = true;
    }
    // as_pdf_object
    let mut d = dss.backing.clone();
    d.set(
        "Certs",
        Obj::Array(
            dss.certs
                .iter()
                .map(|(_, (n, g))| Obj::Ref(*n, *g))
                .collect(),
        ),
    );
    if !dss.vri.is_empty() {
        d.set("VRI", Obj::Dict(Dict(dss.vri.clone())));
    }
    if !dss.ocsps.is_empty() {
        d.set(
            "OCSPs",
            Obj::Array(
                dss.ocsps
                    .iter()
                    .map(|(_, (n, g))| Obj::Ref(*n, *g))
                    .collect(),
            ),
        );
    }
    if !dss.crls.is_empty() {
        d.set(
            "CRLs",
            Obj::Array(
                dss.crls
                    .iter()
                    .map(|(_, (n, g))| Obj::Ref(*n, *g))
                    .collect(),
            ),
        );
    }
    if dss.created {
        let r = w.add_object(Obj::Dict(d));
        w.root_obj
            .as_dict_mut()
            .ok_or("catalog is not a dictionary")?
            .set("DSS", Obj::Ref(r.0, r.1));
    } else if dss.modified {
        match dss.backing_ref {
            Some(r) => w.mark_update(r, Obj::Dict(d)),
            None => w
                .root_obj
                .as_dict_mut()
                .ok_or("catalog is not a dictionary")?
                .set("DSS", Obj::Dict(d)),
        }
    }
    Ok(dss.modified)
}

/// `PdfPostSignatureDocument.post_signature_processing` for B-LTA: DSS + document timestamp in one
/// revision, then the DSS update carrying the timestamp's VRI.
pub async fn post_signature_processing(
    signed: Vec<u8>,
    sig_contents: &[u8],
    ctx: LtaContext,
    url: &str,
) -> Result<Vec<u8>, String> {
    let now = chrono::Local::now();
    let now_pdf = super::pdf::pdf_date(&now);
    let field_name = format!("Timestamp-{}", uuid::Uuid::new_v4());
    let bytes_reserved = super::reserve_for(ctx.dummy_tst.len());
    let mut sig = Dict::new();
    sig.set("Contents", Obj::ContentsPlaceholder(bytes_reserved));
    sig.set("ByteRange", Obj::ByteRangePlaceholder);
    sig.set("Type", Obj::name("DocTimeStamp"));
    sig.set("Filter", Obj::name("Adobe.PPKLite"));
    sig.set("SubFilter", Obj::name("ETSI.RFC3161"));
    let mut prepared = {
        let reader = Reader::new(&signed, false)?;
        let mut hook = |w: &mut IncrementalWriter| {
            supply_dss_in_writer(w, Some(sig_contents), &ctx).map(|_| ())
        };
        super::prepare_document(
            &reader,
            &mut hook,
            sig,
            None,
            &field_name,
            Some("Timestamp"),
            &now_pdf,
        )?
    };
    let digest = super::byte_range_digest(&prepared);
    let token = cms::request_timestamp(url, &digest).await?;
    super::fill_contents(&mut prepared, &token)?;
    let ts_contents = super::padded_contents(&token, prepared.bytes_reserved);
    let out = prepared.bytes;

    // add_dss(..., sig_contents=<timestamp contents>, paths=<signer + TSA + TSA>, force_write=True)
    let now_pdf = super::pdf::pdf_date(&chrono::Local::now());
    let reader = Reader::new(&out, false)?;
    let mut w = IncrementalWriter::new(&reader)?;
    let mut ctx2 = LtaContext {
        paths: ctx.paths.clone(),
        ocsps: ctx.ocsps.clone(),
        crls: ctx.crls.clone(),
        dummy_tst: vec![],
    };
    if let Some(tsa_path) = ctx.paths.get(1) {
        ctx2.paths.push(tsa_path.clone());
    }
    supply_dss_in_writer(&mut w, Some(&ts_contents), &ctx2)?;
    let written = w.write(&now_pdf)?;
    Ok(written.bytes)
}
