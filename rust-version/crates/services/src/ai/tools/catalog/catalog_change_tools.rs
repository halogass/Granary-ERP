//! 23 Oct 2026: the Owner changes the product catalog from the ERP chat, WhatsApp and the MCP client.
//!
//! - `get_catalog_item` (read, catalog viewers): one product in full: its fields, every price row with its id
//!   and dates, retired or not; cost price for cost viewers, volume tiers for the Owner.
//! - `propose_catalog_change` (the Owner only): one product, one or more changes, worked out on the real tables
//!   and rolled back so the card shows each field and price before and after, the open quotations that use the
//!   product and the price list refresh. A card (action catalog.change, Ask, never
//!   promoted), applied by nary::catalog_changes.
//! - `propose_product_file` (the Owner only; 5 Nov 2026): a file onto a product's Product files (a library
//!   document), stored once; a card (action catalog.product_file, Ask, never promoted), applied by
//!   nary::product_files.

use chrono::NaiveDate;
use granary_core::auth::AuthUser;
use granary_core::clock;
use granary_core::errors::ApiResult;
use granary_db::rows::sales::{CatalogPrice, CatalogVolumeTier, ProductCatalog};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::proposal_tools::{card_fields, text};
use super::{can_see_costs, handler, Tool};
use crate::catalog::writes;
use crate::nary::catalog_changes as cc;

/// Listed only for the Owner (see [`keep_for_owner`]).
/// 5 Nov 2026: propose_product_file too.
pub const OWNER_ONLY_TOOLS: [&str; 2] = ["propose_catalog_change", "propose_product_file"];

const VIEW_PERMS: &[&str] = &["sales:view_catalog", "sales:manage_catalog"];

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

/// Takes the Owner-only tools out of a list for anyone but the Owner (the chat and MCP tool lists).
pub async fn keep_for_owner(
    db: &mut PgConnection,
    user: &AuthUser,
    tools: &mut Vec<Tool>,
) -> ApiResult<()> {
    if !tools
        .iter()
        .any(|t| OWNER_ONLY_TOOLS.contains(&t.name.as_str()))
    {
        return Ok(());
    }
    if !cc::is_owner(db, user).await? {
        tools.retain(|t| !OWNER_ONLY_TOOLS.contains(&t.name.as_str()));
    }
    Ok(())
}

