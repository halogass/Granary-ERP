//! tests/test_numbering.py: numbers are unique across sessions and carry the configured prefix.

mod support;

use granary_services::numbering::{self, next_document_number};

fn unique_seq() -> String {
    format!(
        "pytest_seq_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

/// `monkeypatch.setitem(numbering._DEFAULT_PREFIXES, seq, prefix)`, undone on drop.
struct PrefixPatch(String);

impl PrefixPatch {
    fn set(seq: &str, prefix: &str) -> Self {
        numbering::_DEFAULT_PREFIXES
            .write()
            .unwrap()
            .insert(seq.to_string(), prefix.to_string());
        PrefixPatch(seq.to_string())
    }
}

impl Drop for PrefixPatch {
    fn drop(&mut self) {
        if let Ok(mut m) = numbering::_DEFAULT_PREFIXES.write() {
            m.remove(&self.0);
        }
    }
}

#[tokio::test]
async fn test_a_session_that_numbers_again_after_another_session_gets_a_fresh_number() {
    let pool = support::fresh_db("numbering_prefixes_sessions").await;
    let seq = unique_seq();
    let mut a = pool.acquire().await.unwrap();
    let mut b = pool.acquire().await.unwrap();
    let mut ta = sqlx::Connection::begin(&mut *a).await.unwrap();
    let first = next_document_number(&mut ta, &seq).await.unwrap();
    ta.commit().await.unwrap();
    let mut tb = sqlx::Connection::begin(&mut *b).await.unwrap();
    let second = next_document_number(&mut tb, &seq).await.unwrap();
    tb.commit().await.unwrap();
    let mut ta = sqlx::Connection::begin(&mut *a).await.unwrap();
    let third = next_document_number(&mut ta, &seq).await.unwrap();
    ta.commit().await.unwrap();
    let numbers = [first, second, third];
    let unique: std::collections::HashSet<_> = numbers.iter().collect();
    assert_eq!(unique.len(), 3, "{numbers:?}");
    let tails: Vec<&str> = numbers
        .iter()
        .map(|n| n.rsplit('-').next().unwrap())
        .collect();
    assert_eq!(tails, ["00001", "00002", "00003"]);
}

#[tokio::test]
async fn test_a_type_with_no_row_yet_uses_its_configured_prefix() {
    let pool = support::fresh_db("numbering_prefixes_default").await;
    let seq = unique_seq();
    let _patch = PrefixPatch::set(&seq, "PYT");
    let mut tx = pool.begin().await.unwrap();
    let number = next_document_number(&mut tx, &seq).await.unwrap();
    tx.rollback().await.unwrap();
    assert!(number.starts_with("PYT-"), "{number}");
}

#[tokio::test]
async fn test_a_row_stuck_on_the_raw_type_key_is_corrected() {
    let pool = support::fresh_db("numbering_prefixes_repair").await;
    let seq = unique_seq();
    let mut tx = pool.begin().await.unwrap();
    let first = next_document_number(&mut tx, &seq).await.unwrap(); // no default yet: the raw type key
    tx.commit().await.unwrap();
    assert!(first.starts_with(&seq.to_uppercase()), "{first}");
    let _patch = PrefixPatch::set(&seq, "PYT");
    let mut tx = pool.begin().await.unwrap();
    let second = next_document_number(&mut tx, &seq).await.unwrap();
    tx.commit().await.unwrap();
    assert!(
        second.starts_with("PYT-") && second.ends_with("-00002"),
        "{second}"
    );
}

#[test]
fn test_every_seeded_sequence_has_a_default_prefix() {
    assert_eq!(
        numbering::default_prefix("offer_letter").as_deref(),
        Some("OL")
    );
    assert_eq!(
        numbering::default_prefix("credit_note_home").as_deref(),
        Some("CN")
    );
}
