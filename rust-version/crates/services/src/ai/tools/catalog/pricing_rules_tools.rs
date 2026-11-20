//! 5 Nov 2026 (offering expansion E7, Owner decisions 5 Nov): `set_pricing_override`, the Owner lets a draft quotation
//! through the pricing rules with a reason (crate::pricing_rules). It writes at once (an audit row records it), so
//! over MCP it is listed only at act and full power (mcp_access::ACT_TOOLS); in Nary's chat for the Owner. Works
//! while Nary is paused. The check itself is in get_quotation (pricing_check) and on the quotation cards.

use serde_json::{json, Value};
use sqlx::PgConnection;

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};

use super::{handler, Tool};

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

pub fn tools() -> Vec<Tool> {
    vec![Tool::new(
        "set_pricing_override",
        concat!(
            "The Owner only: let a draft quotation through the pricing rules that block its approval (a line below ",
            "the minimum multiple of its cost, or a SKU that always needs the Owner). Give ",
            "quotation_id (id or number) and reason (one sentence, why these prices are accepted); an empty reason ",
            "removes the override. It holds only for the lines as they are now: a later price change needs a new ",
            "override. get_quotation shows pricing_check (blocking, warnings, override). Recorded in the audit log.",
        ),
        json!({
            "type": "object",
            "properties": {
                "quotation_id": {
                    "type": "string",
                    "description": "The quotation's id or number (list_quotations).",
                },
                "reason": {
                    "type": "string",
                    "description": "Why the Owner accepts these prices (at least 10 characters); empty removes the override.",
                },
            },
            "required": ["quotation_id", "reason"],
        }),
        handler!(_h_set_pricing_override),
        &["sales:view_quotations"],
    )]
}

/// `set_pricing_override(quotation_id, reason)`.
pub async fn _h_set_pricing_override(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !crate::nary::quotation_updates::is_owner(db, user).await? {
        return err(crate::pricing_rules::OVERRIDE_REFUSED);
    }
    let Some(q) = crate::nary::quotation_updates::find(db, &args).await? else {
        return err("Quotation not found: give quotation_id or its number (list_quotations).");
    };
    let reason = args.get("reason").and_then(Value::as_str);
    match crate::pricing_rules::set_override(db, user, q.id, reason, "nary").await {
        Ok(mut out) => {
            out["quotation"] = json!(q.full_number.clone().unwrap_or_else(|| q.number.clone()));
            Ok(out)
        }
        Err(e @ ApiError::Http { .. }) => {
            err(crate::nary::governance::http_detail(&e).unwrap_or_default())
        }
        Err(e) => Err(e),
    }
}
