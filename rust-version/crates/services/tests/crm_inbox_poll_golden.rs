//! The Rust poll_crm_inbox against GreenMail, replaying compat-tests/golden/crm_inbox/poll/manifest.json and
//! comparing the rows it leaves with what the real Python poller left (recorded once; the Python generator is gone).

#[path = "golden/bless.rs"]
mod bless;
mod greenmail;
mod support;

use std::path::PathBuf;

use serde_json::Value;

use granary_services::crm_inbox;
use granary_services::imap_sync::pyemail::utils::{format_datetime, PyDateTime};

fn poll_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../compat-tests/golden/crm_inbox/poll")
}

fn mail(name: &str) -> Vec<u8> {
    let raw = std::fs::read(poll_dir().join("mail").join(name)).unwrap();
    let recent = chrono::Utc::now() - chrono::Duration::hours(1);
    let dt = PyDateTime {
        naive: recent.naive_utc(),
        offset: Some(0),
    };
    let s = format_datetime(&dt);
    let mut out = Vec::new();
    let pat = b"{{RECENT}}";
    let mut i = 0;
    while i < raw.len() {
        if raw[i..].starts_with(pat) {
            out.extend_from_slice(s.as_bytes());
            i += pat.len();
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    crlf(&out)
}

/// imaplib.append normalises every line break to CRLF (MapCRLF); the generator appends through it.
fn crlf(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + 64);
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            b'\r' => {
                out.extend_from_slice(b"\r\n");
                if raw.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
            }
            b'\n' => out.extend_from_slice(b"\r\n"),
            b => out.push(b),
        }
        i += 1;
    }
    out
}

async fn dump(pool: &sqlx::PgPool) -> Value {
    let sql = std::fs::read_to_string(poll_dir().join("dump.sql")).unwrap();
    let body = sql.split_once("SET TIME ZONE 'UTC';").unwrap().1;
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("SET TIME ZONE 'UTC'")
        .execute(&mut *conn)
        .await
        .unwrap();
    let s: String = sqlx::query_scalar(body.trim().trim_end_matches(';'))
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    serde_json::from_str(&s).unwrap()
}

async fn run(scenario: &str) -> Vec<Value> {
    let pool = support::fresh_db(&format!("s5_poll_{scenario}")).await;
    sqlx::raw_sql(&std::fs::read_to_string(poll_dir().join("scenario.sql")).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    crm_inbox::set_session_pool(pool.clone());
    greenmail::install_nary();
    greenmail::ensure().await;
    let user = greenmail::new_user(scenario);
    greenmail::configure(&pool, &user).await;
    let mut imap = greenmail::login(&user).await;

    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(poll_dir().join("manifest.json")).unwrap())
            .unwrap();
    let mut results = Vec::new();
    for step in manifest[scenario].as_array().unwrap() {
        let quoted = |b: &str| format!("\"{b}\"");
        match step["op"].as_str().unwrap() {
            "create" => {
                imap.create(&quoted(step["box"].as_str().unwrap()))
                    .await
                    .unwrap();
            }
            "append" => {
                let flags = if step["seen"].as_bool().unwrap() {
                    Some("(\\Seen)")
                } else {
                    None
                };
                let r = imap
                    .append(
                        &mail(step["file"].as_str().unwrap()),
                        &quoted(step["box"].as_str().unwrap()),
                        flags,
                    )
                    .await
                    .unwrap();
                assert!(r.ok(), "{r:?}");
            }
            "seen" => {
                imap.select(&quoted(step["box"].as_str().unwrap()))
                    .await
                    .unwrap();
                let uid = step["uid"].as_i64().unwrap().to_string();
                let flag = if step["seen"].as_bool().unwrap() {
                    "+FLAGS"
                } else {
                    "-FLAGS"
                };
                imap.uid("store", &[&uid, flag, "(\\Seen)"]).await.unwrap();
            }
            "poll" => {
                match crm_inbox::poll_crm_inbox(true, step["backfill"].as_bool().unwrap()).await {
                    Ok(n) => results.push(serde_json::json!({"op": "poll", "stored": n})),
                    Err(e) => {
                        let s = e.to_string();
                        results.push(serde_json::json!({"op": "poll", "error": s.split(':').next().unwrap(), "detail": s}))
                    }
                }
            }
            "dump" => results.push(serde_json::json!({"op": "dump", "rows": dump(&pool).await})),
            other => panic!("{other}"),
        }
    }
    let _ = imap.logout().await;
    results
}

fn compare(scenario: &str, got: &[Value]) {
    if bless::blessing() {
        bless::update("crm_inbox/poll/expected.json", |v| {
            v[scenario] = Value::Array(got.to_vec())
        });
        return;
    }
    let expected: Value =
        serde_json::from_str(&std::fs::read_to_string(poll_dir().join("expected.json")).unwrap())
            .unwrap();
    let want = expected[scenario].as_array().unwrap();
    assert_eq!(got.len(), want.len());
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        if w["op"] == "poll" && w.get("error").is_some() {
            assert_eq!(g["error"], w["error"], "step {i}: {g}");
            // Python: PendingRollbackError after the failed flush; same session state here.
            let pre = "Sync failed: This Session's transaction has been rolled back";
            assert!(w["detail"].as_str().unwrap().starts_with(pre));
            assert!(
                g["detail"].as_str().unwrap().starts_with(pre),
                "step {i}: {g}"
            );
            continue;
        }
        if g != w {
            for key in [
                "messages",
                "threads",
                "activities",
                "events",
                "junk_mark",
                "skipped_marks",
                "orphan_threads",
            ] {
                if g["rows"][key] != w["rows"][key] {
                    eprintln!(
                        "step {i} {key}\n rust:   {}\n python: {}",
                        g["rows"][key], w["rows"][key]
                    );
                }
            }
            panic!(
                "{scenario} step {i} differs: {g} vs {w}",
                g = g.get("stored").unwrap_or(&Value::Null),
                w = w.get("stored").unwrap_or(&Value::Null)
            );
        }
    }
}

#[tokio::test]
async fn poll_matches_python_on_greenmail() {
    let main = run("main").await;
    compare("main", &main);
    let poison = run("poison").await;
    compare("poison", &poison);
    let skip = run("skip").await;
    compare("skip", &skip);
}
