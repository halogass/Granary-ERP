//! Port of backend/app/services/approvals/defaults.py.
//!
//! Default approval policies, transcribed from the SOPs in scripts/seed_sops.py.
//!
//! Seeded once per document type (never overwriting an admin's edits) and editable in
//! Settings -> Approvals & signing -> Approval policies. Amounts are in the functional (base) currency; the engine converts from the document currency
//! at submit time and escalates when no FX rate is available.
//!
//! Condition / precondition rule names are defined in app/services/approvals/conditions.py.
//!
//! `POLICIES` keeps the Python dict key order (serde_json is built with preserve_order) and the Python
//! value types (ints stay ints, `None` is null). The seed (crates/db/src/seed/seed_data.json) carries the
//! same data generated from Python; tests/approvals_defaults.rs checks the two agree.

use once_cell::sync::Lazy;
use serde_json::{json, Value};

pub const OWNER_SLA_HOURS: i64 = 24;

pub static POLICIES: Lazy<Vec<Value>> = Lazy::new(|| {
    vec![
        json!({
            "entity_type": "quotation",
            "name": "Quotation approval",
            "sop_reference": null,
            "description": "Catalog items at list price with no discount are approved automatically; the Owner approves every other quotation.",
            "preconditions": [
                {"rule": "missing_discount_type",
                 "message": "Every discounted line must state its discount type (volume, bundle, loyalty, multi-year SaaS or Owner exception)."},
            ],
            "signature_flow": "none",
            "qr_enabled": true, "view_scope": "metadata_document", "allow_download": true,
            "requires_login": false, "token_ttl_days": 365,
            "sla_hours": OWNER_SLA_HOURS, "whatsapp_escalate_above_base": 200_000_000,
            "steps": [
                {"sequence": 1, "name": "Owner approval", "approver_mode": "owner", "sla_hours": OWNER_SLA_HOURS,
                 "condition": {"rule": "not_catalog_list_price"}},
            ],
        }),
        json!({
            "entity_type": "sales_document",
            "name": "Solution Proposal approval",
            "sop_reference": null,
            "description": "The Owner approves every Solution Proposal; any edit after approval needs approval again. The approval is the signature stamp, and the QR on every page proves it.",
            "signature_flow": "none",
            "qr_enabled": true, "view_scope": "metadata_document", "allow_download": true,
            "requires_login": false, "token_ttl_days": 365,
            "sla_hours": OWNER_SLA_HOURS, "whatsapp_escalate_above_base": 0,
            "steps": [
                {"sequence": 1, "name": "Owner approval", "approver_mode": "owner", "sla_hours": OWNER_SLA_HOURS},
            ],
        }),
        json!({
            "entity_type": "invoice",
            "name": "Invoice approval",
            "sop_reference": null,
            "description": "The Owner approves every invoice (and every credit note) before it is sent.",
            "signature_flow": "none",
            "qr_enabled": true, "view_scope": "metadata_document", "allow_download": true,
            "requires_login": false, "token_ttl_days": 365,
            "sla_hours": OWNER_SLA_HOURS, "whatsapp_escalate_above_base": 150_000_000,
            "steps": [
                {"sequence": 1, "name": "Owner approval", "approver_mode": "owner", "sla_hours": OWNER_SLA_HOURS},
            ],
        }),
        json!({
            "entity_type": "sales_contract",
            "name": "Contract approval",
            "sop_reference": null,
            "description": "The Owner reviews every contract before it goes to the customer's authorized signatory.",
            "signature_flow": "counterparty", "signer_role": "owner",
            "sign_deadline_days": 5, "max_extension_days": 10,
            "qr_enabled": true, "view_scope": "metadata_document", "allow_download": true,
            "requires_login": false, "token_ttl_days": 365 * 3,
            "sla_hours": 48,
            "steps": [
                {"sequence": 1, "name": "Owner review", "approver_mode": "owner", "sla_hours": 48},
            ],
        }),
        json!({
            "entity_type": "purchase_order",
            "name": "Purchase order approval",
            "sop_reference": null,
            "description": "Manufacturing Lead approves up to 5M (base currency); the Owner above that. 10M+ needs two vendor quotes.",
            "preconditions": [
                {"rule": "missing_vendor_quotes", "value": {"min_amount_base": 10_000_000, "min_quotes": 2},
                 "message": "Purchases of 10,000,000 (base currency) or more need at least 2 vendor quotes (an RFQ with two responses, or two quote files attached)."},
            ],
            "signature_flow": "none",
            "qr_enabled": true, "view_scope": "metadata_document", "allow_download": true,
            "requires_login": false, "token_ttl_days": 365,
            "sla_hours": 72, "whatsapp_escalate_above_base": 5_000_000,
            "steps": [
                {"sequence": 1, "name": "Manufacturing Lead approval", "approver_mode": "role",
                 "role_slug": "team_lead", "department": "Manufacturing", "sla_hours": 24,
                 "condition": {"not": {"rule": "amount_gt_base", "value": 5_000_000}}},
                {"sequence": 2, "name": "Owner approval", "approver_mode": "owner", "sla_hours": 72,
                 "condition": {"rule": "amount_gt_base", "value": 5_000_000}},
            ],
        }),
        json!({
            "entity_type": "bom",
            "name": "Bill of materials approval",
            "sop_reference": null,
            "description": "The Manufacturing Lead approves a bill of materials; the Owner too when one unit costs more than 25M (base currency). Production uses only an approved BOM.",
            "sla_hours": 72,
            "steps": [
                {"sequence": 1, "name": "Manufacturing Lead approval", "approver_mode": "role",
                 "role_slug": "team_lead", "department": "Manufacturing", "sla_hours": 48},
                {"sequence": 2, "name": "Owner approval (high unit cost)", "approver_mode": "owner", "sla_hours": 72,
                 "condition": {"rule": "amount_gt_base", "value": 25_000_000}},
            ],
        }),
        json!({
            "entity_type": "contractor_request",
            "name": "Contractor request approval",
            "sop_reference": null,
            "description": "The Owner authorizes every external contractor engagement before any commitment.",
            "sla_hours": 120,
            "steps": [
                {"sequence": 1, "name": "Owner approval", "approver_mode": "owner", "sla_hours": 120},
            ],
        }),
        json!({
            "entity_type": "work_order",
            "name": "Work order issuance",
            "sop_reference": null,
            "description": "A work order must link an Owner-approved contractor request; value above the approved estimate needs the Owner.",
            "preconditions": [
                {"rule": "missing_approved_contractor_request",
                 "message": "A work order must link an Owner-approved contractor request."},
            ],
            "qr_enabled": true, "view_scope": "metadata_document", "allow_download": true,
            "requires_login": false, "token_ttl_days": 365,
            "sla_hours": OWNER_SLA_HOURS,
            "steps": [
                {"sequence": 1, "name": "Owner approval (above approved estimate)", "approver_mode": "owner",
                 "sla_hours": OWNER_SLA_HOURS, "condition": {"rule": "above_cr_estimate"}},
            ],
        }),
        json!({
            "entity_type": "vendor_invoice",
            "name": "Vendor / contractor invoice approval",
            "sop_reference": null,
            "description": "< 5M Finance Admin; 5\u{2013}25M Finance Admin + PM/Dept Head; 25\u{2013}75M Owner; > 75M Owner with WO/PO evidence.",
            "preconditions": [
                {"rule": "missing_closed_wo_po",
                 "message": "Finance processes no invoice without a closed, approved work order or purchase order."},
            ],
            "sla_hours": 72, "whatsapp_escalate_above_base": 25_000_000,
            "steps": [
                {"sequence": 1, "name": "Finance Admin review", "approver_mode": "role", "role_slug": "finance_admin",
                 "allow_self_approval": true, "sla_hours": 48,
                 "condition": {"not": {"rule": "amount_gt_base", "value": 25_000_000}}},
                {"sequence": 2, "name": "PM / Dept Head confirmation", "approver_mode": "snapshot_user",
                 "snapshot_user_key": "responsible_user_id", "sla_hours": 48,
                 "condition": {"all": [
                     {"rule": "amount_gte_base", "value": 5_000_000},
                     {"not": {"rule": "amount_gt_base", "value": 25_000_000}},
                 ]}},
                {"sequence": 3, "name": "Owner approval", "approver_mode": "owner", "sla_hours": 72,
                 "condition": {"rule": "amount_gt_base", "value": 25_000_000}},
            ],
        }),
        json!({
            "entity_type": "expense",
            "name": "Expense claim approval",
            "sop_reference": null,
            "description": "Line manager, then Finance Admin; the Owner too above 2M per claim or 1.5M per item (base currency).",
            "preconditions": [
                {"rule": "flag_missing_above_base", "value": {"amount_base": 500_000, "flag": "pre_approved"},
                 "message": "Expenses above 500,000 (base currency) need manager pre-approval before they are incurred; confirm pre-approval when submitting."},
            ],
            "sla_hours": 72,
            "steps": [
                {"sequence": 1, "name": "Line manager approval", "approver_mode": "manager", "sla_hours": 72},
                {"sequence": 2, "name": "Finance Admin review", "approver_mode": "role", "role_slug": "finance_admin",
                 "sla_hours": 48},
                {"sequence": 3, "name": "Owner approval", "approver_mode": "owner", "sla_hours": 48,
                 "condition": {"any": [
                     {"rule": "amount_gt_base", "value": 2_000_000},
                     {"rule": "single_line_gt_base", "value": 1_500_000},
                     {"rule": "fx_rate_missing"},
                 ]}},
            ],
        }),
        json!({
            "entity_type": "marketing_content",
            "name": "External content approval",
            "sop_reference": null,
            "description": "Technical content gets an R&D Lead review and customer references a PM review before the Owner approves.",
            "sla_hours": 48,
            "steps": [
                {"sequence": 1, "name": "R&D Lead technical review", "approver_mode": "role", "role_slug": "team_lead",
                 "department": "R&D", "sla_hours": 48,
                 "condition": {"rule": "content_type_in", "value": ["blog", "case_study", "whitepaper", "datasheet"]}},
                {"sequence": 2, "name": "PM review of customer references", "approver_mode": "role",
                 "role_slug": "project_manager", "sla_hours": 48,
                 "condition": {"rule": "flag_set", "value": "customer_reference"}},
                {"sequence": 3, "name": "Owner approval", "approver_mode": "owner", "sla_hours": 24},
            ],
        }),
        json!({
            "entity_type": "hiring_request",
            "name": "Headcount approval",
            "sop_reference": null,
            "description": "The Owner approves every headcount request.",
            "sla_hours": 72,
            "steps": [
                {"sequence": 1, "name": "Owner approval", "approver_mode": "owner", "sla_hours": 72},
            ],
        }),
        json!({
            "entity_type": "offer_letter",
            "name": "Offer letter approval",
            "sop_reference": null,
            "description": "The Owner decides the offer; HR issues it; the candidate accepts within 5 business days.",
            "signature_flow": "counterparty", "signer_role": "owner",
            "sign_deadline_days": 5, "max_extension_days": 0,
            "qr_enabled": true, "view_scope": "metadata_document", "allow_download": false,
            "requires_login": true, "token_ttl_days": 180,
            "sla_hours": 24,
            "steps": [
                {"sequence": 1, "name": "Owner approval", "approver_mode": "owner", "sla_hours": 24},
            ],
        }),
        json!({
            "entity_type": "sop_article",
            "name": "SOP publication approval",
            "sop_reference": "Knowledge base",
            "description": "The Owner approves an SOP before it is published. Author/Owner signing stays in the KB flow.",
            "qr_enabled": true, "view_scope": "metadata_document", "allow_download": false,
            "requires_login": false, "token_ttl_days": 365 * 2,
            "sla_hours": 72,
            "steps": [
                {"sequence": 1, "name": "Owner approval", "approver_mode": "owner", "sla_hours": 72},
            ],
        }),
        json!({
            "entity_type": "payroll_run",
            "name": "Payroll run approval",
            "sop_reference": "HR payroll",
            "description": "The Owner approves each monthly payroll run before it is paid.",
            "sla_hours": 48,
            "steps": [
                {"sequence": 1, "name": "Owner approval", "approver_mode": "owner", "sla_hours": 48},
            ],
        }),
    ]
});

// BDR discount authority (no Owner approval needed)
// Discount allowances are retired (migration 081): the only automatic discount is the
// volume tier on each catalog SKU (catalog_volume_tiers, admin-edited); anything else is an Owner
// exception. Admins can still add allowances in Settings -> Approvals & signing -> Approval policies.
pub static DISCOUNT_ALLOWANCES: Lazy<Vec<Value>> = Lazy::new(Vec::new);
