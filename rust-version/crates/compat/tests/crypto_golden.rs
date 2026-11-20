//! Golden tests for `granary_compat::crypto` against fixtures recorded from the Python backend by
//! `compat-tests/golden/gen_crypto.py`, plus the reverse direction: values made here are written to a
//! temp file and checked by the backend code (`gen_crypto.py --verify`) when the Python venv exists.

use std::collections::VecDeque;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use granary_compat::crypto::{calls, fernet, hashes, jwt, mcp, password, pkce, tokens};
use serde_json::{json, Map, Value};

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../compat-tests/golden")
}

fn load(name: &str) -> Value {
    let path = golden_dir().join("crypto").join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}"))
}

fn arr(v: &Value) -> &Vec<Value> {
    v.as_array().unwrap_or_else(|| panic!("not an array: {v}"))
}

fn obj(v: &Value) -> &Map<String, Value> {
    v.as_object()
        .unwrap_or_else(|| panic!("not an object: {v}"))
}

fn jwt_cfg(config: &Value) -> jwt::JwtConfig {
    jwt::JwtConfig {
        secret: s(&config["JWT_SECRET_KEY"]).into(),
        algorithm: s(&config["JWT_ALGORITHM"]).into(),
        access_minutes: config["JWT_ACCESS_TOKEN_EXPIRE_MINUTES"].as_i64().unwrap(),
        refresh_days: config["JWT_REFRESH_TOKEN_EXPIRE_DAYS"].as_i64().unwrap(),
    }
}

/// Claims maps compared as JSON text with numbers normalised (arbitrary_precision keeps "1.0" vs "1").
fn same_claims(a: &Map<String, Value>, b: &Map<String, Value>) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|((ka, va), (kb, vb))| ka == kb && same_value(va, vb))
}

fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            x.as_f64() == y.as_f64() && x.is_f64() == y.is_f64()
        }
        _ => a == b,
    }
}

#[test]
fn jwt_create_matches_python() {
    let d = load("jwt");
    let cfg = jwt_cfg(&d["config"]);
    let mut n = 0;
    for c in arr(&d["create"]) {
        let now: DateTime<Utc> = DateTime::parse_from_rfc3339(s(&c["now"]))
            .unwrap()
            .with_timezone(&Utc);
        let user = s(&c["user_id"]);
        let extra = c.get("extra").map(|e| obj(e).clone());
        let access = jwt::create_access_token_at(&cfg, user, extra.as_ref(), now).unwrap();
        assert_eq!(
            access,
            s(&c["access"]),
            "access token for {user:?} at {now}"
        );
        if !c["refresh"].is_null() {
            let refresh = jwt::create_refresh_token_with(&cfg, user, s(&c["jti"]), now).unwrap();
            assert_eq!(
                refresh,
                s(&c["refresh"]),
                "refresh token for {user:?} at {now}"
            );
        }
        n += 1;
    }
    assert!(n >= 12);
}

#[test]
fn jwt_decode_matches_python() {
    let d = load("jwt");
    let cfg = jwt_cfg(&d["config"]);
    for c in arr(&d["decode"]) {
        let got = jwt::decode_token_at(&cfg, s(&c["token"]), c["now"].as_i64().unwrap());
        let want = obj(&c["claims"]);
        assert!(
            same_claims(&got, want),
            "decode case {:?}: rust {got:?} python {want:?}",
            c["name"]
        );
    }
}

