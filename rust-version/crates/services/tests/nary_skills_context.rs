//! The tests of services/nary/{skills,context,persona}.py: test_nary_learns.py (skills, tool ideas; the
//! catalog handlers _h_propose_skill / _h_use_skill / _h_propose_tool_idea are thin wrappers, called here as
//! the functions they wrap), test_nary_p1_nary.py (context walls), test_nary_p3_playbooks.py (scrub) and
//! test_ai_prompt_cache.py (the persona prompt's cache break).

mod nary_chat_common;

use rust_decimal::Decimal;
use serde_json::json;
use uuid::Uuid;

use granary_services::ai::base::CACHE_BREAK;
use granary_services::nary::{context, memory, persona, skills};
use nary_chat_common as c;

const TAG: &str = "n3_skills_context";

// ─── Skills ───────────────────────────────────────────────────────────────────

#[test]
fn test_a_skill_waits_for_the_owner_then_is_listed_and_readable() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let me = c::make_person(&mut db, true, &[]).await;
        let tag = c::u("pytest-learns");
        let title = format!("Quote a white-label reseller {tag}");
        let entry = skills::propose(
            &mut db,
            Some(&me),
            &title,
            "A partner asks for white-label pricing",
            "1. search_catalog the parent\n2. propose_quotation_draft",
            Some("Never quote the tier"),
            Some("the Owner's correction"),
        )
        .await
        .unwrap();
        assert_eq!(entry.status, "proposed");
        assert!(!skills::prompt_list(&mut db).await.unwrap().contains(&title)); // not used before approval
        assert_eq!(entry.subject_type.as_deref(), Some("skill"));
        assert_eq!(entry.layer, "company");
        // the same title again updates the waiting proposal
        let again = skills::propose(
            &mut db,
            Some(&me),
            &title.to_uppercase(),
            "A partner asks for white-label pricing",
            "1. search_catalog the parent\n2. propose_quotation_draft",
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(again.id, entry.id);
        assert!(!again.content.contains("Watch out"));
        memory::decide(&mut db, entry.id, me.id, "approve")
            .await
            .unwrap();
        let listed = skills::prompt_list(&mut db).await.unwrap();
        assert!(
            listed.contains(&title) && listed.contains("A partner asks for white-label pricing")
        );
        let got = skills::find(&mut db, &format!("white-label reseller {tag}"))
            .await
            .unwrap();
        assert_eq!(got["title"], json!(title));
        assert!(got["procedure"]
            .as_str()
            .unwrap()
            .contains("propose_quotation_draft"));
    });
}

#[test]
fn test_the_prompt_lists_at_most_the_configured_number_of_skills() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let me = c::make_person(&mut db, true, &[]).await;
        let tag = c::u("pytest-learns");
        for i in 0..3 {
            let e = skills::propose(
                &mut db,
                Some(&me),
                &format!("Skill {i} {tag}"),
                &format!("case {i}"),
                "1. do it",
                None,
                None,
            )
            .await
            .unwrap();
            memory::decide(&mut db, e.id, me.id, "approve")
                .await
                .unwrap();
        }
        let old = c::set_settings(&mut db, &[("nary.skills.max_in_prompt", "2")]).await;
        let listed = skills::prompt_list(&mut db).await.unwrap();
        c::restore(&mut db, old).await;
        assert_eq!(listed.lines().filter(|l| l.contains(&tag)).count(), 2);
    });
}

#[test]
fn test_a_skill_needs_a_title_when_and_steps() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let err = skills::propose(&mut db, None, "  ", "w", "s", None, None)
            .await
            .unwrap_err();
        assert_eq!(err.status().as_u16(), 400);
        assert!(format!("{err:?}").contains("A skill needs a title, when to use it, and its steps"));
        assert_eq!(
            skills::find(&mut db, "nothing like this zzzq")
                .await
                .unwrap()["error"]
                .as_str()
                .map(|s| s.starts_with("No approved skill")),
            Some(true)
        );
    });
}

