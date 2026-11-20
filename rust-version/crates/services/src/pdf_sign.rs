//! Port of backend/app/services/pdf_sign.py: PAdES platform seal, cryptographically signs completed PDFs.
//!
//! A single organisational certificate seals each document (tamper-evident). Self-managed but
//! CA-ready: drop a CA-issued .p12 at SIGN_CERT_P12_PATH and it is used instead of the
//! auto-generated self-signed certificate.
//!
//! Python did this with pyHanko; this module reproduces pyHanko's behaviour for the configuration the
//! backend uses (field `GranarySeal`, invisible, subfilter ETSI.CAdES.detached, certification signature with
//! DocMDP NO_CHANGES, SHA-256, B-B / B-T / B-LTA): same incremental-update layout and object numbering,
//! same CMS structure (content-type, message-digest, signing-certificate-v2 signed attributes; signature
//! timestamp token unsigned attribute), same size reservation, same TSA protocol. Submodules:
//! `pdf` (pyhanko.pdf_utils), `cms` (asn1crypto/CMS/RFC 3161), `p12` (PKCS#12), `lta` (B-LTA).

mod cms;
mod lta;
mod p12;
mod pdf;

use std::sync::{Arc, Mutex};

use once_cell::sync::Lazy;
use serde::Serialize;
use sha2::{Digest, Sha256};

use pdf::{Dict, IncrementalWriter, Obj, PdfStr, Reader};

/// pyHanko's metadata vendor string, kept byte-identical in /Producer (output parity with Python).
pub(crate) const PRODUCER: &str = "pyHanko 0.34.1";

const FIELD_NAME: &str = "GranarySeal";

/// `validate_pdf` result: the Python dict `{"intact", "valid", "signer", "sealed_at"}` (same key order).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SealReport {
    pub intact: Option<bool>,
    pub valid: Option<bool>,
    pub signer: Option<String>,
    pub sealed_at: Option<String>,
}

static SIGNER: Lazy<Mutex<Option<Arc<p12::Signer>>>> = Lazy::new(|| Mutex::new(None));

/// `_get_signer()`: cached per process; generates the self-signed .p12 on first use when missing.
fn get_signer() -> Result<Arc<p12::Signer>, String> {
    let mut guard = SIGNER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = guard.as_ref() {
        return Ok(s.clone());
    }
    let cfg = granary_core::config::settings();
    let path = cfg.SIGN_CERT_P12_PATH.clone();
    if !std::path::Path::new(&path).exists() {
        p12::generate_self_signed_p12(&path, &cfg.SIGN_SEAL_NAME)?;
    }
    let passphrase = if cfg.SIGN_CERT_P12_PASSWORD.is_empty() {
        None
    } else {
        Some(cfg.SIGN_CERT_P12_PASSWORD.as_str())
    };
    let signer = Arc::new(p12::load_pkcs12(&path, passphrase)?);
    *guard = Some(signer.clone());
    Ok(signer)
}

/// Test hook: extra trust roots for B-LTA path validation (DER), like the Python tests' injected
/// `extra_trust_roots`; production uses the system CA bundle only.
#[doc(hidden)]
pub fn _set_extra_trust_roots(roots: Vec<Vec<u8>>) {
    lta::set_extra_trust_roots(roots);
}

/// Test hook: forget the cached signer (Python tests reset `pdf_sign._signer = None`).
#[doc(hidden)]
pub fn _reset_signer_cache() {
    *SIGNER.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// SHA-256 fingerprint (hex, colon-less) of the signing certificate.
pub fn cert_fingerprint() -> Option<String> {
    let signer = get_signer().ok()?;
    Some(hex::encode(Sha256::digest(&signer.cert.der)))
}

/// The seal level a new seal would target given current config.
pub fn configured_seal_level() -> &'static str {
    if granary_core::config::settings().SIGN_TSA_URL.is_empty() {
        "pades-b-b"
    } else {
        "pades-b-lta"
    }
}

/// True only when an EU-qualified setup is explicitly configured (admin asserts a qualified
/// cert / QTSP is installed). Gates whether a document may claim eIDAS QES.
pub fn is_qualified() -> bool {
    granary_core::config::settings().SIGN_QES_ENABLED
}

/// The DocMDP permission a certification seal grants (`/TransformParams /P`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocMdp {
    /// P=1: no change at all (the e-sign final seal).
    NoChanges,
    /// P=2: form filling and further signatures allowed (the outbound copy: a customer can still sign
    /// it digitally without breaking the company's seal).
    FillAndSign,
}