#[test]
fn bcrypt_matches_passlib() {
    let d = load("bcrypt");
    for c in arr(&d["python_hashes"]) {
        let (p, h) = (s(&c["password"]), s(&c["hash"]));
        assert_eq!(
            password::verify_password(p, h).unwrap(),
            c["verify_ok"].as_bool().unwrap(),
            "{p:?}"
        );
        assert_eq!(
            password::verify_password(s(&c["wrong"]), h).unwrap(),
            c["verify_wrong"].as_bool().unwrap()
        );
    }
    for c in arr(&d["fixed_salt"]) {
        let salt: [u8; 16] = hex::decode(s(&c["salt_hex"])).unwrap().try_into().unwrap();
        let h = password::hash_password_with_salt(
            s(&c["password"]),
            c["cost"].as_u64().unwrap() as u32,
            salt,
        )
        .unwrap();
        assert_eq!(h, s(&c["hash"]), "fixed-salt hash of {:?}", c["password"]);
        assert_eq!(
            password::verify_password(s(&c["password"]), &h).unwrap(),
            c["passlib_verify"].as_bool().unwrap()
        );
    }
    for c in arr(&d["other_idents"]) {
        assert_eq!(
            password::verify_password(s(&c["password"]), s(&c["hash"])).unwrap(),
            c["passlib_verify"].as_bool().unwrap()
        );
    }
    for c in arr(&d["errors"]) {
        let r = password::verify_password(s(&c["password"]), s(&c["hash"]));
        assert_eq!(r.is_err(), c["raises"].as_bool().unwrap(), "error case {c}");
    }
    assert_eq!(
        password::hash_password("has\0nul").is_err(),
        d["hash_nul_raises"][0].as_bool().unwrap()
    );
}

#[test]
fn fernet_matches_python() {
    let d = load("fernet");
    let secret = s(&d["config"]["APP_SECRET_KEY"]);
    for k in arr(&d["keys"]) {
        assert_eq!(fernet::derive_key(s(&k["app_secret_key"])), s(&k["key"]));
    }
    for c in arr(&d["fixed"]).iter().chain(arr(&d["python_made"])) {
        assert_eq!(
            fernet::decrypt(secret, s(&c["token"])).unwrap(),
            s(&c["plaintext"])
        );
    }
    for c in arr(&d["invalid"]).iter().chain(arr(&d["lenient"])) {
        let r = fernet::decrypt(secret, s(&c["token"]));
        if c["ok"].as_bool().unwrap() {
            assert_eq!(
                r.as_deref().ok(),
                c["plaintext"].as_str(),
                "case {:?}",
                c["name"]
            );
        } else {
            assert!(r.is_err(), "case {:?} should fail", c["name"]);
        }
    }
    // Round trip with a fresh IV.
    let t = fernet::encrypt(secret, "round ☃ trip");
    assert_eq!(fernet::decrypt(secret, &t).unwrap(), "round ☃ trip");
}

fn scripted(c: &Value) -> tokens::Scripted {
    tokens::Scripted {
        bytes: arr(&c["bytes_hex"])
            .iter()
            .map(|b| hex::decode(s(b)).unwrap())
            .collect::<VecDeque<_>>(),
        indices: arr(&c["indices"])
            .iter()
            .map(|i| i.as_u64().unwrap() as usize)
            .collect(),
    }
}

#[test]
fn tokens_match_python() {
    let d = load("tokens");
    for c in arr(&d["scripted"]) {
        let mut r = scripted(c);
        let kind = s(&c["kind"]);
        let out: Value = match kind {
            "token_urlsafe_32" => tokens::token_urlsafe(&mut r, 32).into(),
            "token_urlsafe_12" => tokens::token_urlsafe(&mut r, 12).into(),
            "token_hex_32" => tokens::token_hex(&mut r, 32).into(),
            "pat" => tokens::new_pat(&mut r).into(),
            "mcp_oauth_pair" => {
                let (a, b) = tokens::new_mcp_oauth_pair(&mut r);
                json!([a, b])
            }
            "mcp_auth_code" => tokens::new_mcp_auth_code(&mut r).into(),
            "mcp_client_id" => tokens::new_mcp_client_id(&mut r).into(),
            "kiosk" => tokens::new_kiosk_token(&mut r).into(),
            "product_api_key" => tokens::new_product_api_key(&mut r).into(),
            "product_license_key" => tokens::new_product_license_key(&mut r).into(),
            "demo_licence_key" => tokens::new_demo_licence_key(&mut r).into(),
            "short_code" => tokens::new_short_code(&mut r).into(),
            "temp_password" => tokens::new_temp_password(&mut r).into(),
            other => panic!("unknown kind {other}"),
        };
        assert_eq!(out, c["output"], "{kind}");
        assert!(
            r.bytes.is_empty() && r.indices.is_empty(),
            "{kind} drew a different amount of randomness"
        );
    }
    let real = &arr(&d["real"])[0];
    assert_eq!(
        s(&real["pat"]).len(),
        "gpat_".len() + tokens::token_urlsafe_len(32)
    );
    assert_eq!(s(&real["kiosk"]).len(), 6 + 43);
    assert!(s(&real["short_code"])
        .chars()
        .all(|ch| tokens::SHORT_CODE_ALPHABET.contains(ch)));
    for c in arr(&d["prefixes"]) {
        assert_eq!(tokens::token_prefix(s(&c["raw"])), s(&c["prefix"]));
    }
    for c in arr(&d["hashes"]) {
        assert_eq!(hashes::token_hash(s(&c["token"])), s(&c["sha256"]));
    }
}