// ─── Tool ideas ───────────────────────────────────────────────────────────────

#[test]
fn test_a_tool_idea_goes_to_the_rnd_backlog_once() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let me = c::make_person(&mut db, true, &[]).await;
        let tag = c::u("pytest-learns");
        let title = format!("Attach a file to an email {tag}");
        let old = c::set_settings(&mut db, &[("nary.skills.tool_ideas_project", "")]).await;
        let err = skills::file_tool_idea(
            &mut db,
            Some(&me),
            &title,
            "The Owner asked to send the integration pack",
            "Attach a document from the ERP to a Nary email",
            Some("send the pack"),
        )
        .await
        .unwrap_err();
        // no project set yet: a clear error (SettingError, a 400)
        assert_eq!(err.status().as_u16(), 400);
        assert!(format!("{err:?}").contains("Setting 'nary.skills.tool_ideas_project' is not set"));
        let project = format!("Nary tools {tag}");
        sqlx::query(
            "INSERT INTO rnd_projects (id, name, code, is_deleted) VALUES ($1, $2, $3, false)",
        )
        .bind(Uuid::new_v4())
        .bind(&project)
        .bind(format!(
            "RND-{}",
            Uuid::new_v4().simple().to_string()[..8].to_uppercase()
        ))
        .execute(&mut *db)
        .await
        .unwrap();
        c::restore(&mut db, old).await;
        let old = c::set_settings(&mut db, &[("nary.skills.tool_ideas_project", &project)]).await;
        let first = skills::file_tool_idea(
            &mut db,
            Some(&me),
            &title,
            "The Owner asked to send the integration pack",
            "Attach a document from the ERP to a Nary email",
            Some("send the pack"),
        )
        .await
        .unwrap();
        let again = skills::file_tool_idea(
            &mut db,
            Some(&me),
            &title,
            "The Owner asked to send the integration pack",
            "Attach a document from the ERP to a Nary email",
            Some("attach the brochure"),
        )
        .await
        .unwrap();
        c::restore(&mut db, old).await;
        assert_eq!(first["status"], "filed");
        assert_eq!(again["status"], "added to the open idea");
        let rows: Vec<(Option<String>, Option<serde_json::Value>, Option<String>)> =
            sqlx::query_as(
                "SELECT item_type, labels, description FROM rnd_backlog_items WHERE title = $1",
            )
            .bind(&title)
            .fetch_all(&mut *db)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.as_deref(), Some("internal_tool"));
        assert!(rows[0]
            .1
            .as_ref()
            .unwrap()
            .as_array()
            .unwrap()
            .contains(&json!("nary-idea")));
        assert!(rows[0]
            .2
            .as_deref()
            .unwrap()
            .contains("attach the brochure"));
    });
}

// ─── Context walls ────────────────────────────────────────────────────────────