impl DocMdp {
    fn p(self) -> i64 {
        match self {
            DocMdp::NoChanges => 1,
            DocMdp::FillAndSign => 2,
        }
    }
}

/// Apply a PAdES certification seal (DocMDP NO_CHANGES). Returns (signed_bytes, seal_level).
/// Targets PAdES-B-LTA when a TSA is configured, degrading gracefully to B-T then B-B.
pub async fn sign_pdf(pdf_bytes: &[u8]) -> anyhow::Result<(Vec<u8>, String)> {
    sign_pdf_with(pdf_bytes, DocMdp::NoChanges).await
}

/// `sign_pdf` with the DocMDP permission given: same field (`GranarySeal`), certificate and level fallback.
pub async fn sign_pdf_with(pdf_bytes: &[u8], mdp: DocMdp) -> anyhow::Result<(Vec<u8>, String)> {
    let tsa = granary_core::config::settings().SIGN_TSA_URL.clone();
    if !tsa.is_empty() {
        match _do_sign(pdf_bytes, true, true, mdp).await {
            Ok(out) => return Ok((out, "pades-b-lta".into())),
            Err(e) => {
                tracing::warn!(error = %e, "PAdES-B-LTA seal failed; trying timestamp-only (B-T)");
                match _do_sign(pdf_bytes, false, true, mdp).await {
                    Ok(out) => return Ok((out, "pades-b-t".into())),
                    Err(e) => {
                        tracing::warn!(error = %e, "Timestamped seal failed; falling back to B-B")
                    }
                }
            }
        }
    }
    let out = _do_sign(pdf_bytes, false, false, mdp)
        .await
        .map_err(anyhow::Error::msg)?;
    Ok((out, "pades-b-b".into()))
}

/// The DocMDP permission (`/Perms /DocMDP` -> `/Reference[0] /TransformParams /P`) of a certified PDF, or
/// None when it carries no certification seal.
pub fn docmdp_permission(pdf_bytes: &[u8]) -> Option<i64> {
    let reader = Reader::new(pdf_bytes, false).ok()?;
    let (rn, _) = reader.root_ref().ok()?;
    let root = reader.get(rn).ok()?;
    let perms = reader.resolve(root.as_dict()?.get("Perms")?).ok()?;
    let sig = reader.resolve(perms.as_dict()?.get("DocMDP")?).ok()?;
    let refs = reader.resolve(sig.as_dict()?.get("Reference")?).ok()?;
    let sr = reader.resolve(refs.as_array()?.first()?).ok()?;
    let tp = reader.resolve(sr.as_dict()?.get("TransformParams")?).ok()?;
    reader.resolve(tp.as_dict()?.get("P")?).ok()?.as_int()
}

/// Test hook: append an approval (non-certification) signature in a new field, as a customer signing a
/// received copy in Acrobat would. No timestamp.
#[doc(hidden)]
pub async fn _append_approval_signature(
    pdf_bytes: &[u8],
    field_name: &str,
) -> Result<Vec<u8>, String> {
    let signer = get_signer()?;
    let now_pdf = pdf::pdf_date(&chrono::Local::now());
    let empty_digest = Sha256::digest(b"").to_vec();
    let (test_der, _) = cms::signed_data(
        &signer.key,
        &signer.cert,
        &signer.other_certs,
        &empty_digest,
        None,
    )?;
    let bytes_reserved = reserve_for(test_der.len());
    let mut prepared = {
        let reader = Reader::new(pdf_bytes, false)?;
        prepare_document(
            &reader,
            &mut |_| Ok(()),
            signature_dict(bytes_reserved, &now_pdf),
            None,
            field_name,
            None,
            &now_pdf,
        )?
    };
    let digest = byte_range_digest(&prepared);
    let attrs = cms::pades_signed_attrs(&digest, &signer.cert);
    let signed_set = cms::set_sorted(&attrs);
    let signature = signer.key.sign_sha256(&signed_set)?;
    let der = cms::assemble_signed_data(
        &signer.key,
        &signer.cert,
        &signer.other_certs,
        &signed_set,
        &signature,
        None,
    );
    fill_contents(&mut prepared, &der)?;
    Ok(prepared.bytes)
}

/// The pre-signing document state: the incremental update written with placeholders.
struct Prepared {
    bytes: Vec<u8>,
    contents: (usize, usize),
    bytes_reserved: usize,
}