#[test]
fn short_hashes_match_python() {
    let d = load("hashes");
    for c in arr(&d["send_keys"]) {
        let to: Vec<&str> = arr(&c["to"]).iter().map(s).collect();
        assert_eq!(
            hashes::doc_send_key(
                s(&c["entity_type"]),
                s(&c["entity_id"]),
                s(&c["channel"]),
                &to
            ),
            s(&c["key"])
        );
    }
    for c in arr(&d["fingerprints"]) {
        let parts: Vec<&str> = arr(&c["parts"]).iter().map(s).collect();
        assert_eq!(
            hashes::nary_fingerprint(&parts),
            s(&c["fingerprint"]),
            "{parts:?}"
        );
    }
    for c in arr(&d["gallery"]) {
        assert_eq!(
            hashes::gallery_version(c["count"].as_i64().unwrap(), s(&c["stamp_str"])),
            s(&c["version"])
        );
    }
    for c in arr(&d["message_ids"]) {
        assert_eq!(
            hashes::crm_auto_message_id(&hex::decode(s(&c["raw_hex"])).unwrap()),
            s(&c["message_id"])
        );
    }
}

#[test]
fn pkce_matches_python() {
    let d = load("pkce");
    for c in arr(&d["cases"]) {
        let (v, ch) = (s(&c["verifier"]), s(&c["challenge"]));
        assert_eq!(pkce::s256_challenge(v), ch);
        assert_eq!(
            pkce::pkce_ok(Some(v), Some(ch)).unwrap(),
            c["ok"].as_bool().unwrap()
        );
        assert_eq!(
            pkce::pkce_ok(Some(&format!("{v}x")), Some(ch)).unwrap(),
            c["ok_wrong"].as_bool().unwrap()
        );
        assert_eq!(
            pkce::pkce_ok(None, Some(ch)).unwrap(),
            c["ok_none_verifier"].as_bool().unwrap()
        );
    }
    for c in arr(&d["errors"]) {
        assert_eq!(
            pkce::pkce_ok(Some("v"), Some(s(&c["challenge"]))).is_err(),
            c["raises"].as_bool().unwrap()
        );
    }
    for c in arr(&d["gate"]) {
        assert_eq!(
            pkce::challenge_acceptable(s(&c["method"]), c["challenge"].as_str()),
            c["ok"].as_bool().unwrap(),
            "{c}"
        );
    }
}