#[test]
fn test_a_customer_context_gets_only_its_own_tools_and_data() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let org = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO organizations (id, name, is_deleted, custom) VALUES ($1, $2, false, '{}')",
        )
        .bind(org)
        .bind(c::u("Wall Org"))
        .execute(&mut *db)
        .await
        .unwrap();
        let (contact, stranger) = (Uuid::new_v4(), Uuid::new_v4());
        for (id, name, o) in [
            (contact, c::u("Rachel"), Some(org)),
            (stranger, c::u("Other"), None),
        ] {
            sqlx::query("INSERT INTO contacts (id, first_name, organization_id, is_deleted, custom) VALUES ($1, $2, $3, false, '{}')")
                .bind(id).bind(name).bind(o).execute(&mut *db).await.unwrap();
        }
        let ours = c::u("Ours");
        for (title, cid, o) in [
            (ours.clone(), contact, Some(org)),
            (c::u("Theirs"), stranger, None),
        ] {
            sqlx::query("INSERT INTO deals (id, title, stage, contact_id, organization_id, is_deleted, custom) VALUES ($1, $2, 'proposal', $3, $4, false, '{}')")
                .bind(Uuid::new_v4()).bind(title).bind(cid).bind(o).execute(&mut *db).await.unwrap();
        }
        let product = Uuid::new_v4();
        let sku = c::u("WALL");
        sqlx::query(
            "INSERT INTO product_catalog (id, sku, name, product_type, unit, cost_price, description, is_active, sale_model, \
             price_metric, modules, pricing_mode, addon_for, is_whitelabel, is_orderable, is_deleted) VALUES \
             ($1, $2, $3, 'physical', 'Unit', $4, 'An Laptop', true, 'one_time', 'unit', '{}', 'list', '{}', false, true, false)",
        )
        .bind(product).bind(&sku).bind(c::u("Wall Box")).bind(Decimal::new(12345, 2))
        .execute(&mut *db).await.unwrap();
        sqlx::query("INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) VALUES ($1, $2, 'JPY', 1000000, '2020-01-01')")
            .bind(Uuid::new_v4()).bind(product).execute(&mut *db).await.unwrap();
        sqlx::query("INSERT INTO catalog_volume_tiers (id, product_id, min_qty, max_qty, discount_pct) VALUES ($1, $2, 10, NULL, 5)")
            .bind(Uuid::new_v4()).bind(product).execute(&mut *db).await.unwrap();

        let ctx = context::customer_context(contact);
        let tools = context::tools_for(&ctx, None).unwrap();
        assert_eq!(
            tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            context::CUSTOMER_TOOL_NAMES
        );
        let admin = c::make_person(&mut db, true, &[]).await;
        let internal: Vec<String> = context::tools_for(context::INTERNAL, Some(&admin))
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert!(internal.iter().any(|n| n == "search_records"));
        assert!(!internal
            .iter()
            .any(|n| context::CUSTOMER_TOOL_NAMES.contains(&n.as_str())));

        let recs = context::run_customer_tool(&mut db, &ctx, "customer_records", Some(json!({})))
            .await
            .unwrap();
        assert_eq!(
            recs["projects"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["title"].clone())
                .collect::<Vec<_>>(),
            [json!(ours)]
        );
        let cat = context::run_customer_tool(
            &mut db,
            &ctx,
            "catalog_public",
            Some(json!({"query": sku, "currency": "JPY"})),
        )
        .await
        .unwrap();
        // JPY: no decimals
        assert_eq!(
            cat["products"][0]["list_price"],
            json!({"currency": "JPY", "list_price": "1000000"})
        );
        let blob = serde_json::to_string(&cat).unwrap();
        assert!(!blob.contains("123.45") && !blob.contains("cost") && !blob.contains("tier"));
        let refused = context::run_customer_tool(
            &mut db,
            &ctx,
            "search_records",
            Some(json!({"object_type": "deal"})),
        )
        .await
        .unwrap();
        assert!(refused["error"].as_str().unwrap().contains("not available"));
    });
}

#[test]
fn test_scrub_removes_forbidden_keys_at_any_depth() {
    let out = context::scrub(
        &json!({"a": 1, "cost_price": 5, "lines": [{"tier_pct": 5, "name": "x"}], "notes": "internal"}),
    );
    assert_eq!(out, json!({"a": 1, "lines": [{"name": "x"}]}));
}

#[test]
fn test_customer_tools_never_return_internal_numbers() {
    for k in ["margin", "tier_available_pct", "cost_price", "tiers"] {
        assert!(context::FORBIDDEN_KEYS.contains(&k));
    }
    assert_eq!(
        context::scrub(
            &json!({"a": 1, "margins": {"lines": [{"cost_price": 1, "margin": 2, "tier_available_pct": 3, "x": 4}]}})
        ),
        json!({"a": 1, "margins": {"lines": [{"x": 4}]}})
    );
}

// ─── Persona ──────────────────────────────────────────────────────────────────