/// Fill the `/ByteRange` placeholder and return the document digest.
fn fill_byte_range(written: &mut pdf::Written) -> Result<Prepared, String> {
    let (_, cs, ce) = *written
        .placeholders
        .iter()
        .find(|p| p.0 == "contents")
        .ok_or("no contents placeholder")?;
    let (_, bs, _) = *written
        .placeholders
        .iter()
        .find(|p| p.0 == "byterange")
        .ok_or("no byterange placeholder")?;
    let eof = written.bytes.len();
    let br = format!("[0 {} {} {}]", cs, ce, eof - ce);
    written.bytes[bs..bs + br.len()].copy_from_slice(br.as_bytes());
    Ok(Prepared {
        bytes: std::mem::take(&mut written.bytes),
        contents: (cs, ce),
        bytes_reserved: ce - cs - 2,
    })
}

fn byte_range_digest(p: &Prepared) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(&p.bytes[..p.contents.0]);
    h.update(&p.bytes[p.contents.1..]);
    h.finalize().to_vec()
}

/// `PreparedByteRangeDigest.fill_reserved_region`: upper-case hex, zero padding left in place.
fn fill_contents(p: &mut Prepared, der: &[u8]) -> Result<(), String> {
    let hexd = hex::encode_upper(der);
    if hexd.len() > p.bytes_reserved {
        return Err(format!(
            "Final ByteRange payload larger than expected: allocated {} bytes, but contents required {} bytes.",
            p.bytes_reserved,
            hexd.len()
        ));
    }
    let start = p.contents.0 + 1;
    p.bytes[start..start + hexd.len()].copy_from_slice(hexd.as_bytes());
    Ok(())
}

/// The signature contents as written, zero padding included (`fill_with_cms` return value).
fn padded_contents(der: &[u8], bytes_reserved: usize) -> Vec<u8> {
    let mut v = der.to_vec();
    v.resize((bytes_reserved / 2).max(der.len()), 0);
    v
}

/// `bytes_reserved = test_len + 2 * (test_len // 4)` with `test_len = 2 * len(dummy CMS)`.
fn reserve_for(test_der_len: usize) -> usize {
    let test_len = test_der_len * 2;
    test_len + 2 * (test_len / 4)
}

/// Fully qualified names of signature fields under `fields` (pyHanko `enumerate_fields_in`, `/Sig` only):
/// (name, field value present, field ref).
fn enumerate_sig_fields(
    get: &dyn Fn((u32, u32)) -> Result<Obj, String>,
    fields: &[Obj],
    parent_name: Option<String>,
    parent_ft: Option<Vec<u8>>,
    seen: &mut std::collections::HashSet<(u32, u32)>,
    out: &mut Vec<(String, Option<Obj>, (u32, u32))>,
) -> Result<(), String> {
    for f in fields {
        let Obj::Ref(n, g) = f else { continue };
        if !seen.insert((*n, *g)) {
            continue;
        }
        let obj = get((*n, *g))?;
        let Some(d) = obj.as_dict() else { continue };
        let partial = match d.get("T") {
            Some(Obj::Str(PdfStr::Text(t))) => Some(t.clone()),
            Some(Obj::Str(PdfStr::Bytes(b))) => Some(String::from_utf8_lossy(b).into_owned()),
            _ => None,
        };
        let name = match (&parent_name, &partial) {
            (Some(p), Some(t)) => Some(format!("{p}.{t}")),
            (None, Some(t)) => Some(t.clone()),
            (p, None) => p.clone(),
        };
        let ft = d
            .get("FT")
            .and_then(|o| o.as_name())
            .map(|n| n.to_vec())
            .or_else(|| parent_ft.clone());
        let kids = match d.get("Kids") {
            Some(Obj::Ref(kn, kg)) => get((*kn, *kg))?.as_array().cloned(),
            Some(Obj::Array(a)) => Some(a.clone()),
            _ => None,
        };
        // a field with widget-only kids (no /T on the kids) is a terminal field
        let terminal = match &kids {
            None => true,
            Some(k) => k.iter().all(|kid| match kid {
                Obj::Ref(kn, kg) => get((*kn, *kg))
                    .ok()
                    .and_then(|o| o.as_dict().map(|d| !d.contains("T")))
                    .unwrap_or(true),
                _ => true,
            }),
        };
        if terminal {
            if ft.as_deref() == Some(b"Sig") {
                if let Some(nm) = name.clone() {
                    let v = d.get("V").cloned().filter(|v| !matches!(v, Obj::Null));
                    out.push((nm, v, (*n, *g)));
                }
            }
        } else if let Some(k) = kids {
            enumerate_sig_fields(get, &k, name, ft, seen, out)?;
        }
    }
    Ok(())
}

