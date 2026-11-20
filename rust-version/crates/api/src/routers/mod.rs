//! Router registry: one module per file in backend/app/routers/, registered in main.py's order with the
//! same prefixes and the same INTERNAL_ONLY gate.
//!
//! Notes for porting routes:
//! - Write paths absolute with [`at`]; a module may share its prefix with others (sales/sales_documents,
//!   crm/crm_records, finance/assets/budget, vendor/p2p, sign/signing_flow, nary/nary_playbooks).
//! - axum refuses two routes with the same path and method (Starlette lets the first one win) and two
//!   different parameter names at the same position (`/{id}` and `/{lead_id}` under one prefix):
//!   use one name per position across the modules of a prefix.
//! - Static segments beat parameters in axum regardless of order; Starlette goes by registration order.
//!   Check any Python route that relies on being registered before a parameterised sibling.

use axum::Router;

use granary_core::AppState;

pub mod ai;
pub mod ai_knowledge;
pub mod approvals;
pub mod assets;
pub mod attachments;
pub mod auth;
pub mod budget;
pub mod calls;
pub mod collab;
pub mod crm;
pub mod crm_followups;
pub mod crm_inbox;
pub mod crm_records;
pub mod crm_research;
pub mod delivery;
pub mod email_client;
pub mod email_image_senders; // security triage 18
pub mod email_signature;
pub mod face_attendance;
pub mod finance;
pub mod fleet;
pub mod helpdesk;
pub mod hr;
pub mod inventory;
pub mod knowledge;
pub mod knowledge_documents; // doclib A1
pub mod knowledge_documents_copies; // doclib A2
pub mod maintenance;
pub mod manufacturing;
pub mod marketing;
pub mod mcp;
pub mod mcp_oauth;
pub mod messaging;
pub mod nary;
pub mod nary_playbooks;
pub mod note_templates;
pub mod p2p;
pub mod partner;
pub mod portal_auth;
pub mod product_catalog;
pub mod projects;
pub mod public_drop; // 5 Nov 2026: one-time upload links, the public drop routes (token only)
pub mod rnd;
pub mod roles;
pub mod sales;
pub mod sales_documents;
pub mod scheduled_messages;
pub mod signing;
pub mod signing_flow;
pub mod system;
pub mod totp; // 2 Nov 2026: TOTP two-factor sign-in (profile setup, admin reset)
pub mod users;
pub mod vendor;
pub mod verification;
pub mod whatsapp;

/// `{prefix}{path}`.
pub fn at(prefix: &str, path: &str) -> String {
    format!("{prefix}{path}")
}

/// One `app.include_router(...)` line of main.py.
pub struct Mount {
    pub module: &'static str,
    pub prefix: &'static str,
    pub internal_only: bool,
    pub router: fn() -> Router<AppState>,
}

