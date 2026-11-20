//! winston's JSON console format (`{"level":...,"message":...,"timestamp":...}` with sorted keys).

use std::collections::BTreeMap;
use std::io::Write;

use serde_json::Value;

fn emit(level: &str, message: &str, meta: Value) {
    let mut m: BTreeMap<String, Value> = BTreeMap::new();
    if let Value::Object(o) = meta {
        for (k, v) in o {
            // JSON.stringify drops undefined; a None message is undefined.
            if !v.is_null() || k != "error" {
                m.insert(k, v);
            }
        }
    }
    m.insert("level".into(), Value::String(level.into()));
    m.insert("message".into(), Value::String(message.into()));
    m.insert(
        "timestamp".into(),
        Value::String(
            chrono::Utc::now()
                .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string(),
        ),
    );
    if let Ok(line) = serde_json::to_string(&m) {
        let _ = writeln!(std::io::stdout(), "{line}");
    }
}

pub fn info(message: &str, meta: Value) {
    emit("info", message, meta);
}

pub fn warn(message: &str, meta: Value) {
    emit("warn", message, meta);
}

pub fn error(message: &str, meta: Value) {
    emit("error", message, meta);
}
