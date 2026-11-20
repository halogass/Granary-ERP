//! research (core, site, company, contact) against compat-tests/golden/research/cases.json, recorded from
//! the REAL backend modules by compat-tests/golden/gen_research.py: the grounding and site-picking
//! helpers, the prompts company.deep / contact.research send, and how a model answer is parsed.
//! Plus the pure tests of tests/test_crm_research.py (grounding helpers, page picking, official site).
//! The recorded outputs re-record with `GOLDEN_BLESS=1` (tests/golden/bless.rs); bless research_crm first,
//! because the contact prompts here read the pages its `fetched` lists name.

#[path = "golden/bless.rs"]
mod bless;

use granary_db::rows::crm::{Contact, Organization};
use granary_services::ai::enrichment::{self, EnrichSettings};
use granary_services::research::{company, contact, core_ as core, site};
use serde_json::{json, Value};

const REL: &str = "research/cases.json";

fn golden() -> Value {
    bless::read(REL)
}

/// Compares `got` with the recorded value, or records it when blessing.
fn chk(slot: &mut Value, got: impl Into<Value>, msg: impl std::fmt::Display) {
    bless::eq(slot, got, msg);
}

/// When blessing, writes these sections of `g` back to the golden file.
fn save(g: &Value, keys: &[&str]) {
    bless::update(REL, |v| {
        for k in keys {
            v[*k] = g[*k].clone();
        }
    });
}

fn each(g: &mut Value, key: &str, mut f: impl FnMut(&mut Vec<Value>)) {
    for c in g[key].as_array_mut().unwrap() {
        f(c.as_array_mut().unwrap());
    }
}

fn arr(v: &Value) -> &Vec<Value> {
    v.as_array().unwrap()
}

fn s(v: &Value) -> Option<&str> {
    v.as_str()
}

