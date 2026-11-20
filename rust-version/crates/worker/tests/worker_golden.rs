//! Golden: the beat schedule and the task table, recorded from the real Celery app
//! (compat-tests/golden/gen_worker.py -> compat-tests/golden/worker/*.json).

use serde_json::Value;

use granary_worker::registry::default_registry;

fn golden(name: &str) -> Value {
    let path = format!(
        "{}/../../compat-tests/golden/worker/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .unwrap()
}

#[test]
fn golden_beat_schedule_is_worker_py_exactly() {
    let want: Vec<(String, String, f64)> = golden("beat_schedule")
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            assert_eq!(e["args"], serde_json::json!([]), "beat sends no args");
            assert_eq!(e["kwargs"], serde_json::json!({}), "beat sends no kwargs");
            (
                e["name"].as_str().unwrap().to_string(),
                e["task"].as_str().unwrap().to_string(),
                e["schedule"].as_f64().unwrap(),
            )
        })
        .collect();
    // doclib A1: entries added in Rust after the freeze are not in the golden
    let got: Vec<(String, String, f64)> = granary_worker::beat::SCHEDULE
        .iter()
        .filter(|(_, t, _)| !granary_worker::beat::ADDED_AFTER_FREEZE.contains(t))
        .map(|(n, t, s)| (n.to_string(), t.to_string(), *s))
        .collect();
    assert_eq!(got, want);
    let entries: Vec<_> = granary_worker::beat::schedule()
        .into_iter()
        .filter(|e| !granary_worker::beat::ADDED_AFTER_FREEZE.contains(&e.task.as_str()))
        .collect();
    assert_eq!(entries.len(), want.len());
    for (e, (_, _, secs)) in entries.iter().zip(&want) {
        assert_eq!(e.every.as_secs_f64(), *secs);
    }
}

#[test]
fn golden_celery_config_is_what_the_runtime_implements() {
    // runtime.rs: acks late, one job per slot, JSON jobs. Celery's timezone only affects eta/crontab
    // schedules, which worker.py does not use (every entry is an interval).
    assert_eq!(
        golden("config"),
        serde_json::json!({"task_acks_late": true, "worker_prefetch_multiplier": 1,
                           "task_serializer": "json", "timezone": "UTC", "enable_utc": true})
    );
    assert_eq!(granary_worker::runtime::CONCURRENCY, 4);
}

/// Every registered task has the Python name, parameters, defaults and max_retries.
#[test]
fn golden_registered_tasks_match_the_python_signatures() {
    let reg = default_registry();
    let table = golden("tasks");
    let by_name: std::collections::HashMap<&str, &Value> = table
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap(), t))
        .collect();
    assert!(!reg.names().is_empty());
    for def in reg.defs() {
        if granary_worker::beat::ADDED_AFTER_FREEZE.contains(&def.name) {
            continue; // doclib A1: Rust only
        }
        let want = by_name
            .get(def.name)
            .unwrap_or_else(|| panic!("{} is not a Python task", def.name));
        let params: Vec<Value> = def
            .params
            .iter()
            .map(|p| match p.default {
                None => serde_json::json!({"name": p.name}),
                Some(d) => serde_json::json!({"name": p.name,
                                              "default": serde_json::from_str::<Value>(d).unwrap()}),
            })
            .collect();
        assert_eq!(
            Value::Array(params),
            want["params"],
            "{} parameters",
            def.name
        );
        let max = def.max_retries.map(Value::from).unwrap_or(Value::Null);
        assert_eq!(max, want["max_retries"], "{} max_retries", def.name);
    }
}

/// Python test_audit_critical.py `test_every_beat_task_is_registered`.
#[test]
fn test_every_beat_task_is_registered() {
    let reg = default_registry();
    let missing: Vec<&str> = granary_worker::beat::SCHEDULE
        .iter()
        .map(|(_, t, _)| *t)
        .filter(|t| reg.get(t).is_none())
        .collect();
    assert!(
        missing.is_empty(),
        "beat entries with no registered task: {missing:?}"
    );
}

/// Every `@celery.task` of tasks.py is registered (send_document_email included, though nothing enqueues it).
#[test]
fn every_python_task_is_registered() {
    let mut want: Vec<String> = golden("tasks")
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    want.sort();
    let got: Vec<String> = default_registry()
        .names()
        .into_iter()
        .filter(|n| !granary_worker::beat::ADDED_AFTER_FREEZE.contains(n)) // doclib A1
        .map(str::to_string)
        .collect();
    assert_eq!(got, want);
}

/// Jobs bind to the parameters as a Python call does (`f(*args, **kwargs)`).
#[test]
fn arguments_bind_like_a_python_call() {
    use granary_worker::registry::{Outcome, Param, TaskDef};
    use serde_json::{json, Map};
    const P: &[Param] = &[
        Param::req("chat_id"),
        Param::req("message_id"),
        Param::opt("late", "false"),
    ];
    let def = TaskDef::new("app.tasks.f", P, |_| Box::pin(async { Outcome::done("x") }));
    let kw = |pairs: &[(&str, Value)]| -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    };
    let ok = def.bind(&[json!("c"), json!("m")], &Map::new()).unwrap();
    assert_eq!(
        Value::Object(ok),
        json!({"chat_id": "c", "message_id": "m", "late": false})
    );
    let ok = def
        .bind(
            &[json!("c")],
            &kw(&[("late", json!(true)), ("message_id", json!("m"))]),
        )
        .unwrap();
    assert_eq!(
        Value::Object(ok),
        json!({"chat_id": "c", "message_id": "m", "late": true})
    );
    assert_eq!(
        def.bind(&[], &Map::new()).unwrap_err(),
        "f() missing 2 required positional arguments: 'chat_id' and 'message_id'"
    );
    assert_eq!(
        def.bind(&[json!(1), json!(2), json!(3), json!(4)], &Map::new())
            .unwrap_err(),
        "f() takes 3 positional arguments but 4 were given"
    );
    assert_eq!(
        def.bind(&[json!("c")], &kw(&[("chat_id", json!("x"))]))
            .unwrap_err(),
        "f() got multiple values for argument 'chat_id'"
    );
    assert_eq!(
        def.bind(&[json!("c"), json!("m")], &kw(&[("x", json!(1))]))
            .unwrap_err(),
        "f() got an unexpected keyword argument 'x'"
    );
}
