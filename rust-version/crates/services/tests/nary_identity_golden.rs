//! nary/{identity_match, scoring, memory, profiling} against compat-tests/golden/nary_identity/*.json, first
//! recorded from the retired Python functions (re-record with GOLDEN_BLESS=1 after an intended change and
//! review the diff).

#[path = "golden/bless.rs"]
mod bless;
mod support;

use granary_db::rows::nary::{NaryMemory, WaChat};
use granary_services::nary::{identity_match as im, memory, profiling, scoring};
use serde_json::{json, Value};
use uuid::Uuid;

fn golden(name: &str) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../compat-tests/golden/nary_identity")
                .join(name),
        )
        .unwrap(),
    )
    .unwrap()
}

fn s(v: &Value) -> Option<&str> {
    v.as_str()
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

fn hints_json(h: &im::Hints) -> Value {
    json!({"name": h.name, "company": h.company, "product": h.product, "emails": h.emails, "phones": h.phones})
}

fn table(v: &Value) -> scoring::ClassTable {
    v.as_array()
        .unwrap()
        .iter()
        .map(|kv| {
            let r = kv[1].as_array().unwrap();
            (
                kv[0].as_str().unwrap().to_string(),
                (r[0].as_i64().unwrap(), r[1].as_i64().unwrap()),
            )
        })
        .collect()
}

#[test]
fn pure_golden() {
    let mut g = golden("pure.json");
    for c in g["prefill_hints"].as_array_mut().unwrap() {
        let got =
            im::prefill_hints(s(&c["text"]), &c["labels"], c["country"].as_str().unwrap()).unwrap();
        let msg = c["text"].to_string();
        bless::eq(&mut c["out"], hints_json(&got), msg);
    }
    let hon: Vec<String> = [
        "mr", "mrs", "ms", "miss", "mx", "dr", "prof", "sir", "madam",
    ]
    .iter()
    .map(|x| x.to_string())
    .collect();
    for c in g["clean_person"].as_array_mut().unwrap() {
        let got = im::clean_person(s(&c["name"]), &hon);
        let msg = c["name"].to_string();
        bless::eq(&mut c["out"], got, msg);
    }
    for c in g["clean_company"].as_array_mut().unwrap() {
        let got = im::clean_company(s(&c["name"]));
        let msg = c["name"].to_string();
        bless::eq(&mut c["out"], got, msg);
    }
    for c in g["similarity"].as_array_mut().unwrap() {
        let got = im::similarity(s(&c["a"]).unwrap(), s(&c["b"]).unwrap());
        let msg = format!("{} {}", c["a"], c["b"]);
        if bless::blessing() {
            c["out"] = json!(got);
        } else {
            assert_eq!(got, f(&c["out"]), "{msg}");
        }
    }
    for c in g["text_similarity"].as_array_mut().unwrap() {
        let got = im::text_similarity(s(&c["a"]), s(&c["b"]));
        let msg = format!("{} {}", c["a"], c["b"]);
        if bless::blessing() {
            c["out"] = json!(got);
        } else {
            assert_eq!(got, f(&c["out"]), "{msg}");
        }
    }
    for c in g["domain_of"].as_array_mut().unwrap() {
        let got = im::domain_of(s(&c["email"]));
        let msg = c["email"].to_string();
        bless::eq(&mut c["out"], json!(got), msg);
    }
    for c in g["tier_for"].as_array_mut().unwrap() {
        let got = im::tier_for(c["score"].as_i64().unwrap(), &c["thresholds"]);
        let msg = c["score"].to_string();
        bless::eq(&mut c["out"], json!(got), msg);
    }
    for c in g["candidate_add"].as_array().unwrap() {
        let mut cand = im::Candidate::new("contact", Uuid::from_u128(1), "x");
        for sc in c["scores"].as_array().unwrap() {
            cand.add("sig", f(sc), "d");
        }
        assert_eq!(cand.score, c["score"].as_i64().unwrap(), "{c}");
        assert_eq!(Value::Array(cand.signals.clone()), c["signals"], "{c}");
    }
    for c in g["pct"].as_array().unwrap() {
        let got = granary_compat::pyfmt::format_number(
            &granary_compat::pyfmt::PyNum::Float(f(&c["x"])),
            ".0%",
        )
        .unwrap();
        assert_eq!(got, c["out"].as_str().unwrap(), "{c}");
    }
    for c in g["classify"].as_array().unwrap() {
        assert_eq!(
            scoring::classify(c["total"].as_i64().unwrap(), &table(&c["table"])),
            c["out"].as_str().unwrap(),
            "{c}"
        );
    }
    for c in g["near_boundary"].as_array().unwrap() {
        let got = scoring::near_boundary(
            c["total"].as_i64().unwrap(),
            &table(&c["table"]),
            c["margin"].as_i64().unwrap(),
        );
        assert_eq!(got, c["out"].as_bool().unwrap(), "{c}");
    }
    for c in g["classes"].as_array().unwrap() {
        match scoring::classes_from(&c["raw"]) {
            Ok(t) => {
                let got: Value = t.iter().map(|(k, (lo, hi))| json!([k, [lo, hi]])).collect();
                assert_eq!(got, c["out"], "{c}");
            }
            Err(e) => {
                if c.get("error500").is_some() {
                    assert_eq!(e.status().as_u16(), 500, "{c}");
                } else {
                    assert_eq!(e.status().as_u16(), 400, "{c}");
                    assert_eq!(e.body()["detail"], c["error"], "{c}");
                }
            }
        }
    }
    {
        let odd = &mut g["instructions"][1];
        let st = odd["settings"].clone();
        let got = scoring::instructions_from(
            &st["nary.score.rubric"],
            &st["nary.score.red_flags"],
            &st["nary.score.green_flags"],
        );
        bless::eq(&mut odd["out"], got, "instructions[1]");
    }
    for c in g["schema"].as_array_mut().unwrap() {
        let got = scoring::schema(&strs(&c["products"]));
        bless::eq(&mut c["out"], got, "schema");
    }
    for c in g["as_context"].as_array_mut().unwrap() {
        let entries: Vec<NaryMemory> = c["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| NaryMemory {
                layer: e["layer"].as_str().unwrap().into(),
                subject_type: None,
                subject_id: None,
                title: e["title"].as_str().map(str::to_string),
                content: e["content"].as_str().unwrap().into(),
                source_type: e["source_type"].as_str().unwrap().into(),
                source_ref: None,
                author_id: None,
                authored_by_nary: false,
                trusted: e["trusted"].as_bool().unwrap(),
                status: "approved".into(),
                approved_by_id: None,
                approved_at: None,
                expires_at: None,
                id: Uuid::nil(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            })
            .collect();
        let msg = c["entries"].to_string();
        bless::eq(&mut c["out"], memory::as_context(&entries), msg);
    }
    for c in g["profiling_instructions"].as_array_mut().unwrap() {
        let got = profiling::instructions(&strs(&c["missing"]));
        let msg = c["missing"].to_string();
        bless::eq(&mut c["out"], got, msg);
    }
    // Key order too (the request body is compared byte for byte).
    bless::eq(
        &mut g["profiling_schema"],
        profiling::SCHEMA.clone(),
        "profiling_schema",
    );
    bless::eq(
        &mut g["review_schema"],
        im::REVIEW_SCHEMA.clone(),
        "review_schema",
    );
    bless::eq(
        &mut g["hints_schema"],
        im::HINTS_SCHEMA.clone(),
        "hints_schema",
    );
    bless::update("nary_identity/pure.json", |v| {
        for (k, x) in g.as_object().unwrap() {
            if !matches!(k.as_str(), "instructions" | "questions_keys") {
                v[k] = x.clone();
            }
        }
        v["instructions"][1] = g["instructions"][1].clone();
    });
}

#[tokio::test]
async fn db_golden() {
    let mut g = golden("pure.json");
    let mut c = golden("candidates.json");
    let pool = support::fresh_db("n8_identity_golden").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut sqlx::PgConnection = &mut conn;

    // scoring.instructions / _questions_keys on the seeded settings
    bless::eq(
        &mut g["instructions"][0]["out"],
        scoring::instructions(db).await.unwrap(),
        "instructions[0]",
    );
    bless::eq(
        &mut g["questions_keys"],
        Value::from(scoring::questions_keys(db).await.unwrap()),
        "questions_keys",
    );
    bless::update("nary_identity/pure.json", |v| {
        v["instructions"][0] = g["instructions"][0].clone();
        v["questions_keys"] = g["questions_keys"].clone();
    });

    // settings_bundle
    let cfg = im::settings_bundle(db).await.unwrap();
    let want = &mut c["settings_bundle"];
    bless::eq(
        &mut want["thresholds"],
        cfg.thresholds.clone(),
        "thresholds",
    );
    bless::eq(&mut want["scores"], cfg.scores.clone(), "scores");
    bless::eq(&mut want["labels"], cfg.labels.clone(), "labels");
    bless::eq(
        &mut want["honorifics"],
        Value::from(cfg.honorifics.clone()),
        "honorifics",
    );
    bless::eq(&mut want["recent_days"], cfg.recent_days, "recent_days");
    bless::eq(&mut want["country"], cfg.country.clone(), "country");

    for sql in c["scenario"].as_array().unwrap() {
        sqlx::query(sql.as_str().unwrap())
            .execute(&mut *db)
            .await
            .unwrap();
    }
    let labels = c["labels"].clone();
    for (i, case) in c["cases"].as_array_mut().unwrap().iter_mut().enumerate() {
        let ch = &case["chat"];
        let contact: Option<Uuid> = ch["contact"].as_str().map(|u| Uuid::parse_str(u).unwrap());
        let chat = sqlx::query_as::<_, WaChat>(
            "INSERT INTO wa_chats (id, chat_id, phone, display_name, status, business_profile, contact_id, is_my_contact, \
             attribution_asked) VALUES ($1, $2, $3, $4, 'active', $5, $6, false, false) RETURNING *",
        )
        .bind(Uuid::new_v4())
        .bind(format!("case{i}@c.us"))
        .bind(ch["phone"].as_str())
        .bind(ch["display_name"].as_str())
        .bind(ch.get("business_profile").cloned())
        .bind(contact)
        .fetch_one(&mut *db)
        .await
        .unwrap();
        let extra = case["extra"].as_object().map(|e| im::Hints {
            name: e["name"].as_str().map(str::to_string),
            company: e["company"].as_str().map(str::to_string),
            product: None,
            emails: strs(&e["emails"]),
            phones: strs(&e["phones"]),
        });
        let got = im::candidates_for_chat(db, &chat, case["text"].as_str(), &cfg, extra.as_ref())
            .await
            .unwrap();
        let got: Value = got
            .iter()
            .map(|cand| {
                let id = cand.target_id.to_string();
                json!({"kind": cand.kind, "target": labels.get(&id).cloned().unwrap_or(Value::String(id)),
                       "label": cand.label, "score": cand.score, "signals": cand.signals})
            })
            .collect();
        let msg = format!("case {i}: {}", case["text"]);
        bless::eq(&mut case["out"], got, msg);
    }
    bless::save("nary_identity/candidates.json", &c);
}
