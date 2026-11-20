//! Golden files under compat-tests/golden: read them, and with `GOLDEN_BLESS=1` write the values the Rust code
//! produces back (the goldens were first recorded from the retired Python system; Rust is now the reference).
//! Bless only after a change you meant, then review the diff of the golden file.
//! Use from a test file with `#[path = "golden/bless.rs"] mod bless;`.
#![allow(dead_code)]

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

pub fn blessing() -> bool {
    std::env::var("GOLDEN_BLESS").is_ok_and(|v| v == "1")
}

pub fn path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../compat-tests/golden")
        .join(rel)
}

pub fn read(rel: &str) -> Value {
    let p = path(rel);
    serde_json::from_str(
        &std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())),
    )
    .unwrap()
}

/// Writes `v` in the goldens' layout (one-space indent, UTF-8 kept, trailing newline).
pub fn write(rel: &str, v: &Value) {
    let mut out = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut ser = serde_json::Serializer::with_formatter(&mut out, fmt);
    v.serialize(&mut ser).unwrap();
    out.push(b'\n');
    std::fs::write(path(rel), out).unwrap();
}

/// When blessing: read, change and write one golden file (tests of one binary that bless the same file take
/// turns). Does nothing otherwise.
pub fn update(rel: &str, f: impl FnOnce(&mut Value)) {
    if !blessing() {
        return;
    }
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut v = read(rel);
    f(&mut v);
    write(rel, &v);
}

/// Compares `got` with the golden value in `slot` (exact JSON text), or, when blessing, stores `got` there.
pub fn eq(slot: &mut Value, got: impl Into<Value>, msg: impl std::fmt::Display) {
    let got = got.into();
    if blessing() {
        *slot = got;
    } else {
        assert_eq!(
            serde_json::to_string(&got).unwrap(),
            serde_json::to_string(slot).unwrap(),
            "{msg}"
        );
    }
}

/// Writes `v` to the golden file when blessing (after the `eq` calls that filled it).
pub fn save(rel: &str, v: &Value) {
    if blessing() {
        write(rel, v);
    }
}

/// Compares `got` with `want`; when blessing, records `got` in `*slot` instead (the caller writes the file).
pub fn check(got: &Value, want: &Value, slot: &mut Value, msg: &str) {
    if blessing() {
        *slot = got.clone();
    } else {
        assert_eq!(
            serde_json::to_string(got).unwrap(),
            serde_json::to_string(want).unwrap(),
            "{msg}"
        );
    }
}

/// The seed's first superadmin (and its employee record) as a neutral owner, so goldens never carry the
/// seed's default identity.
pub async fn neutral_owner(pool: &sqlx::PgPool) {
    let id: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE is_superadmin ORDER BY created_at LIMIT 1")
            .fetch_optional(pool)
            .await
            .unwrap();
    let Some(id) = id else { return };
    sqlx::query("UPDATE users SET email = 'owner@example.com', name = 'Alex Owner' WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE employees SET first_name = 'Alex', last_name = 'Owner', email = 'owner@example.com' \
         WHERE user_id = $1",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
}