/// The tools of this module (appended to ALL_TOOLS by defs.rs).
pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            "get_catalog_item",
            "One catalog product in full (also a retired one): its fields, every price row with its price_id, \
             currency, amount and dates (valid_from, valid_to; current marks today's) and \
             whether it is retired; the cost price for cost viewers and the volume tiers for the Owner. Give the \
             product by id, SKU or name (search_catalog finds it). Read it before propose_catalog_change.",
            json!({
                "type": "object",
                "properties": {
                    "product": {"type": "string", "description": "The product's id, SKU or name."},
                },
                "required": ["product"],
            }),
            handler!(_h_get_catalog_item),
            VIEW_PERMS,
        ),
        Tool::new(
            "propose_catalog_change",
            "The Owner only: change the product catalog as a card the Owner confirms (never applied on its own). \
             One product per card (product: id, SKU or name; leave it out for a new SKU) and one or more changes \
             in order, each with op: create_product (fields: sku, name and the other product fields; prices and \
             tiers may follow as further changes), update_product (fields: only the fields that change, e.g. \
             name, description, family, unit, sale_model, price_metric, min_quantity, terms_note), add_price \
             (currency, amount as plain digits like \"16500000\", valid_from default today, optional valid_to; \
             the open price of that currency ends the day before), update_price or end_price (price_id from \
             get_catalog_item, or the currency for today's row; end_price valid_to default today), set_tiers \
             (tiers: the full list of {min_qty, max_qty, discount_pct}; private, never shown to customers), \
             set_cost_price (cost_price; empty clears it), retire, unretire. \
             The card shows each field and price before and after, the open quotations with this product (they \
             keep their price), and the internal price list refresh. The \
             editor's checks apply (a refused change says why).",
            json!({
                "type": "object",
                "properties": {
                    "product": {"type": "string", "description": "The product's id, SKU or name (not for create_product)."},
                    "changes": {
                        "type": "array",
                        "description": "The changes, in order.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "op": {"type": "string", "enum": cc::OPS},
                                "fields": {"type": "object", "description": "create_product, update_product: the product fields."},
                                "price_id": {"type": "string", "description": "update_price, end_price: the price row (get_catalog_item)."},
                                "currency": {"type": "string", "description": "add_price (and to find today's row for update_price or end_price): e.g. USD, JPY."},
                                "amount": {"type": "string", "description": "add_price, update_price: plain digits, e.g. \"1650.00\"."},
                                "valid_from": {"type": "string", "description": "YYYY-MM-DD."},
                                "valid_to": {"type": "string", "description": "YYYY-MM-DD."},
                                "tiers": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "min_qty": {"type": "number"},
                                            "max_qty": {"type": "number"},
                                            "discount_pct": {"type": "number"},
                                        },
                                        "required": ["min_qty", "discount_pct"],
                                    },
                                },
                                "cost_price": {"type": "string", "description": "set_cost_price: plain digits; empty clears it."},
                            },
                            "required": ["op"],
                        },
                    },
                },
                "required": ["changes"],
            }),
            handler!(_h_propose_catalog_change),
            VIEW_PERMS,
        ),
        // 5 Nov 2026 (Stream A): a file onto a catalog product's Product files, as the Owner's card
        Tool::new(
            "propose_product_file",
            "The Owner only: add a file to a catalog product's Product files (a document of the document library, \
             category Products, linked to the product) as a card the Owner confirms (never applied on its own). \
             file_id is any file id Nary can send: a CRM attachment (a deal's, lead's, organization's... Attachments, \
             list_attachments gives them), a file sent in the chat, an email attachment of the CRM inbox. The file \
             must be a PDF, an image or a video (MP4, MOV, WebM; judged by its content), within \
             documents.max_file_mb (videos nary.upload.video_max_mb). Store once: a CRM attachment is not copied, \
             the product file uses the same stored file, and deleting the attachment later keeps it. By default a \
             new document (title default the file name, audience customer, language en); with document_id it \
             becomes a new version of that document. remove_from_record true takes the attachment off its record \
             after applying (the stored file stays). The card shows the product, the file (type, size, a video's \
             length), the audience and new document or new version; once the Owner confirms it goes live at once. \
             Videos are logged on every send but never watermarked; find_product_files lists them with their length.",
            json!({
                "type": "object",
                "properties": {
                    "product": {"type": "string", "description": "The product's id, SKU or name."},
                    "file_id": {"type": "string", "description": "The file's id (a CRM attachment, a chat file, an email attachment)."},
                    "title": {"type": "string", "description": "The document's title (default: the file name without its extension)."},
                    "audience": {"type": "string", "enum": ["customer", "partner", "internal"], "description": "Who may get it (default customer)."},
                    "language": {"type": "string", "description": "Two-letter code, e.g. en (default en)."},
                    "document_id": {"type": "string", "description": "Optional: add the file as a new version of this library document (doc:<id> or the id) instead of a new document."},
                    "remove_from_record": {"type": "boolean", "description": "After applying, take the attachment off its record (the stored file stays). Default false."},
                },
                "required": ["product", "file_id"],
            }),
            handler!(_h_propose_product_file),
            VIEW_PERMS,
        ),
    ]
}