/// Where a modified object lives (pyHanko `update_container`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Container {
    Root,
    Indirect(u32, u32),
}

/// The part of `_do_sign` that sets up fields, DocMDP and the signature dictionary, then writes the
/// incremental update with placeholders.
fn prepare_document(
    reader: &Reader,
    hook: &mut dyn FnMut(&mut IncrementalWriter) -> Result<(), String>,
    sig_dict: Dict,
    certify: Option<DocMdp>,
    field_name: &str,
    readable_name: Option<&str>,
    now_pdf: &str,
) -> Result<Prepared, String> {
    let mut w = IncrementalWriter::new(reader)?;
    hook(&mut w)?;

    // ── prepare_sig_field ──
    let acro = w
        .root_obj
        .as_dict()
        .and_then(|d| d.get("AcroForm"))
        .cloned();
    let mut form_created = false;
    let (mut form, form_container): (Dict, Container) = match acro {
        Some(Obj::Ref(n, g)) => (
            w.get((n, g))?
                .as_dict()
                .cloned()
                .ok_or("AcroForm is not a dictionary")?,
            Container::Indirect(n, g),
        ),
        Some(Obj::Dict(d)) => (d, Container::Root),
        _ => {
            form_created = true;
            (Dict::new(), Container::Root)
        }
    };
    let form_ref = if form_created {
        let r = w.add_object(Obj::Dict(Dict::new()));
        w.root_obj
            .as_dict_mut()
            .ok_or("catalog is not a dictionary")?
            .set("AcroForm", Obj::Ref(r.0, r.1));
        Some(r)
    } else if let Container::Indirect(n, g) = form_container {
        Some((n, g))
    } else {
        None
    };
    let (mut fields, fields_container): (Vec<Obj>, Container) = match form.get("Fields") {
        Some(Obj::Ref(n, g)) => (
            w.get((*n, *g))?.as_array().cloned().unwrap_or_default(),
            Container::Indirect(*n, *g),
        ),
        Some(Obj::Array(a)) => (
            a.clone(),
            form_ref
                .map(|(n, g)| Container::Indirect(n, g))
                .unwrap_or(Container::Root),
        ),
        _ => (
            Vec::new(),
            form_ref
                .map(|(n, g)| Container::Indirect(n, g))
                .unwrap_or(Container::Root),
        ),
    };
    let mut dirty: Vec<Container> = Vec::new();

    let mut existing = Vec::new();
    {
        let getter = |r: (u32, u32)| w.get(r);
        enumerate_sig_fields(
            &getter,
            &fields,
            None,
            None,
            &mut Default::default(),
            &mut existing,
        )?;
    }
    let candidate = existing.iter().find(|(n, _, _)| n == field_name).cloned();
    let sig_field_ref;
    let mut field_created = false;
    if let Some((_, value, r)) = candidate {
        if value.is_some() {
            return Err(format!(
                "Signature field with name {field_name} appears to be filled already."
            ));
        }
        sig_field_ref = r;
    } else {
        let page_ref = reader.page_ref(0)?;
        let mut field = Dict::new();
        field.set("FT", Obj::name("Sig"));
        field.set("T", Obj::text(field_name));
        field.set("Type", Obj::name("Annot"));
        field.set("Subtype", Obj::name("Widget"));
        field.set("F", Obj::Int(0b1000_0100));
        field.set("Rect", Obj::Array(vec![Obj::Real("0".into()); 4]));
        field.set("P", Obj::Ref(page_ref.0, page_ref.1));
        if let Some(tu) = readable_name {
            field.set("TU", Obj::text(tu));
        }
        sig_field_ref = w.add_object(Obj::Dict(field));
        fields.push(Obj::Ref(sig_field_ref.0, sig_field_ref.1));
        dirty.push(fields_container);
        field_created = true;
        // register_annotation on the page
        let mut page = w.get(page_ref)?;
        let pd = page.as_dict_mut().ok_or("page is not a dictionary")?;
        match pd.get("Annots").cloned() {
            Some(Obj::Ref(an, ag)) => {
                let mut arr = w.get((an, ag))?;
                if let Obj::Array(a) = &mut arr {
                    a.push(Obj::Ref(sig_field_ref.0, sig_field_ref.1));
                }
                w.mark_update((an, ag), arr);
            }
            Some(Obj::Array(mut a)) => {
                a.push(Obj::Ref(sig_field_ref.0, sig_field_ref.1));
                pd.set("Annots", Obj::Array(a));
                w.mark_update(page_ref, page);
            }
            _ => {
                pd.set(
                    "Annots",
                    Obj::Array(vec![Obj::Ref(sig_field_ref.0, sig_field_ref.1)]),
                );
                w.mark_update(page_ref, page);
            }
        }
        if !form_created {
            dirty.push(fields_container);
        }
    }

    // ── ensure_sig_flags(lock_sig_flags=True) ──
    let orig_flags = form.get("SigFlags").and_then(|o| o.as_int());
    let had_need_app = form.remove("NeedAppearances").is_some();
    // put /Fields back first so a new form reads << /Fields [...] /SigFlags 3 >>
    match fields_container {
        Container::Indirect(n, g) if Some((n, g)) != form_ref => {
            w.mark_update_if(
                dirty.contains(&Container::Indirect(n, g)),
                (n, g),
                Obj::Array(fields.clone()),
            );
        }
        _ => form.set("Fields", Obj::Array(fields.clone())),
    }
    form.set("SigFlags", Obj::Int(3));
    if orig_flags != Some(3) || had_need_app {
        dirty.push(
            form_ref
                .map(|(n, g)| Container::Indirect(n, g))
                .unwrap_or(Container::Root),
        );
    }

    // ── register_extensions: ESIC developer extension level 1 (PAdES on PDF < 2.0) ──
    {
        let root = w
            .root_obj
            .as_dict_mut()
            .ok_or("catalog is not a dictionary")?;
        let mut ext = match root.get("Extensions") {
            Some(Obj::Dict(d)) => d.clone(),
            _ => Dict::new(),
        };
        let needs = match ext.get("ESIC") {
            Some(Obj::Dict(d)) => {
                d.get("ExtensionLevel")
                    .and_then(|o| o.as_int())
                    .unwrap_or(0)
                    < 1
            }
            Some(Obj::Array(a)) => !a.iter().any(|e| {
                e.as_dict()
                    .and_then(|d| d.get("ExtensionLevel"))
                    .and_then(|o| o.as_int())
                    .unwrap_or(0)
                    >= 1
            }),
            _ => true,
        };
        if needs {
            let mut esic = Dict::new();
            esic.set("Type", Obj::name("DeveloperExtensions"));
            esic.set("BaseVersion", Obj::name("1.7"));
            esic.set("ExtensionLevel", Obj::Int(1));
            ext.set("ESIC", Obj::Dict(esic));
            root.set("Extensions", Obj::Dict(ext));
        }
    }

    // ── the signature object, /V, DocMDP ──
    let mut sig_dict = sig_dict;
    let sig_ref_slot = w.last_id + 1;
    if let Some(mdp) = certify {
        let mut tp = Dict::new();
        tp.set("Type", Obj::name("TransformParams"));
        tp.set("V", Obj::name("1.2"));
        tp.set("P", Obj::Int(mdp.p()));
        let mut sr = Dict::new();
        sr.set("Type", Obj::name("SigRef"));
        sr.set("TransformMethod", Obj::name("DocMDP"));
        sr.set("TransformParams", Obj::Dict(tp));
        sig_dict.set("Reference", Obj::Array(vec![Obj::Dict(sr)]));
    }
    let sig_ref = w.add_object(Obj::Dict(sig_dict));
    debug_assert_eq!(sig_ref.0, sig_ref_slot);
    let mut field_obj = w.get(sig_field_ref)?;
    if let Some(d) = field_obj.as_dict_mut() {
        d.set("V", Obj::Ref(sig_ref.0, sig_ref.1));
    }
    w.mark_update(sig_field_ref, field_obj);
    let _ = field_created;
    if certify.is_some() {
        let root = w
            .root_obj
            .as_dict_mut()
            .ok_or("catalog is not a dictionary")?;
        let mut perms = match root.get("Perms") {
            Some(Obj::Dict(d)) => d.clone(),
            _ => Dict::new(),
        };
        perms.set("DocMDP", Obj::Ref(sig_ref.0, sig_ref.1));
        root.set("Perms", Obj::Dict(perms));
    }

    // write the form where it lives
    match form_ref {
        Some(r) => {
            if form_created || dirty.contains(&Container::Indirect(r.0, r.1)) {
                w.mark_update(r, Obj::Dict(form));
            }
        }
        None => {
            let root = w
                .root_obj
                .as_dict_mut()
                .ok_or("catalog is not a dictionary")?;
            root.set("AcroForm", Obj::Dict(form));
        }
    }
    let mut written = w.write(now_pdf)?;
    fill_byte_range(&mut written)
}