fn strings(v: &Value) -> Vec<String> {
    arr(v)
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

// ─── the Python tests ─────────────────────────────────────────────────────────

#[test]
fn test_names_are_matched_without_titles_and_degrees() {
    assert_eq!(
        core::normalise_name(Some("Dr. Bob Sanders, M.M.")),
        "bob sanders"
    );
    assert!(core::name_in(
        "Bob Sanders",
        &core::normalise_name(Some("\u{2026}Mr Bob  Sanders serves as\u{2026}"))
    ));
    assert!(!core::name_in(
        "Bob Harper",
        &core::normalise_name(Some("Bob Sanders"))
    ));
}

#[test]
fn test_the_right_pages_of_the_company_site_are_read() {
    let links = json!([
        {"text": "Home", "href": "https://northwindsteel.example/"},
        {"text": "Products", "href": "https://northwindsteel.example/products"},
        {"text": "Contact Us", "href": "https://northwindsteel.example/contact"},
        {"text": "Board of Directors", "href": "https://www.northwindsteel.example/about/board-of-directors"},
        {"text": "Annual Report", "href": "https://northwindsteel.example/about/annual-report.pdf"},
        {"text": "Risk Management", "href": "https://northwindsteel.example/en/governance/risk-management"},
        {"text": "About", "href": "https://other-site.example/about"},
    ]);
    let picked = site::pick_pages(arr(&links), "https://northwindsteel.example/", 5);
    assert!(
        picked[0].ends_with("/about/board-of-directors"),
        "the management page comes first"
    );
    assert!(picked.contains(&"https://northwindsteel.example/contact".to_string()));
    assert!(!picked.iter().any(|p| p.ends_with(".pdf")
        || p.contains("other-site")
        || p.ends_with("/products")
        || p.contains("risk")));

    let results = json!([{"url": "https://www.linkedin.com/company/northwind-steel"},
                         {"url": "https://www.exchange.example/northwind"},
                         {"url": "https://www.northwindsteel.example/en/"}]);
    assert_eq!(
        site::official_site("Northwind Steel Inc", None, arr(&results)).as_deref(),
        Some("https://www.northwindsteel.example/")
    );
    assert_eq!(
        site::official_site(
            "Northwind Steel Inc",
            Some("northwindsteel.com"),
            arr(&results)
        )
        .as_deref(),
        Some("https://northwindsteel.com")
    );
}

#[test]
fn test_an_abbreviated_domain_is_recognised_from_the_ticker() {
    let results = json!([
        {"url": "https://investor.glxc.example/newsroom/x.pdf", "title": "[PDF] Company Profile - Investor Relations - glxc"},
        {"url": "https://www.exchange.example/listed-companies/x", "title": "GLXC - Exchange"},
        {"url": "https://finance.yahoo.com/quote/GLXC.T/profile/", "title": "Globex Construction Corp (GLXC.T) Company Profile"},
        {"url": "https://www.findirectory.example/glxc/globex-construction", "title": "Globex Construction Corp [GLXC] - FinDirectory"},
        {"url": "https://investor.glxc.example/", "title": "Globex Construction Corp - Investor Relations: Welcome"},
    ]);
    assert_eq!(
        site::official_site("Globex Construction Corp", None, arr(&results)).as_deref(),
        Some("https://www.glxc.example/")
    );
    assert!(site::_is_portal("jp.linkedin.com") && !site::_is_portal("fedex.com"));
    let one = json!([{"url": "https://www.exchange.example/x", "title": "Prosper Holdings"}]);
    assert_eq!(
        site::official_site("Prosper Holdings Ltd", None, arr(&one)),
        None
    );
}

// ─── golden: site ─────────────────────────────────────────────────────────────

#[test]
fn golden_site_helpers() {
    let mut g = golden();
    each(&mut g, "urlsplit", |c| {
        let p = site::urlsplit(s(&c[0]).unwrap());
        let got = json!([p.scheme, p.netloc, p.path, p.hostname()]);
        let msg = format!("urlsplit {:?}", c[0]);
        chk(&mut c[1], got, msg);
    });
    each(&mut g, "domain_of", |c| {
        let got = site::domain_of(s(&c[0]));
        let msg = format!("domain_of {:?}", c[0]);
        chk(&mut c[1], got, msg);
    });
    each(&mut g, "registrable", |c| {
        let got = site::registrable(s(&c[0]).unwrap());
        let msg = format!("registrable {:?}", c[0]);
        chk(&mut c[1], got, msg);
    });
    each(&mut g, "is_portal", |c| {
        let got = site::_is_portal(s(&c[0]).unwrap());
        let msg = format!("is_portal {:?}", c[0]);
        chk(&mut c[1], got, msg);
    });
    each(&mut g, "name_tokens", |c| {
        let tokens = site::_name_tokens(s(&c[0]).unwrap());
        let mut ab = site::_abbreviations(&tokens);
        ab.sort();
        let msg = format!("tokens {:?}", c[0]);
        chk(&mut c[1], tokens, &msg);
        chk(&mut c[2], ab, format!("abbreviations {msg}"));
    });
    each(&mut g, "official_site", |c| {
        let got = site::official_site(s(&c[0]).unwrap(), s(&c[1]), arr(&c[2]));
        let msg = format!("official_site {:?}", c);
        chk(&mut c[3], got, msg);
    });
    each(&mut g, "pick_pages", |c| {
        let got = site::pick_pages(
            arr(&c[0]),
            s(&c[1]).unwrap(),
            c[2].as_u64().unwrap() as usize,
        );
        let msg = format!("pick_pages {:?}", c[1]);
        chk(&mut c[3], got, msg);
    });
    each(&mut g, "pages_block", |c| {
        let got = site::pages_block(arr(&c[0]), c[1].as_u64().unwrap() as usize);
        let msg = format!("pages_block budget {}", c[1]);
        chk(&mut c[2], got, msg);
    });
    save(
        &g,
        &[
            "urlsplit",
            "domain_of",
            "registrable",
            "is_portal",
            "name_tokens",
            "official_site",
            "pick_pages",
            "pages_block",
        ],
    );
}

// ─── golden: core ─────────────────────────────────────────────────────────────

#[test]
fn golden_core_helpers() {
    let mut g = golden();
    each(&mut g, "normalise_name", |c| {
        let got = core::normalise_name(s(&c[0]));
        let msg = format!("normalise {:?}", c[0]);
        chk(&mut c[1], got, msg);
    });
    each(&mut g, "name_in", |c| {
        let norm = core::normalise_name(s(&c[1]));
        let got = core::name_in(s(&c[0]).unwrap(), &norm);
        let msg = format!("name_in {:?}", c);
        chk(&mut c[2], got, msg);
    });
    each(&mut g, "phone_in", |c| {
        let digits = core::digits(s(&c[0]));
        let got = core::phone_in(s(&c[0]), s(&c[1]).unwrap());
        let msg = format!("phone_in {:?}", c);
        chk(&mut c[2], digits, &msg);
        chk(&mut c[3], got, msg);
    });
    each(&mut g, "email_in", |c| {
        let got = core::email_in(s(&c[0]), s(&c[1]).unwrap());
        let msg = format!("email_in {:?}", c);
        chk(&mut c[2], got, msg);
    });
    let results = arr(&g["sources"][0][0]).clone();
    let pages = arr(&g["sources"][0][1]).clone();
    each(&mut g, "url_seen", |c| {
        let got = core::url_seen(s(&c[0]), &results, &pages);
        let msg = format!("url_seen {:?}", c[0]);
        chk(&mut c[1], got, msg);
    });
    each(&mut g, "sources", |c| {
        let got = Value::Array(core::sources(arr(&c[0]), arr(&c[1]), &strings(&c[2]), 8));
        let msg = format!("sources {:?}", c[2]);
        chk(&mut c[3], got, msg);
    });
    each(&mut g, "results_block", |c| {
        let got = core::results_block(arr(&c[0]), c[1].as_u64().unwrap() as usize);
        chk(&mut c[2], got, "results_block");
    });
    each(&mut g, "haystack", |c| {
        let got = core::haystack(arr(&c[0]), arr(&c[1]));
        chk(&mut c[2], got, "haystack");
    });
    save(
        &g,
        &[
            "normalise_name",
            "name_in",
            "phone_in",
            "email_in",
            "url_seen",
            "sources",
            "results_block",
            "haystack",
        ],
    );
}

// ─── golden: answer parsing ───────────────────────────────────────────────────

#[test]
fn golden_answer_parsing() {
    let mut g = golden();
    let results = arr(&g["sources"][0][0]).clone();
    let pages = arr(&g["sources"][0][1]).clone();
    each(&mut g, "company_news", |c| {
        let got = Value::Array(company::_news(&c[0], &results).unwrap());
        let msg = format!("news {:?}", c[0]);
        chk(&mut c[1], got, msg);
    });
    let material = core::haystack(&results, &pages);
    each(&mut g, "company_channels", |c| {
        let got = Value::Object(company::_channels(&c[0], &material, &results, &pages).unwrap());
        let msg = format!("channels {:?}", c[0]);
        chk(&mut c[1], got, msg);
    });
    each(&mut g, "company_summary", |c| {
        let text = if c[0].is_null() { None } else { Some(&c[0]) };
        let got = company::_summary(text, s(&c[1]));
        let msg = format!("summary {:?}", c);
        chk(&mut c[2], got, msg);
    });
    let mut contact_results = results.clone();
    contact_results.push(json!({"title": "Andy Wilson - Procurement Manager - Northwind Steel | LinkedIn",
        "url": "https://jp.linkedin.com/in/andy-wilson-123", "snippet": "Procurement Manager at Northwind Steel, Osaka"}));
    each(&mut g, "contact_grounded", |c| {
        let got = Value::Object(contact::_grounded(&c[0], &contact_results, &pages).unwrap());
        let msg = format!("grounded {:?}", c[0]);
        chk(&mut c[1], got, msg);
    });
    save(
        &g,
        &[
            "company_news",
            "company_channels",
            "company_summary",
            "contact_grounded",
        ],
    );
}

// ─── golden: prompts ──────────────────────────────────────────────────────────

fn org_from(v: &Value) -> Organization {
    let mut o = json!({"custom": {}, "id": uuid::Uuid::nil(), "created_at": "2026-10-15T00:00:00Z",
                       "updated_at": "2026-10-15T00:00:00Z", "is_deleted": false});
    for (k, x) in v.as_object().unwrap() {
        o[k] = x.clone();
    }
    serde_json::from_value(o).unwrap()
}

fn contact_from(v: &Value, org: Option<&Organization>) -> Contact {
    let mut o = json!({"custom": {}, "id": uuid::Uuid::nil(), "created_at": "2026-10-15T00:00:00Z",
                       "updated_at": "2026-10-15T00:00:00Z", "is_deleted": false,
                       "organization_id": org.map(|o| o.id)});
    for (k, x) in v.as_object().unwrap() {
        o[k] = x.clone();
    }
    serde_json::from_value(o).unwrap()
}

fn enrich_cfg(v: &Value) -> EnrichSettings {
    EnrichSettings {
        on_create: v["on_create"].as_bool().unwrap(),
        fields: v["fields"].as_str().unwrap().to_string(),
        min_confidence: v["min_confidence"].as_i64().unwrap(),
        refresh_days: v["refresh_days"].as_i64().unwrap(),
        daily_limit: v["daily_limit"].as_i64().unwrap(),
    }
}

#[test]
fn golden_company_prompts() {
    let mut g = golden();
    for c in g["company_prompts"].as_array_mut().unwrap() {
        let org = org_from(&c["org"]);
        let msg = format!("{:?}", c["org"]);
        chk(
            &mut c["queries"],
            company::_queries(&org),
            format!("queries {msg}"),
        );
        // what read_site returned in the scenario: the pages, unless the service is off or failed
        let pages: Vec<Value> = if c["configured"].as_bool().unwrap()
            && c["site_error"].is_null()
            && !c["home"].is_null()
        {
            arr(&c["pages"]).clone()
        } else {
            Vec::new()
        };
        let cfg = enrich_cfg(&c["cfg"]);
        let targets: Vec<String> = enrichment::wanted_fields(&cfg, None)
            .into_iter()
            .filter(|f| enrichment::_empty(&org, f))
            .collect();
        let prompt = company::build_prompt(&org, &targets, arr(&c["results"]), &pages);
        chk(&mut c["prompt"], prompt, format!("prompt {msg}"));
        chk(&mut c["system"], company::_SYSTEM, "system");
        // the home page research would read
        let home = site::official_site(&org.name, org.website.as_deref(), arr(&c["results"]));
        if c["configured"].as_bool().unwrap() {
            chk(&mut c["home"], home, format!("home {msg}"));
        }
    }
    save(&g, &["company_prompts"]);
}

#[test]
fn golden_contact_prompts() {
    let mut g = golden();
    let by_url = g["contact_pages_by_url"].clone();
    for c in g["contact_prompts"].as_array_mut().unwrap() {
        let org = if c["org"].is_null() {
            None
        } else {
            Some(org_from(&c["org"]))
        };
        let ct = contact_from(&c["contact"], org.as_ref());
        let name = contact::_full_name(&ct);
        if let Some(err) = c.get("error") {
            assert_eq!(
                err.as_str().unwrap(),
                "This contact has no name to research.",
            );
            assert!(
                granary_compat::text::pystr::split(&core::normalise_name(Some(&name))).is_empty()
            );
            continue;
        }
        let msg = format!("{:?}", c["contact"]);
        let company_name = org.as_ref().map(|o| o.name.as_str());
        chk(
            &mut c["queries"],
            contact::_queries(&name, company_name, ct.position.as_deref()),
            format!("queries {msg}"),
        );
        // the pages _read_mentions read (fetched in this order; errors skipped)
        let pages: Vec<Value> = arr(&c["fetched"])
            .iter()
            .filter_map(|u| {
                by_url
                    .get(u.as_str().unwrap())
                    .filter(|p| p.is_object())
                    .cloned()
            })
            .collect();
        let targets: Vec<&str> = contact::FIELDS
            .iter()
            .map(|(f, _)| *f)
            .filter(|f| enrichment::_empty(&ct, f))
            .collect();
        let prompt = contact::build_prompt(
            &name,
            company_name,
            org.as_ref().and_then(|o| o.website.as_deref()),
            &ct,
            &targets,
            arr(&c["results"]),
            &pages,
        );
        chk(&mut c["prompt"], prompt, format!("prompt {msg}"));
        chk(&mut c["system"], contact::_SYSTEM, "system");
    }
    save(&g, &["contact_prompts"]);
}
