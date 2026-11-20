//! Shared by the crm_* tests of the S15 port: the golden fixtures, the scenario database and test users.
#![allow(dead_code)]

use std::collections::HashSet;
use std::path::PathBuf;

use chrono::Utc;
use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

pub fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../compat-tests/golden")
}

pub fn load(rel: &str) -> Value {
    let p = golden_dir().join(rel);
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p:?}: {e}")))
        .unwrap()
}

/// support::fresh_db plus compat-tests/golden/crm_search/scenario.sql.
pub async fn scenario_db(pool: &PgPool) {
    let sql = std::fs::read_to_string(golden_dir().join("crm_search/scenario.sql")).unwrap();
    sqlx::raw_sql(&sql)
        .execute(pool)
        .await
        .expect("scenario.sql");
}

pub fn user(id: &str, is_superadmin: bool, perms: &[&str]) -> AuthUser {
    let now = Utc::now();
    AuthUser {
        id: Uuid::parse_str(id).unwrap(),
        email: "t@s15.test".into(),
        name: "T".into(),
        phone: None,
        avatar_url: None,
        department: None,
        position: None,
        is_active: true,
        is_superadmin,
        is_system_admin: false,
        portal_type: "internal".into(),
        partner_account_id: None,
        vendor_id: None,
        last_login_at: None,
        must_change_password: None,
        created_at: now,
        updated_at: now,
        perms: perms.iter().map(|s| s.to_string()).collect::<HashSet<_>>(),
    }
}

/// The generator's USERS table.
pub fn named_user(k: &str) -> AuthUser {
    match k {
        "super" => user("00000000-0000-0000-0000-0000000000a0", true, &[]),
        "own_a1" => user(
            "00000000-0000-0000-0000-0000000000a1",
            false,
            &["crm:view_own"],
        ),
        "all_a2" => user(
            "00000000-0000-0000-0000-0000000000a2",
            false,
            &["crm:view_all"],
        ),
        "none_a2" => user("00000000-0000-0000-0000-0000000000a2", false, &[]),
        other => panic!("unknown user {other}"),
    }
}

/// (status, detail) of an HTTP-style error.
pub fn http_of(e: &ApiError) -> (u16, Value) {
    (e.status().as_u16(), e.body()["detail"].clone())
}