impl<'a> IncrementalWriter<'a> {
    fn mark_update_if(&mut self, cond: bool, r: (u32, u32), obj: Obj) {
        if cond {
            self.mark_update(r, obj);
        }
    }
}

/// The signature dictionary placeholder (`SignatureObject`): Contents, ByteRange, Type, Filter,
/// SubFilter, M, Location, Reason.
fn signature_dict(bytes_reserved: usize, now_pdf: &str) -> Dict {
    let cfg = granary_core::config::settings();
    let mut d = Dict::new();
    d.set("Contents", Obj::ContentsPlaceholder(bytes_reserved));
    d.set("ByteRange", Obj::ByteRangePlaceholder);
    d.set("Type", Obj::name("Sig"));
    d.set("Filter", Obj::name("Adobe.PPKLite"));
    d.set("SubFilter", Obj::name("ETSI.CAdES.detached"));
    d.set("M", Obj::text(now_pdf));
    if !cfg.SIGN_LOCATION.is_empty() {
        d.set("Location", Obj::text(&cfg.SIGN_LOCATION));
    }
    if !cfg.SIGN_REASON.is_empty() {
        d.set("Reason", Obj::text(&cfg.SIGN_REASON));
    }
    d
}

/// `_do_sign(pdf_bytes, lta=..., timestamp=...)`.
async fn _do_sign(
    pdf_bytes: &[u8],
    lta: bool,
    timestamp: bool,
    mdp: DocMdp,
) -> Result<Vec<u8>, String> {
    let cfg = granary_core::config::settings();
    let tsa_url = if (lta || timestamp) && !cfg.SIGN_TSA_URL.is_empty() {
        Some(cfg.SIGN_TSA_URL.clone())
    } else {
        None
    };
    // strict=False: tolerate PyMuPDF's xref quirk ("Illegal generation 65536 for object ID 0").
    // Read before the signer: a PDF that can't be read fails at once, without generating a certificate.
    {
        let reader = Reader::new(pdf_bytes, false)?;
        // _enforce_certification_constraints: a certification signature must come first
        let existing = existing_signatures(&reader)?;
        if existing.iter().any(|(n, _, _)| n == FIELD_NAME) {
            // prepare_sig_field raises first for a filled field of the same name
            return Err(format!(
                "Signature field with name {FIELD_NAME} appears to be filled already."
            ));
        }
        if !existing.is_empty() {
            return Err(
                "Certification signatures must be the first signature in a given document.".into(),
            );
        }
    }
    let signer = get_signer()?;

    // presign validation (B-LTA only): the signer's chain must validate against the trust roots
    let lta_ctx = if lta {
        Some(lta::presign_validation(&signer, tsa_url.as_deref()).await?)
    } else {
        None
    };

    let now = chrono::Local::now();
    let now_pdf = pdf::pdf_date(&now);

    // size estimate from a dry-run CMS over the digest of an empty message (+ a dummy timestamp)
    let empty_digest = Sha256::digest(b"").to_vec();
    let dummy_tst = match (&tsa_url, &lta_ctx) {
        // the dummy fetched while validating the TSA is cached and reused (pyHanko _dummy_response_cache)
        (Some(_), Some(ctx)) => Some(ctx.dummy_tst.clone()),
        (Some(url), None) => Some(cms::request_timestamp(url, &empty_digest).await?),
        _ => None,
    };
    let (test_der, _) = cms::signed_data(
        &signer.key,
        &signer.cert,
        &signer.other_certs,
        &empty_digest,
        dummy_tst.as_deref(),
    )?;
    let bytes_reserved = reserve_for(test_der.len());

    let mut prepared = {
        let reader = Reader::new(pdf_bytes, false)?;
        prepare_document(
            &reader,
            &mut |_| Ok(()),
            signature_dict(bytes_reserved, &now_pdf),
            Some(mdp),
            FIELD_NAME,
            None,
            &now_pdf,
        )?
    };
    let digest = byte_range_digest(&prepared);
    let attrs = cms::pades_signed_attrs(&digest, &signer.cert);
    let signed_set = cms::set_sorted(&attrs);
    let signature = signer.key.sign_sha256(&signed_set)?;
    let tst = match &tsa_url {
        Some(url) => Some(cms::request_timestamp(url, &Sha256::digest(&signature)).await?),
        None => None,
    };
    let der = cms::assemble_signed_data(
        &signer.key,
        &signer.cert,
        &signer.other_certs,
        &signed_set,
        &signature,
        tst.as_deref(),
    );
    fill_contents(&mut prepared, &der)?;
    let sig_contents = padded_contents(&der, prepared.bytes_reserved);
    let mut out = prepared.bytes;

    if let (Some(ctx), Some(url)) = (lta_ctx, tsa_url) {
        out = lta::post_signature_processing(out, &sig_contents, ctx, &url).await?;
    }
    Ok(out)
}