/// `propose_product_file` (5 Nov 2026): the Owner's card that adds a file to a product's Product files.
pub async fn _h_propose_product_file(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    use crate::nary::product_files as pf;
    if !cc::is_owner(db, user).await? {
        return err(pf::ONLY_OWNER);
    }
    let raw = text(&args, "product").unwrap_or_default();
    let product = match find_product(db, &raw).await? {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    let file_id = text(&args, "file_id").unwrap_or_default();
    if file_id.trim().is_empty() {
        return err(
            "file_id is required (list_attachments, find_product_files or the chat give the ids)",
        );
    }
    let mut payload = json!({
        "product_id": product.id.to_string(),
        "file_id": file_id.trim(),
        "title": text(&args, "title").unwrap_or_default().trim(),
        "audience": text(&args, "audience").unwrap_or_else(|| "customer".into()).trim().to_lowercase(),
        "language": text(&args, "language").unwrap_or_else(|| "en".into()).trim().to_lowercase(),
        "document_id": text(&args, "document_id").filter(|d| !d.trim().is_empty()),
        "remove_from_record": args.get("remove_from_record").and_then(Value::as_bool).unwrap_or(false),
    });
    let checked = match pf::check(db, user, &payload).await? {
        Ok(c) => c,
        Err(why) => return err(why),
    };
    // the card carries the title and audience it shows
    payload["title"] = json!(checked.title);
    payload["audience"] = json!(checked.audience);
    payload["language"] = json!(checked.language);
    let (summary, lines) = pf::card_text(&checked);
    let mut out = super::_autonomy_card(db, user, pf::ACTION, payload, summary, None).await?;
    card_fields(
        &mut out,
        vec![
            ("details", json!(lines)),
            ("product_label", json!(checked.product_label)),
        ],
    );
    Ok(out)
}

/// The product an id, SKU or name names (not deleted; retired ones too): Ok(Err(why)) when none or several.
pub(crate) async fn find_product(
    db: &mut PgConnection,
    raw: &str,
) -> ApiResult<Result<ProductCatalog, String>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(Err(
            "product is required (its id, SKU or name; search_catalog finds it)".into(),
        ));
    }
    if let Ok(id) = Uuid::parse_str(raw) {
        let p: Option<ProductCatalog> =
            sqlx::query_as("SELECT * FROM product_catalog WHERE id = $1 AND is_deleted = false")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?;
        return Ok(p.ok_or_else(|| format!("No catalog product with id {raw}")));
    }
    for sql in [
        "SELECT * FROM product_catalog WHERE is_deleted = false AND lower(sku) = lower($1)",
        "SELECT * FROM product_catalog WHERE is_deleted = false AND lower(name) = lower($1)",
    ] {
        let rows: Vec<ProductCatalog> = sqlx::query_as(sql).bind(raw).fetch_all(&mut *db).await?;
        if rows.len() == 1 {
            return Ok(Ok(rows.into_iter().next().unwrap()));
        }
    }
    let like = format!("%{}%", granary_compat::text::utils::escape_like(raw));
    let rows: Vec<ProductCatalog> = sqlx::query_as(
        "SELECT * FROM product_catalog WHERE is_deleted = false \
         AND (name ILIKE $1 ESCAPE '\\' OR sku ILIKE $1 ESCAPE '\\') ORDER BY sku LIMIT 11",
    )
    .bind(&like)
    .fetch_all(&mut *db)
    .await?;
    Ok(match rows.len() {
        0 => Err(format!(
            "No catalog product with id, SKU or name {raw} (search_catalog finds it)"
        )),
        1 => Ok(rows.into_iter().next().unwrap()),
        _ => Err(format!(
            "Several products match {raw}: {}. Give the SKU.",
            rows.iter()
                .take(10)
                .map(|p| format!("{} ({})", p.name, p.sku))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    })
}

fn dec(d: &rust_decimal::Decimal) -> Value {
    json!(d.normalize().to_string())
}

/// `get_catalog_item`.
pub async fn _h_get_catalog_item(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let raw = text(&args, "product").unwrap_or_default();
    let p = match find_product(db, &raw).await? {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    let today = clock::today_local(db).await?;
    let prices: Vec<CatalogPrice> = sqlx::query_as(
        "SELECT * FROM catalog_prices WHERE product_id = $1 ORDER BY currency, valid_from",
    )
    .bind(p.id)
    .fetch_all(&mut *db)
    .await?;
    let snap = cc::snapshot(db, p.id).await?;
    let mut fields = snap["product"].as_object().cloned().unwrap_or_default();
    // 5 Nov 2026: the box licence value is internal like the cost price
    for k in [
        "id",
        "is_deleted",
        "created_by_id",
        "cost_price",
        "box_licence_value",
    ] {
        fields.remove(k);
    }
    let mut out = Map::new();
    out.insert("id".into(), json!(p.id.to_string()));
    out.insert("sku".into(), json!(p.sku));
    out.insert("name".into(), json!(p.name));
    out.insert(
        "retired".into(),
        json!(!(p.is_active.unwrap_or(true) && p.is_orderable.unwrap_or(true))),
    );
    out.insert("fields".into(), Value::Object(fields));
    out.insert(
        "price_rows".into(),
        json!(prices
            .iter()
            .map(|r| json!({
                "price_id": r.id.to_string(), "currency": r.currency, "amount": dec(&r.amount),
                "valid_from": r.valid_from.to_string(),
                "valid_to": r.valid_to.map(|d| d.to_string()),
                "current": r.valid_from <= today && r.valid_to.is_none_or(|t| t >= today),
            }))
            .collect::<Vec<_>>()),
    );
    if can_see_costs(user) || cc::is_owner(db, user).await? {
        out.insert(
            "cost_price".into(),
            p.cost_price.as_ref().map(dec).unwrap_or(Value::Null),
        );
    }
    if cc::is_owner(db, user).await? {
        let tiers: Vec<CatalogVolumeTier> = writes::tiers_of(db, p.id).await?;
        out.insert(
            "volume_tiers".into(),
            json!(tiers
                .iter()
                .map(|t| json!({"min_qty": dec(&t.min_qty),
                                "max_qty": t.max_qty.as_ref().map(dec),
                                "discount_pct": dec(&t.discount_pct)}))
                .collect::<Vec<_>>()),
        );
    }
    out.insert("today".into(), json!(today.to_string()));
    Ok(Value::Object(out))
}

/// Today's row of a currency (the latest that started), else its latest open row.
async fn row_of_currency(
    db: &mut PgConnection,
    product_id: Uuid,
    currency: &str,
    today: NaiveDate,
) -> ApiResult<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM catalog_prices WHERE product_id = $1 AND currency = upper($2) \
         AND (valid_to IS NULL OR valid_to >= $3) \
         ORDER BY (valid_from <= $3) DESC, valid_from DESC LIMIT 1",
    )
    .bind(product_id)
    .bind(currency.trim())
    .bind(today)
    .fetch_optional(&mut *db)
    .await?)
}