/// Every `include_router` call, in order.
pub fn mounts() -> Vec<Mount> {
    vec![
        Mount {
            module: "auth",
            prefix: auth::PREFIX,
            internal_only: false,
            router: auth::router,
        },
        Mount {
            module: "system",
            prefix: system::PREFIX,
            internal_only: false,
            router: system::router,
        },
        Mount {
            module: "users",
            prefix: users::PREFIX,
            internal_only: false,
            router: users::router,
        },
        Mount {
            module: "roles",
            prefix: roles::PREFIX,
            internal_only: false,
            router: roles::router,
        },
        Mount {
            module: "crm",
            prefix: crm::PREFIX,
            internal_only: true,
            router: crm::router,
        },
        Mount {
            module: "sales",
            prefix: sales::PREFIX,
            internal_only: true,
            router: sales::router,
        },
        Mount {
            module: "sales_documents",
            prefix: sales_documents::PREFIX,
            internal_only: true,
            router: sales_documents::router,
        },
        Mount {
            module: "finance",
            prefix: finance::PREFIX,
            internal_only: true,
            router: finance::router,
        },
        Mount {
            module: "projects",
            prefix: projects::PREFIX,
            internal_only: true,
            router: projects::router,
        },
        Mount {
            module: "hr",
            prefix: hr::PREFIX,
            internal_only: false,
            router: hr::router,
        },
        Mount {
            module: "face_attendance",
            prefix: face_attendance::PREFIX,
            internal_only: false,
            router: face_attendance::router,
        },
        Mount {
            module: "crm_followups",
            prefix: crm_followups::PREFIX,
            internal_only: true,
            router: crm_followups::router,
        },
        Mount {
            module: "crm_research",
            prefix: crm_research::PREFIX,
            internal_only: true,
            router: crm_research::router,
        },
        Mount {
            module: "ai_knowledge",
            prefix: ai_knowledge::PREFIX,
            internal_only: true,
            router: ai_knowledge::router,
        },
        Mount {
            module: "marketing",
            prefix: marketing::PREFIX,
            internal_only: false,
            router: marketing::router,
        },
        Mount {
            module: "manufacturing",
            prefix: manufacturing::PREFIX,
            internal_only: true,
            router: manufacturing::router,
        },
        Mount {
            module: "partner",
            prefix: partner::PREFIX,
            internal_only: false,
            router: partner::router,
        },
        Mount {
            module: "vendor",
            prefix: vendor::PREFIX,
            internal_only: false,
            router: vendor::router,
        },
        Mount {
            module: "knowledge",
            prefix: knowledge::PREFIX,
            internal_only: false,
            router: knowledge::router,
        },
        Mount {
            module: "whatsapp",
            prefix: whatsapp::PREFIX,
            internal_only: false,
            router: whatsapp::router,
        },
        Mount {
            module: "messaging",
            prefix: messaging::PREFIX,
            internal_only: false,
            router: messaging::router,
        },
        Mount {
            module: "email_client",
            prefix: email_client::PREFIX,
            internal_only: true,
            router: email_client::router,
        },
        Mount {
            module: "email_image_senders",
            prefix: email_image_senders::PREFIX,
            internal_only: true,
            router: email_image_senders::router,
        },
        Mount {
            module: "email_signature",
            prefix: email_signature::PREFIX,
            internal_only: false,
            router: email_signature::router,
        },
        Mount {
            module: "calls",
            prefix: calls::PREFIX,
            internal_only: false,
            router: calls::router,
        },
        Mount {
            module: "rnd",
            prefix: rnd::PREFIX,
            internal_only: false,
            router: rnd::router,
        },
        Mount {
            module: "crm_inbox",
            prefix: crm_inbox::PREFIX,
            internal_only: true,
            router: crm_inbox::router,
        },
        Mount {
            module: "crm_records",
            prefix: crm_records::PREFIX,
            internal_only: true,
            router: crm_records::router,
        },
        Mount {
            module: "attachments",
            prefix: attachments::PREFIX,
            internal_only: false,
            router: attachments::router,
        },
        Mount {
            module: "assets",
            prefix: assets::PREFIX,
            internal_only: true,
            router: assets::router,
        },
        Mount {
            module: "budget",
            prefix: budget::PREFIX,
            internal_only: true,
            router: budget::router,
        },
        Mount {
            module: "inventory",
            prefix: inventory::PREFIX,
            internal_only: true,
            router: inventory::router,
        },
        Mount {
            module: "p2p",
            prefix: p2p::PREFIX,
            internal_only: false,
            router: p2p::router,
        },
        Mount {
            module: "fleet",
            prefix: fleet::PREFIX,
            internal_only: true,
            router: fleet::router,
        },
        Mount {
            module: "maintenance",
            prefix: maintenance::PREFIX,
            internal_only: true,
            router: maintenance::router,
        },
        Mount {
            module: "helpdesk",
            prefix: helpdesk::PREFIX,
            internal_only: false,
            router: helpdesk::router,
        },
        Mount {
            module: "signing",
            prefix: signing::PREFIX,
            internal_only: false,
            router: signing::router,
        },
        Mount {
            module: "approvals",
            prefix: approvals::PREFIX,
            internal_only: true,
            router: approvals::router,
        },
        Mount {
            module: "verification",
            prefix: verification::PREFIX,
            internal_only: false,
            router: verification::router,
        },
        Mount {
            module: "signing_flow",
            prefix: signing_flow::PREFIX,
            internal_only: false,
            router: signing_flow::router,
        },
        Mount {
            module: "delivery",
            prefix: delivery::PREFIX,
            internal_only: false,
            router: delivery::router,
        },
        Mount {
            module: "portal_auth",
            prefix: portal_auth::PREFIX,
            internal_only: false,
            router: portal_auth::router,
        },
        Mount {
            module: "scheduled_messages",
            prefix: scheduled_messages::PREFIX,
            internal_only: true,
            router: scheduled_messages::router,
        },
        Mount {
            module: "collab",
            prefix: collab::PREFIX,
            internal_only: true,
            router: collab::router,
        },
        Mount {
            module: "product_catalog",
            prefix: product_catalog::PREFIX,
            internal_only: true,
            router: product_catalog::router,
        },
        Mount {
            module: "note_templates",
            prefix: note_templates::PREFIX,
            internal_only: true,
            router: note_templates::router,
        },
        Mount {
            module: "ai",
            prefix: ai::PREFIX,
            internal_only: false,
            router: ai::router,
        },
        Mount {
            module: "mcp_oauth",
            prefix: mcp_oauth::PREFIX,
            internal_only: false,
            router: mcp_oauth::router,
        },
        Mount {
            module: "mcp_oauth.well_known",
            prefix: "",
            internal_only: false,
            router: mcp_oauth::well_known,
        },
        Mount {
            module: "mcp",
            prefix: mcp::PREFIX,
            internal_only: false,
            router: mcp::router,
        },
        Mount {
            module: "nary",
            prefix: nary::PREFIX,
            internal_only: true,
            router: nary::router,
        },
        Mount {
            module: "nary_playbooks",
            prefix: nary_playbooks::PREFIX,
            internal_only: true,
            router: nary_playbooks::router,
        },
        Mount {
            module: "nary_playbooks.public_router",
            prefix: nary_playbooks::PREFIX,
            internal_only: false,
            router: nary_playbooks::public_router,
        },
        // Rust-only (2 Nov 2026): TOTP two-factor sign-in, the person's setup and the admin reset
        Mount {
            module: "totp",
            prefix: totp::PREFIX,
            internal_only: false,
            router: totp::router,
        },
        // Rust-only (5 Nov 2026): one-time upload links, the drop page's routes (no login; the token is the key)
        Mount {
            module: "public_drop",
            prefix: public_drop::PREFIX,
            internal_only: false,
            router: public_drop::router,
        },
    ]
}

/// All API routes, gated like main.py.
pub fn api_router(state: &AppState) -> Router<AppState> {
    let mut app = Router::new();
    for m in mounts() {
        let mut r = (m.router)();
        if m.internal_only && r.has_routes() {
            r = r.route_layer(axum::middleware::from_fn_with_state(
                state.clone(),
                granary_core::auth::internal_only,
            ));
        }
        app = app.merge(r);
    }
    app
}