/// Filled signature fields of a document (name, /V, field ref), AcroForm order.
fn existing_signatures(reader: &Reader) -> Result<Vec<(String, Option<Obj>, (u32, u32))>, String> {
    let (rn, _) = reader.root_ref()?;
    let root = reader.get(rn)?;
    let Some(acro) = root.as_dict().and_then(|d| d.get("AcroForm")).cloned() else {
        return Ok(vec![]);
    };
    let acro = reader.resolve(&acro)?;
    let fields = match acro.as_dict().and_then(|d| d.get("Fields")) {
        Some(f) => reader.resolve(f)?.as_array().cloned().unwrap_or_default(),
        None => vec![],
    };
    let mut out = Vec::new();
    let getter = |r: (u32, u32)| reader.get(r.0);
    enumerate_sig_fields(
        &getter,
        &fields,
        None,
        None,
        &mut Default::default(),
        &mut out,
    )?;
    Ok(out.into_iter().filter(|(_, v, _)| v.is_some()).collect())
}

/// Best-effort integrity report of an embedded signature. Tolerant of unsigned PDFs.
pub fn validate_pdf(pdf_bytes: &[u8]) -> SealReport {
    match validate_inner(pdf_bytes) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("signature validation failed: {}", e);
            SealReport::default()
        }
    }
}

