//! Golden: documents/core_.rs `fingerprint` (sha256 of json.dumps(sort_keys=True, default=str)) against
//! core.fingerprint (compat-tests/golden/documents/fingerprint.json). Block data and cover are the text
//! Postgres prints for the jsonb, totals the NUMERIC(18,2) text, as both drivers read them back.
//! `dumps` and `sha256` re-record with `GOLDEN_BLESS=1` (tests/golden/bless.rs).

#[path = "golden/bless.rs"]
mod bless;

use granary_services::documents::core_;
use serde_json::Value;

#[derive(serde::Deserialize)]
struct Doc {
    title: String,
    subtitle: Option<String>,
    language: String,
    cover: String,
    validity_days: i32,
    classification: Option<String>,
}

#[derive(serde::Deserialize)]
struct Block {
    section: String,
    block_type: String,
    data: String,
}

#[derive(serde::Deserialize)]
struct Quote {
    id: String,
    number: String,
    full_number: Option<String>,
    total: Option<String>,
    currency: Option<String>,
    revision: Option<i32>,
}

#[derive(serde::Deserialize)]
struct Case {
    doc: Doc,
    blocks: Vec<Block>,
    quotes: Vec<Quote>,
    dumps: String,
    sha256: String,
}

#[test]
fn fingerprint_matches_python() {
    const REL: &str = "documents/fingerprint.json";
    let mut raw = bless::read(REL);
    let cases: Vec<Case> = serde_json::from_value(raw.clone()).unwrap();
    assert!(cases.len() >= 100);
    for (i, c) in cases.iter().enumerate() {
        let cover: Value = serde_json::from_str(&c.doc.cover).unwrap();
        let blocks: Vec<(String, String, Value)> = c
            .blocks
            .iter()
            .map(|b| {
                (
                    b.section.clone(),
                    b.block_type.clone(),
                    serde_json::from_str(&b.data).unwrap(),
                )
            })
            .collect();
        let quotes: Vec<core_::FingerprintQuote> = c
            .quotes
            .iter()
            .map(|q| {
                let number = q
                    .full_number
                    .clone()
                    .filter(|f| !f.is_empty())
                    .unwrap_or(q.number.clone());
                (
                    q.id.parse().unwrap(),
                    number,
                    q.total.as_deref().map(|t| t.parse().unwrap()),
                    q.currency.clone(),
                    q.revision,
                )
            })
            .collect();
        let body = core_::fingerprint_body(
            &c.doc.title,
            c.doc.subtitle.as_deref(),
            &c.doc.language,
            &cover,
            c.doc.validity_days,
            c.doc.classification.as_deref(),
            &blocks,
            &quotes,
        );
        let _ = (&c.dumps, &c.sha256);
        let slot = &mut raw[i];
        bless::eq(
            &mut slot["dumps"],
            core_::dump_body(&body).unwrap(),
            format!("case {i}"),
        );
        bless::eq(
            &mut slot["sha256"],
            core_::hash_body(&body).unwrap(),
            format!("case {i}"),
        );
    }
    bless::save(REL, &raw);
}