#[test]
fn test_nary_prompts_put_the_per_call_rules_after_the_cache_break() {
    let system =
        persona::system_prompt("customer", "", "WRITE LIKE THE Owner", "ask about laptops");
    let (stable, dynamic) = system.split_once(CACHE_BREAK).unwrap();
    assert!(stable.contains(persona::IDENTITY) && stable.contains("WRITE LIKE THE Owner"));
    assert_eq!(dynamic, "ask about laptops");
    assert!(!persona::system_prompt("staff", "fixed job rules", "", "").contains(CACHE_BREAK));
    // nothing per call: one block
}

#[test]
fn test_the_style_guide_is_the_newest_approved_one() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        sqlx::query("DELETE FROM nary_memory WHERE title = $1")
            .bind(persona::STYLE_GUIDE_TITLE)
            .execute(&mut *db)
            .await
            .unwrap();
        assert_eq!(
            persona::style_guide(&mut db, Some("whatsapp"))
                .await
                .unwrap(),
            ""
        );
        for (content, status, approved) in [
            (
                "## On WhatsApp\nOld.\n## By email\nOld mail.",
                "approved",
                "2026-01-01T00:00:00+00:00",
            ),
            (
                "What changed: newer\n## On WhatsApp\nShort lines.\n## By email\nFormal.",
                "approved",
                "2026-02-01T00:00:00+00:00",
            ),
            (
                "## On WhatsApp\nProposed only.",
                "proposed",
                "2026-03-01T00:00:00+00:00",
            ),
        ] {
            sqlx::query("INSERT INTO nary_memory (id, layer, title, content, source_type, status, approved_at, authored_by_nary, trusted) \
                         VALUES ($1, 'company', $2, $3, 'nary', $4, $5::timestamptz, true, true)")
                .bind(Uuid::new_v4()).bind(persona::STYLE_GUIDE_TITLE).bind(content).bind(status).bind(approved)
                .execute(&mut *db).await.unwrap();
        }
        assert_eq!(
            persona::style_guide(&mut db, Some("whatsapp"))
                .await
                .unwrap(),
            "WRITE LIKE THE Owner (style guide):\n## On WhatsApp\nShort lines."
        );
        assert_eq!(
            persona::style_guide(&mut db, Some("email")).await.unwrap(),
            "WRITE LIKE THE Owner (style guide):\n## By email\nFormal."
        );
        assert!(persona::style_guide_text(&mut db)
            .await
            .unwrap()
            .starts_with("What changed: newer"));
        sqlx::query("DELETE FROM nary_memory WHERE title = $1")
            .bind(persona::STYLE_GUIDE_TITLE)
            .execute(&mut *db)
            .await
            .unwrap();
    });
}

/// Batch E: while nary.skills.tool_ideas_project is unset, propose_tool_idea can only fail, so the tool lists
/// (chat and MCP) leave it out; once a project is named it is offered again.
#[test]
fn test_propose_tool_idea_is_hidden_until_a_project_is_set() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let me = c::make_person(&mut db, true, &[]).await;
        let names = |t: &[granary_services::ai::tools::Tool]| {
            t.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
        };
        let old = c::set_settings(&mut db, &[("nary.skills.tool_ideas_project", "  ")]).await;
        let mut tools = granary_services::ai::tools::tools_for_user(&me);
        assert!(names(&tools).contains(&"propose_tool_idea".to_string()));
        skills::drop_unconfigured_tools(&mut db, &mut tools)
            .await
            .unwrap();
        let hidden = !names(&tools).contains(&"propose_tool_idea".to_string());
        assert!(names(&tools).contains(&"propose_skill".to_string()));
        c::restore(&mut db, old).await;
        let old =
            c::set_settings(&mut db, &[("nary.skills.tool_ideas_project", "Nary tools")]).await;
        let mut tools = granary_services::ai::tools::tools_for_user(&me);
        skills::drop_unconfigured_tools(&mut db, &mut tools)
            .await
            .unwrap();
        let shown = names(&tools).contains(&"propose_tool_idea".to_string());
        c::restore(&mut db, old).await;
        assert!(hidden, "propose_tool_idea offered with no project set");
        assert!(shown, "propose_tool_idea hidden with a project set");
    });
}