fn validate_inner(pdf_bytes: &[u8]) -> Result<SealReport, String> {
    // PdfFileReader(...) in strict mode (the default): PyMuPDF output with the "65536" generation on
    // object 0 fails here, so Python reports all-None for those files. Reproduced.
    let reader = Reader::new(pdf_bytes, true)?;
    let mut sigs = existing_signatures(&reader)?;
    if sigs.is_empty() {
        return Ok(SealReport::default());
    }
    // embedded_signatures: sorted by the revision in which the signature object last changed
    sigs.sort_by_key(|(_, v, _)| match v {
        Some(Obj::Ref(n, _)) => reader.last_change(*n).unwrap_or(0),
        _ => 0,
    });
    let (_, v, _) = &sigs[0];
    let sig_obj = match v {
        Some(o) => reader.resolve(o)?,
        None => return Ok(SealReport::default()),
    };
    let sd = sig_obj
        .as_dict()
        .ok_or("signature object is not a dictionary")?;
    // trust our own signing cert (generating it if missing, as Python's _get_signer() does)
    let _ = get_signer();
    let ty = sd.get("Type").and_then(|o| o.as_name()).unwrap_or(b"Sig");
    if ty != b"Sig" {
        return Err("Signature object type must be /Sig".into());
    }
    match sd.get("SubFilter").and_then(|o| o.as_name()) {
        Some(b"adbe.pkcs7.detached") | Some(b"ETSI.CAdES.detached") => {}
        other => {
            return Err(format!(
                "{} is not a recognized SubFilter type in signatures.",
                other
                    .map(|o| format!("/{}", String::from_utf8_lossy(o)))
                    .unwrap_or_else(|| "None".into())
            ))
        }
    }
    let contents = sd
        .get("Contents")
        .and_then(|o| o.str_bytes())
        .ok_or("signature has no /Contents")?;
    let br: Vec<usize> = sd
        .get("ByteRange")
        .and_then(|o| o.as_array())
        .ok_or("signature has no /ByteRange")?
        .iter()
        .map(|o| o.as_int().unwrap_or(0).max(0) as usize)
        .collect();
    let si = cms::parse_signer_info(&contents)?;
    let md_name = cms::digest_name(&si.digest_alg).ok_or("unsupported digest algorithm")?;
    // ByteRange digest (pairs of offset/length; reading past EOF just stops, like stream reads)
    let mut chunks: Vec<&[u8]> = Vec::new();
    for pair in br.chunks(2) {
        if pair.len() == 2 {
            let s = pair[0].min(pdf_bytes.len());
            let e = pair[0].saturating_add(pair[1]).min(pdf_bytes.len());
            chunks.push(&pdf_bytes[s..e]);
        }
    }
    let actual = digest_of(md_name, &chunks);
    // the signer's certificate (by issuer + serial)
    let cert = si
        .certs
        .iter()
        .filter_map(|c| cms::Cert::parse(c).ok())
        .find(|c| c.issuer == si.sid_issuer && c.serial == si.sid_serial)
        .ok_or("signer certificate not found in the signature")?;
    let (signed_data, embedded_digest) = match &si.signed_attrs_raw {
        None => (actual.clone(), None),
        Some(raw) => {
            check_signing_certificate(&cert, raw)?;
            let ct = cms::find_attrs(raw, cms::OID_CONTENT_TYPE)?;
            if ct.len() != 1 || ct[0].len() != 1 {
                return Err("Content type not found in signature, or multiple content-type attributes present.".into());
            }
            let ct_val = cms::oid_to_string(ct[0][0].content);
            if ct_val != si.encap_content_type {
                return Err(format!(
                    "Content type {} did not match expected value {}",
                    ct_val, si.encap_content_type
                ));
            }
            let md = cms::find_attrs(raw, cms::OID_MESSAGE_DIGEST)?;
            if md.len() != 1 || md[0].len() != 1 {
                return Err("Message digest not found in signature, or multiple message digest attributes present.".into());
            }
            (raw.clone(), Some(md[0][0].content.to_vec()))
        }
    };
    let valid = if si.signed_attrs_raw.is_none() {
        // prehashed: verify against the digest directly (RSA only)
        cms::verify_signature(
            &cert.spki,
            &si.sig_alg,
            md_name,
            &signed_data,
            &si.signature,
        )
    } else {
        cms::verify_signature(
            &cert.spki,
            &si.sig_alg,
            md_name,
            &signed_data,
            &si.signature,
        )
    };
    let intact = match embedded_digest {
        Some(d) => d == actual,
        None => valid,
    };
    Ok(SealReport {
        intact: Some(intact),
        valid: Some(valid),
        signer: Some(granary_core::config::settings().SIGN_SEAL_NAME.clone()),
        // Python reads `sig.signer_reported_dt`, an attribute EmbeddedPdfSignature does not have, so this is
        // always None. Reproduced.
        sealed_at: None,
    })
}