/// The changes with their defaults filled (dates, price rows by currency), so the card is exactly what applies.
async fn normalized(
    db: &mut PgConnection,
    product: Option<&ProductCatalog>,
    changes: &[Value],
    today: NaiveDate,
) -> ApiResult<Result<Vec<Value>, String>> {
    let mut out = vec![];
    for (i, c) in changes.iter().enumerate() {
        let Some(m) = c.as_object() else {
            return Ok(Err(format!("change {} is not an object", i + 1)));
        };
        let mut m = m.clone();
        let op = m
            .get("op")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let blank = |m: &Map<String, Value>, k: &str| match m.get(k) {
            None | Some(Value::Null) => true,
            Some(Value::String(s)) => s.trim().is_empty(),
            _ => false,
        };
        match op.as_str() {
            "add_price" if blank(&m, "valid_from") => {
                m.insert("valid_from".into(), json!(today.to_string()));
            }
            "update_price" | "end_price" if blank(&m, "price_id") => {
                let cur = m
                    .get("currency")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let Some(p) = product else {
                    return Ok(Err(format!("change {}: name the product first", i + 1)));
                };
                if cur.trim().is_empty() {
                    return Ok(Err(format!(
                        "change {} ({op}): give price_id (get_catalog_item) or the currency",
                        i + 1
                    )));
                }
                match row_of_currency(db, p.id, &cur, today).await? {
                    Some(id) => {
                        m.insert("price_id".into(), json!(id.to_string()));
                        if op == "end_price" {
                            m.remove("currency");
                        }
                    }
                    None => {
                        return Ok(Err(format!(
                            "change {} ({op}): {} has no current {} price",
                            i + 1,
                            p.sku,
                            cur.trim().to_uppercase()
                        )))
                    }
                }
            }
            _ => {}
        }
        if op == "end_price" && blank(&m, "valid_to") {
            m.insert("valid_to".into(), json!(today.to_string()));
        }
        if op == "set_cost_price" && blank(&m, "cost_price") {
            m.insert("cost_price".into(), Value::Null);
        }
        for k in ["valid_from", "valid_to"] {
            if blank(&m, k) {
                m.remove(k);
            }
        }
        out.push(Value::Object(m));
    }
    Ok(Ok(out))
}

/// `propose_catalog_change`.
pub async fn _h_propose_catalog_change(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !cc::is_owner(db, user).await? {
        return err(cc::ONLY_OWNER);
    }
    let changes = args
        .get("changes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if changes.is_empty() {
        return err("changes is required: one or more changes, each with op");
    }
    let creates = changes
        .first()
        .and_then(|c| c.get("op"))
        .and_then(Value::as_str)
        == Some("create_product");
    let product = if creates {
        None
    } else {
        let raw = text(&args, "product").unwrap_or_default();
        match find_product(db, &raw).await? {
            Ok(p) => Some(p),
            Err(e) => return err(e),
        }
    };
    let today = clock::today_local(db).await?;
    let changes = match normalized(db, product.as_ref(), &changes, today).await? {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    let ops = match cc::parse_ops(&changes, today) {
        Ok(o) => o,
        Err(e) => return err(e),
    };
    let pv = match cc::preview(db, user.id, product.clone(), &ops).await {
        Ok(p) => p,
        Err(e) => return err(cc::detail(&e)),
    };
    let lines = cc::card_lines(db, &pv, &ops).await?;
    let summary = cc::summary(&pv, &ops);
    let payload = json!({
        "product_id": product.as_ref().map(|p| p.id.to_string()),
        "changes": changes,
        "before": cc::fingerprint(&pv.before),
    });
    let mut out = super::_autonomy_card(db, user, cc::CHANGE, payload, summary, None).await?;
    card_fields(
        &mut out,
        vec![
            ("details", json!(lines)),
            ("product_label", json!(cc::product_label(&pv.after))),
        ],
    );
    Ok(out)
}
