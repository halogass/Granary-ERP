//! Port of backend/app/services/nary/proposal_actions.py (agent N6).
//!
//! The Phase 4 actions (Solution Proposals). Through the executor like every other action.
//!
//! ```text
//!     proposal.discovery.start   start requirement discovery on a deal (a card)
//!     proposal.create_draft      draft the proposal, its figures and cost sheet (a card)
//!     rates.propose_update       a researched rate card change (a card; never promoted)
//! ```
//! Python registers them on import; here [`actions`] returns them for the registry (actions::module_actions).

use serde_json::{json, Value};

use super::actions::{apply_fn, entity_fn, to_uuid, Action, Field, Schema, Ty};

fn text(p: &Value, k: &str) -> String {
    p.get(k)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// `DiscoveryStart(_Strict)`.
pub fn discovery_start_schema() -> Schema {
    Schema::strict(
        "DiscoveryStart",
        vec![
            Field::req("deal_id", Ty::Str),
            Field::req("reason", Ty::Str),
        ],
    )
}

/// `DraftProposal(_Strict)`.
pub fn draft_proposal_schema() -> Schema {
    Schema::strict("DraftProposal", vec![Field::req("deal_id", Ty::Str)])
}

/// `RateUpdate(_Strict)`.
pub fn rate_update_schema() -> Schema {
    Schema::strict(
        "RateUpdate",
        vec![
            Field::req("role", Ty::Str),
            Field::req("seniority", Ty::Str),
            Field::req("cost_base", Ty::Str),
            Field::req("source", Ty::Str),
            Field::req("valid_from", Ty::Str),
        ],
    )
}

/// The actions this module registers, in Python's order.
pub fn actions() -> Vec<Action> {
    let deal_entity = entity_fn(|p| Ok((Some("deal".into()), Some(to_uuid(&p["deal_id"])?))));
    vec![
        Action::new(
            "proposal.discovery.start",
            "Start requirement discovery for a Solution Proposal",
            discovery_start_schema(),
            apply_fn(|db, actor, p| {
                Box::pin(async move {
                    super::proposals::start_discovery(
                        db,
                        actor,
                        &text(p, "deal_id"),
                        &text(p, "reason"),
                    )
                    .await
                })
            }),
        )
        .permission("sales:create_quotation")
        .entity(deal_entity.clone()),
        Action::new(
            "proposal.create_draft",
            "Draft the Solution Proposal",
            draft_proposal_schema(),
            apply_fn(|db, actor, p| {
                Box::pin(
                    async move { super::proposals::draft(db, actor, &text(p, "deal_id")).await },
                )
            }),
        )
        .permission("sales:create_quotation")
        .entity(deal_entity),
        Action::new(
            "rates.propose_update",
            "Update the rate card",
            rate_update_schema(),
            apply_fn(|db, actor, p| {
                Box::pin(async move { super::rates::apply_update(db, actor, p).await })
            }),
        )
        .permission("sales:manage_costs")
        .describe(apply_fn(|_db, _actor, p| {
            Box::pin(async move { Ok(_describe_rate(p)) })
        })),
        // batch 2 T1: a Solution Proposal asked for in the chat (sales:create_quotation or the Owner, checked
        // when applied)
        Action::new(
            "proposal.from_chat",
            "Write the requirement sheet and draft the Solution Proposal",
            from_chat_schema(),
            apply_fn(|db, actor, p| {
                Box::pin(async move { super::proposals::apply_from_chat(db, actor, p).await })
            }),
        )
        .describe(apply_fn(|db, _actor, p| {
            Box::pin(async move { super::proposals::describe_from_chat(db, p).await })
        }))
        .entity(deal_entity_b2()),
        Action::new(
            "proposal.update_requirements",
            "Update a Solution Proposal's requirement sheet",
            from_chat_schema(),
            apply_fn(|db, actor, p| {
                Box::pin(async move { super::proposals::apply_update(db, actor, p).await })
            }),
        )
        .describe(apply_fn(|db, _actor, p| {
            Box::pin(async move { super::proposals::describe_update(db, p).await })
        }))
        .entity(deal_entity_b2()),
    ]
}

/// batch 2 T1: `FromChat(_Strict)`: the deal and the sheet edits (see proposals::clean_requirements).
pub fn from_chat_schema() -> Schema {
    Schema::strict(
        "ProposalFromChat",
        vec![
            Field::req("deal_id", Ty::Str),
            Field::req("requirements", Ty::Dict),
            // fix-longapply: a second proposal for a deal only when asked for
            Field::opt("new_proposal", Ty::Bool),
        ],
    )
}

fn deal_entity_b2() -> super::actions::EntityFn {
    entity_fn(|p| Ok((Some("deal".into()), Some(to_uuid(&p["deal_id"])?))))
}

/// `_describe_rate`.
pub fn _describe_rate(p: &Value) -> Value {
    json!({"label": format!("{} ({}): {} per man-day", text(p, "role"), text(p, "seniority"), text(p, "cost_base")),
           "after": {"source": p.get("source").cloned().unwrap_or(Value::Null)}})
}