#[test]
fn mcp_codes_match_python() {
    let d = load("mcp");
    let secret = s(&d["config"]["JWT_SECRET_KEY"]);
    for c in arr(&d["confirm"]) {
        let edited = if c["edited"].is_null() {
            None
        } else {
            Some(&c["edited"])
        };
        let v = mcp::confirm_value(
            s(&c["secret"]),
            s(&c["user_id"]),
            s(&c["card_id"]),
            s(&c["decision"]),
            edited,
        );
        assert_eq!(
            v,
            s(&c["value"]),
            "confirm_value edited={} secret={:?}",
            c["edited"],
            c["secret"]
        );
    }
    // GOLDEN_BLESS=1 re-records the cards' version, sign and code after an intended change to their inputs.
    if std::env::var("GOLDEN_BLESS").is_ok_and(|v| v == "1") {
        let mut out = d.clone();
        for c in out["cards"].as_array_mut().unwrap() {
            let p = obj(&c["proposal"]).clone();
            let issued = c["issued"].as_i64().unwrap();
            let user = s(&c["user_id"]).to_string();
            c["version"] = mcp::proposal_version(&p).into();
            c["sign"] = mcp::sign(secret, &user, &p, issued).into();
            c["code"] = mcp::card_code(secret, &user, &p, issued).into();
        }
        let mut buf = Vec::new();
        let fmt = serde_json::ser::PrettyFormatter::with_indent(b" ");
        let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
        serde::Serialize::serialize(&out, &mut ser).unwrap();
        buf.push(b'\n');
        std::fs::write(golden_dir().join("crypto/mcp.json"), buf).unwrap();
        return;
    }
    for c in arr(&d["cards"]) {
        let p = obj(&c["proposal"]);
        let issued = c["issued"].as_i64().unwrap();
        assert_eq!(
            mcp::proposal_version(p),
            s(&c["version"]),
            "version of {p:?}"
        );
        assert_eq!(
            mcp::sign(secret, s(&c["user_id"]), p, issued),
            s(&c["sign"])
        );
        assert_eq!(
            mcp::card_code(secret, s(&c["user_id"]), p, issued),
            s(&c["code"])
        );
    }
    let mut used = std::collections::HashSet::new();
    for c in arr(&d["checks"]) {
        let r = mcp::check_code(
            secret,
            s(&c["user_id"]),
            obj(&c["proposal"]),
            c["code"].as_str(),
            c["now"].as_f64().unwrap(),
            c["minutes"].as_i64().unwrap(),
        )
        .unwrap();
        // The Redis claim (SET NX) the Python makes after a good check, simulated with a set.
        let outcome: Value = match r {
            mcp::CodeCheck::Good { sig } => {
                if used.insert(sig) {
                    Value::Null
                } else {
                    "used".into()
                }
            }
            mcp::CodeCheck::Changed => "changed".into(),
            mcp::CodeCheck::Expired => "expired".into(),
        };
        assert_eq!(outcome, c["result"], "check case {:?}", c["name"]);
    }
}

#[test]
fn calls_match_python() {
    let d = load("calls");
    let app_secret = s(&d["config"]["APP_SECRET_KEY"]);
    for c in arr(&d["turn"]) {
        let t = calls::turn_credentials(
            s(&c["turn_secret"]),
            s(&c["user_id"]),
            c["ttl"].as_i64().unwrap(),
            c["now"].as_i64().unwrap(),
        );
        assert_eq!(t.username, s(&c["username"]));
        assert_eq!(t.password, s(&c["password"]));
    }
    for c in arr(&d["tickets"]) {
        let t = calls::signal_ticket(
            app_secret,
            s(&c["room"]),
            s(&c["peer"]),
            c["host"].as_bool().unwrap(),
            c["ttl"].as_i64().unwrap(),
            c["now"].as_i64().unwrap(),
        );
        assert_eq!(t, s(&c["ticket"]));
    }
    for c in arr(&d["verify"]) {
        let got =
            calls::verify_signal_ticket(app_secret, s(&c["ticket"]), c["now"].as_f64().unwrap())
                .unwrap();
        let got = got.map(Value::Object).unwrap_or(Value::Null);
        let ok = match (&got, &c["result"]) {
            (Value::Object(a), Value::Object(b)) => same_claims(a, b),
            (a, b) => a == b,
        };
        assert!(
            ok,
            "verify case {:?}: rust {got} python {}",
            c["name"], c["result"]
        );
    }
}