fn digest_of(name: &str, chunks: &[&[u8]]) -> Vec<u8> {
    fn run<D: Digest>(chunks: &[&[u8]]) -> Vec<u8> {
        let mut h = D::new();
        for c in chunks {
            h.update(c);
        }
        h.finalize().to_vec()
    }
    match name {
        "sha1" => run::<sha1::Sha1>(chunks),
        "sha384" => run::<sha2::Sha384>(chunks),
        "sha512" => run::<sha2::Sha512>(chunks),
        _ => run::<Sha256>(chunks),
    }
}

/// `_check_signing_certificate`: an ESS signing-certificate(-v2) attribute, when present, must point at the
/// signer's certificate.
fn check_signing_certificate(cert: &cms::Cert, attrs: &[u8]) -> Result<(), String> {
    let v2 = cms::find_attrs(attrs, cms::OID_SIGNING_CERT_V2)?;
    let v1 = cms::find_attrs(attrs, cms::OID_SIGNING_CERT)?;
    if v2.len() + v1.len() > 1 || v2.iter().chain(v1.iter()).any(|v| v.len() != 1) {
        return Err("Wrong cardinality for signing certificate attribute".into());
    }
    let (val, is_v2) = match (v2.first(), v1.first()) {
        (Some(v), _) => (v[0], true),
        (None, Some(v)) => (v[0], false),
        _ => return Ok(()),
    };
    let certs = cms::children(val.content)?;
    let ids = cms::children(
        certs
            .first()
            .ok_or("empty signing certificate attribute")?
            .content,
    )?;
    let first = ids.first().ok_or("empty signing certificate attribute")?;
    let parts = cms::children(first.content)?;
    let (alg, hash_ix) = if is_v2 && parts.first().map(|p| p.tag == 0x30).unwrap_or(false) {
        (
            cms::oid_to_string(cms::children(parts[0].content)?[0].content),
            1,
        )
    } else if is_v2 {
        (cms::OID_SHA256.to_string(), 0)
    } else {
        (cms::OID_SHA1.to_string(), 0)
    };
    let expected = parts.get(hash_ix).ok_or("bad ESSCertID")?.content;
    let actual = digest_of(cms::digest_name(&alg).unwrap_or("sha256"), &[&cert.der]);
    if expected != actual.as_slice() {
        return Err(
            "Signing certificate attribute does not match selected signer's certificate".into(),
        );
    }
    Ok(())
}
