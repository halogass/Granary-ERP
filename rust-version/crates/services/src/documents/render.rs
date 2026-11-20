//! Port of backend/app/services/documents/render.py.
//!
//! The Solution Proposal PDF (Nary Phase 4): data for services/pdf/src/templates/proposal.html.
//!
//! Markdown to HTML (markdown-it, raw HTML off) to the shared sanitiser. Images are inlined as data: URIs
//! (the PDF service has no network to the backend). The commercial section is built here from the
//! linked quotations' lines and totals, every time, so the numbers can't differ from the quotations.
//! The header ("Your Company | Confidential") and the verified footer (QR, reference and revision,
//! page) are printed on every page by the PDF service.
//!
//! Python's `_MD = MarkdownIt("commonmark", {"html": False, "linkify": False}).enable("table")
//! .enable("strikethrough")` (markdown-it-py 3.0.0) is the `md` module below: the Rust `markdown-it` crate
//! with the cmark rules, where every rule that renders differently from markdown-it-py is replaced by a
//! port of the Python rule (table, blockquote, fence, paragraph, reference, code spans, emphasis
//! classification, link normalisation with mdurl + punycode, nesting limit 20, the normalize core rule).
//! Golden-tested on the CommonMark 0.30 spec plus fuzzed inputs (compat-tests/golden/documents).

use granary_compat::pyfmt::{format_number, PyNum};
use granary_compat::text::pystr;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{currency_places, get_optional};
use granary_db::rows::approval::ApprovalRequest;
use granary_db::rows::documents::{SalesDocument, SalesDocumentAsset};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;

use super::assets as asset_store;
use super::core_ as core;
use super::core_::{as_dict, or, py_iter, py_str, py_uuid, truthy};
use crate::verification::render::{approval_stamp, fmt_date, fmt_money, sanitize_html};

/// `_MD.render(md)` (markdown-it-py, before the sanitiser).
pub fn md_render(md: &str) -> String {
    md::md_render(md)
}

/// `markdown_html(md)`: rendered and sanitised; "" for blank text.
pub fn markdown_html(md: Option<&str>) -> String {
    let m = md.unwrap_or("");
    if pystr::strip(m).is_empty() {
        return String::new();
    }
    sanitize_html(Some(&md_render(m)))
}

/// `_cell(text, num)`.
fn cell(text: &Value, num: bool) -> Value {
    json!({"text": if text.is_null() { String::new() } else { py_str(text) }, "num": num})
}

fn cell_s(text: &str, num: bool) -> Value {
    json!({"text": text, "num": num})
}

/// `labels[key]` (a KeyError, a 500, when missing).
fn lab<'a>(labels: &'a Value, key: &str) -> ApiResult<&'a Value> {
    labels
        .get(key)
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: '{key}'")))
}

#[derive(sqlx::FromRow)]
struct ItemRow {
    name: String,
    amount: Decimal,
    is_optional: Option<bool>,
}

/// `commercial(db, doc, labels)`: Item | Amount | Payment, one row per billed quotation line, a total per
/// currency, then the terms that come with the quotations (validity).
pub async fn commercial(
    db: &mut PgConnection,
    doc: &SalesDocument,
    labels: &Value,
) -> ApiResult<Value> {
    let quotes = core::quotations_of(db, doc.id).await?;
    let mut rows = Vec::new();
    let mut notes = Vec::new();
    let mut totals: Vec<(String, Decimal)> = Vec::new();
    for (q, label) in &quotes {
        let cur = q
            .currency
            .clone()
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| "USD".into());
        let places = currency_places(db, Some(&cur)).await?;
        let items: Vec<ItemRow> = sqlx::query_as(
            "SELECT name, amount, is_optional FROM quotation_items WHERE quotation_id = $1 ORDER BY sort_order",
        )
        .bind(q.id)
        .fetch_all(&mut *db)
        .await?;
        let empty = json!({});
        let terms = q.payment_terms.clone().unwrap_or(Value::Null);
        let terms = or(Some(&terms), &empty);
        let payment = match terms {
            Value::Object(o) => or(o.get("text"), &Value::String(String::new())).clone(),
            other => {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "AttributeError: payment_terms {other} has no get"
                )))
            }
        };
        let number = q
            .full_number
            .clone()
            .filter(|f| !f.is_empty())
            .unwrap_or_else(|| q.number.clone());
        let suffix = match label.as_deref() {
            Some(l) if !l.is_empty() => format!(" ({l})"),
            _ => String::new(),
        };
        for (i, it) in items
            .iter()
            .filter(|it| !it.is_optional.unwrap_or(false))
            .enumerate()
        {
            rows.push(json!({"cells": [
                cell_s(&format!("{number}{suffix}: {}", it.name), false),
                cell_s(&fmt_money(Some(it.amount), Some(&cur), places), true),
                if i == 0 { cell(&payment, false) } else { cell_s("", false) },
            ]}));
        }
        if q.tax_amount.unwrap_or(Decimal::ZERO) > Decimal::ZERO {
            rows.push(json!({"cells": [
                cell_s(&format!("{number}: tax"), false),
                cell_s(&fmt_money(q.tax_amount, Some(&cur), places), true),
                cell_s("", false),
            ]}));
        }
        let add = q.total.unwrap_or(Decimal::ZERO);
        match totals.iter_mut().find(|(c, _)| *c == cur) {
            Some((_, t)) => *t += add,
            None => totals.push((cur.clone(), Decimal::ZERO + add)),
        }
        // valid_until is NOT NULL, so the note is always there
        notes.push(Value::String(format!(
            "{number}: {} {}.",
            py_str(lab(labels, "validity")?),
            granary_compat::pyjson::date_isoformat(q.valid_until)
        )));
    }
    for (cur, total) in &totals {
        let places = currency_places(db, Some(cur)).await?;
        rows.push(json!({"total": true, "cells": [
            cell_s(&format!("{} ({cur})", py_str(lab(labels, "total")?)), false),
            cell_s(&fmt_money(Some(*total), Some(cur), places), true),
            cell_s("", false),
        ]}));
    }
    Ok(json!({
        "columns": [{"label": lab(labels, "item")?}, {"label": lab(labels, "amount")?, "num": true},
                    {"label": lab(labels, "payment")?}],
        "rows": rows,
        "notes": notes,
    }))
}

/// `_image(db, asset_id, caption, labels, figure_no)`.
async fn image(
    db: &mut PgConnection,
    asset_id: &Value,
    caption: &Value,
    labels: &Value,
    figure_no: i64,
) -> ApiResult<Option<Value>> {
    if !truthy(asset_id) {
        return Ok(None);
    }
    let id = py_uuid(&py_str(asset_id))?;
    let cols = SalesDocumentAsset::COLUMNS.join(", ");
    let asset: Option<SalesDocumentAsset> = sqlx::query_as(&format!(
        "SELECT {cols} FROM sales_document_assets WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    let Some(asset) = asset else { return Ok(None) };
    let Some(key) = asset.storage_key.clone().filter(|k| !k.is_empty()) else {
        return Ok(None);
    };
    if asset.status != "ready" {
        return Ok(None);
    }
    let data = asset_store::get(&key).await?;
    let asset_caption = asset
        .caption
        .clone()
        .map(Value::String)
        .unwrap_or(Value::Null);
    let empty = Value::String(String::new());
    let cap = or(Some(caption), or(Some(&asset_caption), &empty));
    let figure = py_str(lab(labels, "figure")?);
    let caption = if truthy(cap) {
        pystr::strip(&format!("{figure} {figure_no}. {}", py_str(cap))).to_string()
    } else {
        format!("{figure} {figure_no}")
    };
    let mime = asset
        .mime
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "image/png".into());
    Ok(Some(
        json!({"src": asset_store::data_uri(&data, &mime), "caption": caption}),
    ))
}

/// visuals V2: a chart or diagram block for the template: {src (SVG data: URI), caption ("Figure N. ..."),
/// half (two per row), source ("Source: ..." for customer and analysis numbers)}. None without a drawing.
async fn figure_item(
    db: &mut PgConnection,
    data: &Value,
    labels: &Value,
    figure_no: i64,
) -> ApiResult<Option<Value>> {
    let src = {
        let Some(svg) = super::figures::stored_svg(db, data).await? else {
            return Ok(None);
        };
        asset_store::data_uri(&svg, super::figures::SVG_MIME)
    };
    let cap = data
        .get("caption")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let figure = py_str(lab(labels, "figure")?);
    let caption = if cap.is_empty() {
        format!("{figure} {figure_no}")
    } else {
        format!("{figure} {figure_no}. {cap}")
    };
    let spec = data
        .get("spec")
        .cloned()
        .and_then(|s| crate::visuals::spec::Spec::from_value(s).ok());
    let source = spec.as_ref().and_then(|s| s.caption_line()); // visuals2 P2C: also a power drawing's calculation
    let half = data.get("size").and_then(Value::as_str) == Some("half");
    Ok(Some(json!({
        "src": src,
        "caption": caption,
        "half": half,
        "source": source,
    })))
}

/// `str.format(days=n)` for the validity label: `{days}`, `{days:<spec>}`, `{{`, `}}`; anything else is the
/// KeyError/IndexError/ValueError Python raises (a 500).
fn format_days(template: &str, days: i64) -> ApiResult<String> {
    let bad = || ApiError::internal(anyhow::anyhow!("str.format failed on {template:?}"));
    let mut out = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut field = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(x) => field.push(x),
                        None => return Err(bad()),
                    }
                }
                let (name, spec) = match field.split_once(':') {
                    Some((n, s)) => (n, s),
                    None => (field.as_str(), ""),
                };
                if name != "days" {
                    return Err(bad());
                }
                out.push_str(&format_number(&PyNum::Int(days as i128), spec).map_err(|_| bad())?);
            }
            '}' => return Err(bad()),
            other => out.push(other),
        }
    }
    Ok(out)
}

/// `_setting_json(db, key)`: the decoded setting, None when unset or not JSON.
async fn setting_json(db: &mut PgConnection, key: &str) -> ApiResult<Option<Value>> {
    let raw = get_optional(db, key).await?;
    Ok(match raw {
        Some(r) => granary_core::validation::json_loads(r.as_bytes())
            .ok()
            .map(|o| o.to_json()),
        None => None,
    })
}

/// `build(db, doc, company, request)`: ("proposal", data, facts). Facts are (label, value) pairs as
/// Python returns them (the verification builder keeps the non-empty ones).
pub async fn build(
    db: &mut PgConnection,
    doc: &SalesDocument,
    company: &Value,
    request: Option<&ApprovalRequest>,
) -> ApiResult<(String, Value, Vec<(String, Option<String>)>)> {
    let labels = core::labels(db, &doc.language).await?;
    // {s["key"]: s for s in template}: first position, last value
    let mut template: Vec<(Value, Value)> = Vec::new();
    for s in core::template(db).await? {
        let key = s["key"].clone();
        match template.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = s,
            None => template.push((key, s)),
        }
    }
    let blocks = core::blocks_of(db, doc.id).await?;
    let issued = core::issue_date(db, doc).await?;
    let empty = json!({});
    let cover = as_dict(or(Some(&doc.cover), &empty), "cover")?.clone();
    let null = Value::Null;
    let prepared_by = match cover.get("prepared_by") {
        Some(v) if truthy(v) => v.clone(),
        _ => {
            let s = setting_json(db, "documents.proposal.prepared_by")
                .await?
                .unwrap_or(Value::Null);
            as_dict(or(Some(&s), &empty), "documents.proposal.prepared_by")?
                .get(&doc.language)
                .cloned()
                .unwrap_or(Value::Null)
        }
    };
    let reference = doc
        .full_number
        .clone()
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| doc.number.clone());
    let revision = format!("Rev. {}", doc.revision)
        + &match doc.revision_reason.as_deref() {
            Some(r) if !r.is_empty() => format!(": {r}"),
            _ => String::new(),
        };
    let validity_tpl = lab(&labels, "validity_value")?;
    let validity = match validity_tpl {
        Value::String(t) => format_days(t, doc.validity_days as i64)?,
        _ => {
            return Err(ApiError::internal(anyhow::anyhow!(
                "AttributeError: validity_value has no format"
            )))
        }
    };
    let rows: Vec<(Value, Value)> = vec![
        (
            lab(&labels, "prepared_for")?.clone(),
            cover.get("prepared_for").unwrap_or(&null).clone(),
        ),
        (
            lab(&labels, "attention")?.clone(),
            cover.get("attention").unwrap_or(&null).clone(),
        ),
        (lab(&labels, "prepared_by")?.clone(), prepared_by),
        (
            lab(&labels, "reference")?.clone(),
            Value::String(reference.clone()),
        ),
        (lab(&labels, "revision")?.clone(), Value::String(revision)),
        (
            lab(&labels, "date")?.clone(),
            Value::String(fmt_date(issued)),
        ),
        (lab(&labels, "validity")?.clone(), Value::String(validity)),
        (
            lab(&labels, "classification")?.clone(),
            doc.classification
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        ),
    ];
    let stamp = match request {
        Some(r) => approval_stamp(db, Some(r)).await?,
        None => None,
    };
    let mut sections = Vec::new();
    let mut figure: i64 = 0;
    let order: Vec<&(Value, Value)> = template
        .iter()
        .filter(|(k, _)| {
            blocks
                .iter()
                .any(|b| k.as_str() == Some(b.section.as_str()))
        })
        .collect();
    for (n, (key, section)) in order.into_iter().enumerate() {
        let mut out = Vec::new();
        for b in blocks
            .iter()
            .filter(|b| key.as_str() == Some(b.section.as_str()))
        {
            let d = as_dict(or(Some(&b.data), &empty), "block data")?;
            let mut item = Map::new();
            item.insert(
                "heading".into(),
                d.get("heading").cloned().unwrap_or(Value::Null),
            );
            match b.block_type.as_str() {
                "text" => {
                    let md = d.get("md").unwrap_or(&null);
                    let html = match md {
                        Value::String(s) => markdown_html(Some(s)),
                        v if !truthy(v) => String::new(),
                        v => {
                            return Err(ApiError::internal(anyhow::anyhow!(
                                "AttributeError: md {v} has no attribute 'strip'"
                            )))
                        }
                    };
                    item.insert("html".into(), Value::String(html));
                }
                "kv" => {
                    let mut kv = Vec::new();
                    for r in py_iter(or(d.get("rows"), &json!([])))? {
                        // a row that is not a list (stored before rows were cleaned on save) is skipped
                        let Value::Array(parts) = &r else { continue };
                        if parts.len() < 2 {
                            continue;
                        }
                        kv.push(json!([py_str(&parts[0]), py_str(&parts[1])]));
                    }
                    item.insert("kv".into(), Value::Array(kv));
                }
                "table" => {
                    let cols: Vec<Value> = py_iter(or(d.get("columns"), &json!([])))?
                        .iter()
                        .map(|c| json!({"label": py_str(c)}))
                        .collect();
                    let mut trs = Vec::new();
                    for r in py_iter(or(d.get("rows"), &json!([])))? {
                        let Value::Array(row) = &r else { continue };
                        let cells: Vec<Value> = row.iter().map(|c| cell(c, false)).collect();
                        trs.push(json!({"cells": cells}));
                    }
                    item.insert("table".into(), json!({"columns": cols, "rows": trs}));
                }
                "coverage" => {
                    let legend = match d.get("legend") {
                        Some(l) if truthy(l) => l.clone(),
                        _ => lab(&labels, "coverage_legend")?.clone(),
                    };
                    let mut crow = Vec::new();
                    for r in py_iter(or(d.get("rows"), &json!([])))? {
                        let Value::Object(r2) = &r else { continue };
                        let mut r2 = r2.clone();
                        let status = r2.get("status").cloned().unwrap_or(Value::Null);
                        let label = match &status {
                            Value::String(s) => labels.get(s).cloned().unwrap_or(status.clone()),
                            Value::Array(_) | Value::Object(_) => {
                                return Err(ApiError::internal(anyhow::anyhow!(
                                    "TypeError: unhashable type"
                                )))
                            }
                            other => other.clone(),
                        };
                        r2.insert("status_label".into(), label);
                        crow.push(Value::Object(r2));
                    }
                    item.insert("coverage".into(), json!({"legend": legend, "rows": crow}));
                }
                "image" => {
                    figure += 1;
                    let empty_s = Value::String(String::new());
                    let caption = or(d.get("caption"), &empty_s).clone();
                    let img = image(
                        db,
                        d.get("asset_id").unwrap_or(&null),
                        &caption,
                        &labels,
                        figure,
                    )
                    .await?;
                    if img.is_none() {
                        figure -= 1;
                    }
                    // visuals3 P3B: a picture may be half width (two per row), like a figure
                    let mut img = img;
                    if let Some(Value::Object(m)) = img.as_mut() {
                        if d.get("size").and_then(Value::as_str) == Some("half") {
                            m.insert("half".into(), Value::Bool(true));
                        }
                    }
                    item.insert("image".into(), img.unwrap_or(Value::Null));
                }
                "chart" | "diagram" => {
                    // visuals V2: the stored SVG as a data: URI (never redrawn on read); numbered with images
                    figure += 1;
                    let fig = figure_item(db, &b.data, &labels, figure).await?;
                    if fig.is_none() {
                        figure -= 1;
                    }
                    item.insert("figure".into(), fig.unwrap_or(Value::Null));
                }
                "commercial" => {
                    item.insert("commercial".into(), commercial(db, doc, &labels).await?);
                }
                "stamp" => {
                    item.insert("stamp".into(), json!({"approval": stamp.clone()}));
                }
                _ => {}
            }
            let keep =
                item.iter().any(|(k, v)| k != "heading" && truthy(v)) || truthy(&item["heading"]);
            if keep {
                out.push(Value::Object(item));
            }
        }
        let title = super::core_::section_title_text(section.get("title"))
            .map(Value::String)
            .unwrap_or_else(|| key.clone());
        sections.push(json!({"number": n + 1, "title": title, "blocks": out}));
    }
    let mut comp = as_dict(company, "company")?.clone();
    let trade = comp.get("trade_name").cloned().unwrap_or(Value::Null);
    comp.insert("company_trade".into(), trade);
    let cover_rows: Vec<Value> = rows
        .iter()
        .filter(|(_, v)| truthy(v))
        .map(|(a, b)| json!([a, b]))
        .collect();
    let data = json!({
        "language": doc.language, "title": doc.title, "subtitle": doc.subtitle, "cover_rows": cover_rows,
        "sections": sections, "labels": labels, "company": comp,
        "draft": doc.status == "draft" || doc.status == "review", "pdf_header": lab(&labels, "header")?,
    });
    let quotes = core::quotations_of(db, doc.id).await?;
    let mut totals = Vec::new();
    for (q, _) in &quotes {
        let cur = q
            .currency
            .clone()
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| "USD".into());
        let places = currency_places(db, Some(&cur)).await?;
        totals.push(fmt_money(q.total, q.currency.as_deref(), places));
    }
    let customer = match cover.get("prepared_for") {
        Some(v) if truthy(v) => py_str(v),
        _ => String::new(),
    };
    let facts = vec![
        (
            "Document".to_string(),
            Some(format!("Solution Proposal {reference}")),
        ),
        ("Customer".to_string(), Some(customer)),
        ("Title".to_string(), Some(doc.title.clone())),
        ("Issue date".to_string(), Some(fmt_date(issued))),
        ("Quotations".to_string(), Some(totals.join("; "))),
    ];
    Ok(("proposal".into(), data, facts))
}

/// The markdown-it-py renderer (see the module docs).
mod md {
    use markdown_it::common::utils::unescape_all;
    use markdown_it::parser::block::{BlockRule, BlockState};
    use markdown_it::parser::inline::InlineRoot;
    use markdown_it::plugins::cmark::block::blockquote::BlockquoteScanner;
    use markdown_it::plugins::cmark::block::heading::HeadingScanner;
    use markdown_it::plugins::cmark::block::hr::HrScanner;
    use markdown_it::plugins::cmark::block::list::{
        BulletList, ListItem, ListScanner, OrderedList,
    };
    use markdown_it::plugins::extra::tables::{
        ColumnAlignment, Table, TableBody, TableCell, TableHead, TableRow,
    };
    use markdown_it::{MarkdownIt, Node, NodeValue, Renderer};
    use once_cell::sync::Lazy;

    use granary_compat::text::pystr::{is_space as py_is_space, strip as py_strip};

    pub(super) static MD: Lazy<MarkdownIt> = Lazy::new(|| {
        use markdown_it::plugins::cmark::{block, inline};
        let mut md = MarkdownIt::new();
        md.max_nesting = 20;
        md.link_formatter = Box::new(PyLinkFormatter);
        inline::newline::add(&mut md);
        inline::escape::add(&mut md);
        md.inline.add_rule::<PyBackticksScanner>();
        {
            use markdown_it::plugins::cmark::inline::emphasis::{Em, Strong};
            emph_add_with::<'*', 1, true>(&mut md, || Node::new(Em { marker: '*' }));
            emph_add_with::<'_', 1, false>(&mut md, || Node::new(Em { marker: '_' }));
            emph_add_with::<'*', 2, true>(&mut md, || Node::new(Strong { marker: '*' }));
            emph_add_with::<'_', 2, false>(&mut md, || Node::new(Strong { marker: '_' }));
        }
        inline::link::add(&mut md);
        inline::image::add(&mut md);
        inline::autolink::add(&mut md);
        inline::entity::add(&mut md);
        block::code::add(&mut md);
        md.block.add_rule::<PyFenceScanner>();
        md.block.add_rule::<PyBlockquoteScanner>();
        block::hr::add(&mut md);
        md.block.add_rule::<PyListScanner>();
        md.block.add_rule::<PyReferenceScanner>();
        block::heading::add(&mut md);
        block::lheading::add(&mut md);
        md.block
            .add_rule::<PyParagraphScanner>()
            .after::<markdown_it::plugins::cmark::block::lheading::LHeadingScanner>()
            .after_all();
        md.block.add_rule::<PyTableScanner>().before_all();
        emph_add_with::<'~', 2, true>(&mut md, || {
            Node::new(markdown_it::plugins::extra::strikethrough::Strikethrough { marker: '~' })
        });
        md.add_rule::<PyInstallRefMap>()
            .before::<markdown_it::parser::block::builtin::BlockParserRule>();
        md.add_rule::<PyPostInline>()
            .after::<markdown_it::parser::inline::builtin::InlineParserRule>();
        md.add_rule::<PyStripInline>()
            .after::<markdown_it::parser::block::builtin::BlockParserRule>()
            .before::<markdown_it::parser::inline::builtin::InlineParserRule>();
        md
    });

    /// After the inline parse: markdown-it-py sets a link's or image's title only when it is not empty.
    pub struct PyPostInline;

    impl markdown_it::parser::core::CoreRule for PyPostInline {
        fn run(root: &mut Node, _md: &MarkdownIt) {
            use markdown_it::plugins::cmark::inline::{image::Image, link::Link};
            root.walk_mut(|node, _| {
                if let Some(l) = node.cast_mut::<Link>() {
                    if l.title.as_deref() == Some("") {
                        l.title = None;
                    }
                } else if let Some(i) = node.cast_mut::<Image>() {
                    if i.title.as_deref() == Some("") {
                        i.title = None;
                    }
                }
            });
        }
    }

    /// markdown-it-py strips paragraph and heading text with Python's `str.strip()` (every Unicode space,
    /// NBSP included) before the inline parse; the crate trims only spaces and tabs.
    pub struct PyStripInline;

    impl markdown_it::parser::core::CoreRule for PyStripInline {
        fn run(root: &mut Node, _md: &MarkdownIt) {
            use markdown_it::plugins::cmark::block::{
                heading::ATXHeading, lheading::SetextHeader, paragraph::Paragraph,
            };
            root.walk_mut(|node, _| {
                if node.is::<ListItem>() {
                    for i in 1..node.children.len() {
                        if node.children[i - 1].is::<InlineRoot>() {
                            if let Some(f) = node.children[i].cast_mut::<PyCodeFence>() {
                                f.after_tight_text = true;
                            }
                        }
                    }
                }
                // (a tight list item holds its paragraph's inline content directly)
                if !(node.is::<Paragraph>()
                    || node.is::<ATXHeading>()
                    || node.is::<SetextHeader>()
                    || node.is::<ListItem>())
                {
                    return;
                }
                for child in node.children.iter_mut() {
                    if let Some(inline) = child.cast_mut::<InlineRoot>() {
                        let stripped = py_strip(&inline.content);
                        if stripped.len() == inline.content.len() {
                            continue;
                        }
                        let lead = inline.content.len()
                            - inline.content.trim_start_matches(py_is_space).len();
                        let new_len = stripped.len();
                        // (content offset, source offset) breakpoints, shifted past the stripped lead
                        let mut first = (0usize, lead);
                        for &(d, s) in inline.mapping.iter() {
                            if d <= lead {
                                first = (0, s + (lead - d));
                            }
                        }
                        let mut mapping = vec![first];
                        for &(d, s) in inline.mapping.iter() {
                            if d > lead && d - lead < new_len {
                                mapping.push((d - lead, s));
                            }
                        }
                        inline.content = stripped.to_owned();
                        inline.mapping = mapping;
                    }
                }
            });
        }
    }

    pub fn md_render(md: &str) -> String {
        // markdown-it-py's normalize core rule: \r\n and \r become \n, NUL becomes U+FFFD
        let mut src = md
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\0', "\u{FFFD}");
        // StateBlock never registers a last line made only of spaces and tabs with no line feed after it
        let last_start = src.rfind('\n').map(|p| p + 1).unwrap_or(0);
        if src[last_start..].chars().all(|c| c == ' ' || c == '\t') {
            src.truncate(last_start);
        }
        MD.parse(&src).xrender()
    }

    // ---------------------------------------------------------------------------------------- fence

    /// The crate's fence rule, except that the last line of an unclosed fence that ends the source without a
    /// line feed keeps no line feed (markdown-it-py's getLines slices past the end of the source).
    pub struct PyFenceScanner;

    impl PyFenceScanner {
        fn get_header<'a>(state: &'a mut BlockState) -> Option<(char, usize, &'a str)> {
            if state.line_indent(state.line) >= state.md.max_indent {
                return None;
            }
            let line = state.get_line(state.line);
            let mut chars = line.chars();
            let marker = chars.next()?;
            if marker != '~' && marker != '`' {
                return None;
            }
            let mut len = 1;
            while Some(marker) == chars.next() {
                len += 1;
            }
            if len < 3 {
                return None;
            }
            let params = &line[len..];
            if marker == '`' && params.contains(marker) {
                return None;
            }
            Some((marker, len, params))
        }
    }

    impl BlockRule for PyFenceScanner {
        fn check(state: &mut BlockState) -> Option<()> {
            Self::get_header(state).map(|_| ())
        }

        fn run(state: &mut BlockState) -> Option<(Node, usize)> {
            let (marker, len, params) = Self::get_header(state)?;
            let params = params.to_owned();
            let mut next_line = state.line;
            let mut have_end_marker = false;
            'outer: loop {
                next_line += 1;
                if next_line >= state.line_max {
                    break;
                }
                let line = state.get_line(next_line);
                if !line.is_empty() && state.line_indent(next_line) < 0 {
                    break;
                }
                let mut chars = line.chars().peekable();
                if Some(marker) != chars.next() {
                    continue;
                }
                if state.line_indent(next_line) >= state.md.max_indent {
                    continue;
                }
                let mut len_end = 1;
                while Some(&marker) == chars.peek() {
                    chars.next();
                    len_end += 1;
                }
                if len_end < len {
                    continue;
                }
                loop {
                    match chars.next() {
                        Some(' ' | '\t') => {}
                        Some(_) => continue 'outer,
                        None => {
                            have_end_marker = true;
                            break 'outer;
                        }
                    }
                }
            }
            let indent = state.line_offsets[state.line].indent_nonspace;
            let (mut content, _) =
                state.get_lines(state.line + 1, next_line, indent as usize, true);
            if next_line > state.line + 1
                && state.line_offsets[next_line - 1].line_end >= state.src.len()
            {
                // the source ends on this line, with no line feed after it
                if content.ends_with('\n') {
                    content.pop();
                }
            }
            let node = Node::new(PyCodeFence {
                info: params,
                content,
                after_tight_text: false,
            });
            Some((
                node,
                next_line - state.line + if have_end_marker { 1 } else { 0 },
            ))
        }
    }

    /// The fence as markdown-it-py renders it: `info = unescapeAll(token.info).strip()`, language = the first
    /// `str.split()` word (Python whitespace), class "language-<lang>".
    #[derive(Debug)]
    pub struct PyCodeFence {
        pub info: String,
        pub content: String,
        /// Right after a tight list item's text: markdown-it-py's fence renderer adds no line feed before
        /// `<pre>` and the hidden paragraph adds none either.
        pub after_tight_text: bool,
    }

    impl NodeValue for PyCodeFence {
        fn render(&self, node: &Node, fmt: &mut dyn Renderer) {
            let info = unescape_all(&self.info);
            let info = py_strip(&info);
            let lang = info
                .split(py_is_space)
                .find(|w| !w.is_empty())
                .unwrap_or("");
            let mut attrs = node.attrs.clone();
            if !info.is_empty() {
                attrs.push(("class", format!("language-{lang}")));
            }
            if !self.after_tight_text {
                fmt.cr();
            }
            fmt.open("pre", &[]);
            fmt.open("code", &attrs);
            fmt.text(&self.content);
            fmt.close("code");
            fmt.close("pre");
            fmt.cr();
        }
    }

    // ----------------------------------------------------------------------------------- blockquote

    /// Python's blockquote renders an empty quote as `<blockquote></blockquote>`.
    #[derive(Debug)]
    pub struct PyBlockquote;

    impl NodeValue for PyBlockquote {
        fn render(&self, node: &Node, fmt: &mut dyn Renderer) {
            fmt.cr();
            fmt.open("blockquote", &node.attrs);
            // markdown-it-py puts no line feed between an opening tag and its own closing tag; a quote
            // holding only reference definitions renders nothing inside either.
            if node.children.iter().all(|c| c.xrender().is_empty()) {
                fmt.close("blockquote");
                fmt.cr();
                return;
            }
            fmt.cr();
            fmt.contents(&node.children);
            fmt.cr();
            fmt.close("blockquote");
            fmt.cr();
        }
    }

    /// The terminators markdown-it-py's blockquote and table rules use (`getRules("blockquote")`: fence,
    /// blockquote, hr, list, html_block (off), heading), with `parentType` not "paragraph".
    fn blockquote_terminators(state: &mut BlockState) -> bool {
        PyFenceScanner::check(state).is_some()
            || BlockquoteScanner::check(state).is_some()
            || HrScanner::check(state).is_some()
            || py_list_check(state, false)
            || HeadingScanner::check(state).is_some()
    }

    fn skip_ordered_list_marker(line: &str) -> Option<usize> {
        let mut chars = line.chars();
        let mut pos = 0usize;
        match chars.next() {
            Some('0'..='9') => pos += 1,
            _ => return None,
        }
        loop {
            match chars.next() {
                Some('0'..='9') => {
                    pos += 1;
                    if pos >= 10 {
                        return None;
                    }
                }
                Some(')' | '.') => {
                    pos += 1;
                    break;
                }
                _ => return None,
            }
        }
        match chars.next() {
            Some(' ' | '\t') | None => Some(pos),
            Some(_) => None,
        }
    }

    fn skip_bullet_list_marker(line: &str) -> Option<usize> {
        let mut chars = line.chars();
        match chars.next() {
            Some('*' | '-' | '+') => {}
            _ => return None,
        }
        match chars.next() {
            Some(' ' | '\t') | None => Some(1),
            Some(_) => None,
        }
    }

    /// markdown-it-py `list(state, line, endLine, silent=True)`; `paragraph` is `state.parentType == "paragraph"`.
    fn py_list_check(state: &mut BlockState, paragraph: bool) -> bool {
        if state.line_indent(state.line) >= state.md.max_indent {
            return false;
        }
        if let Some(list_indent) = state.list_indent {
            let ind = state.line_offsets[state.line].indent_nonspace;
            if ind - list_indent as i32 >= state.md.max_indent && ind < state.blk_indent as i32 {
                return false;
            }
        }
        let terminating = paragraph && state.line_indent(state.line) >= 0;
        let line = state.get_line(state.line);
        let pos = if let Some(p) = skip_ordered_list_marker(line) {
            let v: u64 = line[..p - 1].parse().unwrap_or(0);
            if terminating && v != 1 {
                return false;
            }
            p
        } else if let Some(p) = skip_bullet_list_marker(line) {
            p
        } else {
            return false;
        };
        !(terminating && line[pos..].chars().all(|c| c == ' ' || c == '\t'))
    }

    pub struct PyBlockquoteScanner;

    impl BlockRule for PyBlockquoteScanner {
        fn check(state: &mut BlockState) -> Option<()> {
            BlockquoteScanner::check(state)
        }

        fn run(state: &mut BlockState) -> Option<(Node, usize)> {
            Self::check(state)?;
            let mut old_line_offsets = Vec::new();
            let start_line = state.line;
            let mut next_line = state.line;
            let mut last_line_empty = false;
            while next_line < state.line_max {
                let is_outdented = state.line_indent(next_line) < 0;
                let line = state.get_line(next_line).to_owned();
                let mut chars = line.chars();
                match chars.next() {
                    None => break,
                    Some('>') if !is_outdented => {
                        let offsets = &state.line_offsets[next_line];
                        let pos_after_marker = offsets.first_nonspace + 1;
                        old_line_offsets.push(state.line_offsets[next_line].clone());
                        let (mut indent_after_marker, first_nonspace) =
                            markdown_it::common::utils::find_indent_of(
                                &state.src[offsets.line_start..offsets.line_end],
                                pos_after_marker - offsets.line_start,
                            );
                        last_line_empty = first_nonspace == offsets.line_end - offsets.line_start;
                        if matches!(chars.next(), Some(' ' | '\t')) {
                            indent_after_marker -= 1;
                        }
                        state.line_offsets[next_line].indent_nonspace = indent_after_marker as i32;
                        state.line_offsets[next_line].first_nonspace =
                            first_nonspace + state.line_offsets[next_line].line_start;
                        next_line += 1;
                        continue;
                    }
                    _ => {}
                }
                if last_line_empty {
                    break;
                }
                let old_line = state.line;
                state.line = next_line;
                let terminate = blockquote_terminators(state);
                state.line = old_line;
                if terminate {
                    if state.blk_indent != 0 {
                        old_line_offsets.push(state.line_offsets[next_line].clone());
                        state.line_offsets[next_line].indent_nonspace -= state.blk_indent as i32;
                    }
                    break;
                }
                old_line_offsets.push(state.line_offsets[next_line].clone());
                // remember the indent markdown-it-py's getLines still sees on this lazy line (see PyParagraphScanner)
                let key = state.line_offsets[next_line].line_start;
                let ind = state.line_offsets[next_line].indent_nonspace;
                state
                    .root_ext
                    .get_or_insert_default::<LazyIndents>()
                    .0
                    .insert(key, ind);
                state.line_offsets[next_line].indent_nonspace = -1;
                next_line += 1;
            }
            let old_indent = state.blk_indent;
            state.blk_indent = 0;
            let old_node = std::mem::replace(&mut state.node, Node::new(PyBlockquote));
            let old_line_max = state.line_max;
            state.line = start_line;
            state.line_max = next_line;
            // markdown-it-py's block nesting level: blockquote_open is one level
            state.level += 1;
            state.md.block.tokenize(state);
            state.level -= 1;
            next_line = state.line;
            state.line = start_line;
            state.line_max = old_line_max;
            for (idx, line_offset) in old_line_offsets.iter_mut().enumerate() {
                std::mem::swap(&mut state.line_offsets[idx + start_line], line_offset);
            }
            state.blk_indent = old_indent;
            let node = std::mem::replace(&mut state.node, old_node);
            Some((node, next_line - start_line))
        }
    }

    /// The crate's list rule, counted as two block levels (bullet_list_open + list_item_open) for the
    /// maxNesting limit (20 in the commonmark preset) as markdown-it-py counts them.
    pub struct PyListScanner;

    impl BlockRule for PyListScanner {
        fn check(state: &mut BlockState) -> Option<()> {
            ListScanner::check(state)
        }

        fn run(state: &mut BlockState) -> Option<(Node, usize)> {
            state.level += 2;
            let out = ListScanner::run(state);
            state.level -= 2;
            out
        }
    }

    // ---------------------------------------------------------------------------------------- table

    /// markdown-it-py `escapedSplit`.
    fn escaped_split(s: &str) -> Vec<String> {
        let chars: Vec<char> = s.chars().collect();
        let mut result = Vec::new();
        let mut current = String::new();
        let mut last = 0usize;
        let mut escaped = false;
        for (pos, &ch) in chars.iter().enumerate() {
            if ch == '|' {
                if !escaped {
                    current.extend(&chars[last..pos]);
                    result.push(std::mem::take(&mut current));
                    last = pos + 1;
                } else {
                    current.extend(&chars[last..pos - 1]);
                    last = pos;
                }
            }
            escaped = ch == '\\';
        }
        current.extend(&chars[last..]);
        result.push(current);
        result
    }

    fn split_row(line: &str) -> Vec<String> {
        let mut columns = escaped_split(line);
        if columns.first().is_some_and(|c| c.is_empty()) {
            columns.remove(0);
        }
        if columns.last().is_some_and(|c| c.is_empty()) {
            columns.pop();
        }
        columns
    }

    /// markdown-it-py's table rule (GFM), ported as is: its header and delimiter checks, `escapedSplit`, Python
    /// `str.strip` on cells, the blockquote terminators for body rows; it never terminates a list or a lazy
    /// blockquote line (it is only a paragraph/reference terminator there).
    pub struct PyTableScanner;

    impl PyTableScanner {
        fn header(state: &BlockState) -> Option<(Vec<String>, Vec<ColumnAlignment>)> {
            let start = state.line;
            if start + 2 > state.line_max {
                return None;
            }
            let next = start + 1;
            if state.line_indent(next) < 0 {
                return None;
            }
            if state.line_indent(next) >= 4 {
                return None;
            }
            let delim = state.get_line(next);
            let mut it = delim.chars();
            let first = it.next()?;
            if !matches!(first, '|' | '-' | ':') {
                return None;
            }
            let second = it.next()?;
            if !matches!(second, '|' | '-' | ':' | ' ' | '\t') {
                return None;
            }
            if first == '-' && matches!(second, ' ' | '\t') {
                return None;
            }
            if !it.all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t')) {
                return None;
            }
            let columns: Vec<&str> = delim.split('|').collect();
            let mut aligns = Vec::new();
            for (i, col) in columns.iter().enumerate() {
                let t = py_strip(col);
                if t.is_empty() {
                    if i == 0 || i == columns.len() - 1 {
                        continue;
                    }
                    return None;
                }
                let inner = t.strip_prefix(':').unwrap_or(t);
                let inner = inner.strip_suffix(':').unwrap_or(inner);
                if inner.is_empty() || !inner.chars().all(|c| c == '-') {
                    return None;
                }
                aligns.push(if t.ends_with(':') {
                    if t.starts_with(':') {
                        ColumnAlignment::Center
                    } else {
                        ColumnAlignment::Right
                    }
                } else if t.starts_with(':') {
                    ColumnAlignment::Left
                } else {
                    ColumnAlignment::None
                });
            }
            let head = py_strip(state.get_line(start));
            if !head.contains('|') {
                return None;
            }
            if state.line_indent(start) >= 4 {
                return None;
            }
            let columns = split_row(head);
            if columns.is_empty() || columns.len() != aligns.len() {
                return None;
            }
            Some((columns, aligns))
        }
    }

    fn cell(content: &str, offset: usize) -> Node {
        let mut n = Node::new(TableCell);
        if !content.is_empty() {
            n.children.push(Node::new(InlineRoot::new(
                content.to_owned(),
                vec![(0, offset)],
            )));
        }
        n
    }

    impl BlockRule for PyTableScanner {
        fn check(state: &mut BlockState) -> Option<()> {
            // Python's table is a terminator for paragraphs and references only, not for lists.
            if state.node.is::<BulletList>()
                || state.node.is::<OrderedList>()
                || state.node.is::<TableBody>()
            {
                return None;
            }
            Self::header(state).map(|_| ())
        }

        fn run(state: &mut BlockState) -> Option<(Node, usize)> {
            let (columns, aligns) = Self::header(state)?;
            let count = columns.len();
            let offset = state.line_offsets[state.line].first_nonspace;
            let mut table = Node::new(Table { alignments: aligns });
            let mut thead = Node::new(TableHead);
            let mut row = Node::new(TableRow);
            for c in &columns {
                row.children.push(cell(py_strip(c), offset));
            }
            thead.children.push(row);
            table.children.push(thead);
            let start = state.line;
            let mut tbody = Node::new(TableBody);
            let mut next = start + 2;
            while next < state.line_max {
                if state.line_indent(next) < 0 {
                    break;
                }
                let old_line = state.line;
                state.line = next;
                let terminate = blockquote_terminators(state);
                state.line = old_line;
                if terminate {
                    break;
                }
                let text = py_strip(state.get_line(next));
                if text.is_empty() {
                    break;
                }
                if state.line_indent(next) >= 4 {
                    break;
                }
                let cols = split_row(text);
                let offset = state.line_offsets[next].first_nonspace;
                let mut row = Node::new(TableRow);
                for i in 0..count {
                    let content = cols.get(i).map(|c| py_strip(c)).unwrap_or("");
                    row.children.push(cell(content, offset));
                }
                tbody.children.push(row);
                next += 1;
            }
            if !tbody.children.is_empty() {
                table.children.push(tbody);
            }
            Some((table, next - start))
        }
    }

    #[allow(dead_code)]
    fn _unused() {
        let _ = unescape_all("");
    }

    // ---------------------------------------------------------------------------------------- code spans

    #[derive(Debug, Default)]
    struct BacktickCache {
        scanned: bool,
        max: Vec<usize>,
    }
    impl markdown_it::parser::extset::InlineRootExt for BacktickCache {}

    /// The crate's code span rule, except that the one space each side is removed when the content is not
    /// all (Python `str.strip`) whitespace, as markdown-it-py does; the crate tests `len > 2`.
    pub struct PyBackticksScanner;

    impl markdown_it::parser::inline::InlineRule for PyBackticksScanner {
        const MARKER: char = '`';

        fn run(state: &mut markdown_it::parser::inline::InlineState) -> Option<(Node, usize)> {
            use markdown_it::parser::inline::Text;
            use markdown_it::plugins::cmark::inline::backticks::CodeInline;
            let mut chars = state.src[state.pos..state.pos_max].chars();
            if chars.next()? != '`' {
                return None;
            }
            let mut pos = state.pos + 1;
            while Some('`') == chars.next() {
                pos += 1;
            }
            let cache = state.inline_ext.get_or_insert_default::<BacktickCache>();
            let opener_len = pos - state.pos;
            // markdown-it-py: an unmatched run is text as a whole (the crate skips one backtick at a time,
            // which lets the rest of the run open a code span)
            let as_text = |len: usize| {
                Some((
                    Node::new(Text {
                        content: "`".repeat(len),
                    }),
                    len,
                ))
            };
            if cache.scanned && cache.max.get(opener_len).copied().unwrap_or(0) <= state.pos {
                return as_text(opener_len);
            }
            let mut match_end = pos;
            while let Some(p) = state.src[match_end..state.pos_max].find('`') {
                let mut match_start = match_end + p;
                match_end = match_start + 1;
                chars = state.src[match_end..state.pos_max].chars();
                while Some('`') == chars.next() {
                    match_end += 1;
                }
                let closer_len = match_end - match_start;
                if closer_len == opener_len {
                    let mut content = state.src[pos..match_start].replace('\n', " ");
                    if content.starts_with(' ')
                        && content.ends_with(' ')
                        && !py_strip(&content).is_empty()
                    {
                        content = content[1..content.len() - 1].to_owned();
                        pos += 1;
                        match_start -= 1;
                    }
                    let mut node = Node::new(CodeInline {
                        marker: '`',
                        marker_len: opener_len,
                    });
                    let mut inner = Node::new(Text { content });
                    inner.srcmap = state.get_map(pos, match_start.max(pos));
                    node.children.push(inner);
                    return Some((node, match_end - state.pos));
                }
                let cache = state.inline_ext.get_mut::<BacktickCache>().unwrap();
                while cache.max.len() <= closer_len {
                    cache.max.push(0);
                }
                cache.max[closer_len] = match_start;
            }
            let cache = state.inline_ext.get_mut::<BacktickCache>().unwrap();
            cache.scanned = true;
            as_text(opener_len)
        }
    }

    // ---------------------------------------------------------------------------------------- emphasis

    /// markdown-it-py `isPunctChar`: its UNICODE_PUNCT_RE, BMP only (the astral alternatives are written as
    /// surrogate pairs, which never match a Python str).
    const PY_PUNCT: &[(u32, u32)] = &[
        (0x0021, 0x0023),
        (0x0025, 0x002A),
        (0x002C, 0x002F),
        (0x003A, 0x003B),
        (0x003F, 0x0040),
        (0x005B, 0x005D),
        (0x005F, 0x005F),
        (0x007B, 0x007B),
        (0x007D, 0x007D),
        (0x00A1, 0x00A1),
        (0x00A7, 0x00A7),
        (0x00AB, 0x00AB),
        (0x00B6, 0x00B7),
        (0x00BB, 0x00BB),
        (0x00BF, 0x00BF),
        (0x037E, 0x037E),
        (0x0387, 0x0387),
        (0x055A, 0x055F),
        (0x0589, 0x058A),
        (0x05BE, 0x05BE),
        (0x05C0, 0x05C0),
        (0x05C3, 0x05C3),
        (0x05C6, 0x05C6),
        (0x05F3, 0x05F4),
        (0x0609, 0x060A),
        (0x060C, 0x060D),
        (0x061B, 0x061B),
        (0x061E, 0x061F),
        (0x066A, 0x066D),
        (0x06D4, 0x06D4),
        (0x0700, 0x070D),
        (0x07F7, 0x07F9),
        (0x0830, 0x083E),
        (0x085E, 0x085E),
        (0x0964, 0x0965),
        (0x0970, 0x0970),
        (0x09FD, 0x09FD),
        (0x0A76, 0x0A76),
        (0x0AF0, 0x0AF0),
        (0x0C84, 0x0C84),
        (0x0DF4, 0x0DF4),
        (0x0E4F, 0x0E4F),
        (0x0E5A, 0x0E5B),
        (0x0F04, 0x0F12),
        (0x0F14, 0x0F14),
        (0x0F3A, 0x0F3D),
        (0x0F85, 0x0F85),
        (0x0FD0, 0x0FD4),
        (0x0FD9, 0x0FDA),
        (0x104A, 0x104F),
        (0x10FB, 0x10FB),
        (0x1360, 0x1368),
        (0x1400, 0x1400),
        (0x166D, 0x166E),
        (0x169B, 0x169C),
        (0x16EB, 0x16ED),
        (0x1735, 0x1736),
        (0x17D4, 0x17D6),
        (0x17D8, 0x17DA),
        (0x1800, 0x180A),
        (0x1944, 0x1945),
        (0x1A1E, 0x1A1F),
        (0x1AA0, 0x1AA6),
        (0x1AA8, 0x1AAD),
        (0x1B5A, 0x1B60),
        (0x1BFC, 0x1BFF),
        (0x1C3B, 0x1C3F),
        (0x1C7E, 0x1C7F),
        (0x1CC0, 0x1CC7),
        (0x1CD3, 0x1CD3),
        (0x2010, 0x2027),
        (0x2030, 0x2043),
        (0x2045, 0x2051),
        (0x2053, 0x205E),
        (0x207D, 0x207E),
        (0x208D, 0x208E),
        (0x2308, 0x230B),
        (0x2329, 0x232A),
        (0x2768, 0x2775),
        (0x27C5, 0x27C6),
        (0x27E6, 0x27EF),
        (0x2983, 0x2998),
        (0x29D8, 0x29DB),
        (0x29FC, 0x29FD),
        (0x2CF9, 0x2CFC),
        (0x2CFE, 0x2CFF),
        (0x2D70, 0x2D70),
        (0x2E00, 0x2E2E),
        (0x2E30, 0x2E4E),
        (0x3001, 0x3003),
        (0x3008, 0x3011),
        (0x3014, 0x301F),
        (0x3030, 0x3030),
        (0x303D, 0x303D),
        (0x30A0, 0x30A0),
        (0x30FB, 0x30FB),
        (0xA4FE, 0xA4FF),
        (0xA60D, 0xA60F),
        (0xA673, 0xA673),
        (0xA67E, 0xA67E),
        (0xA6F2, 0xA6F7),
        (0xA874, 0xA877),
        (0xA8CE, 0xA8CF),
        (0xA8F8, 0xA8FA),
        (0xA8FC, 0xA8FC),
        (0xA92E, 0xA92F),
        (0xA95F, 0xA95F),
        (0xA9C1, 0xA9CD),
        (0xA9DE, 0xA9DF),
        (0xAA5C, 0xAA5F),
        (0xAADE, 0xAADF),
        (0xAAF0, 0xAAF1),
        (0xABEB, 0xABEB),
        (0xFD3E, 0xFD3F),
        (0xFE10, 0xFE19),
        (0xFE30, 0xFE52),
        (0xFE54, 0xFE61),
        (0xFE63, 0xFE63),
        (0xFE68, 0xFE68),
        (0xFE6A, 0xFE6B),
        (0xFF01, 0xFF03),
        (0xFF05, 0xFF0A),
        (0xFF0C, 0xFF0F),
        (0xFF1A, 0xFF1B),
        (0xFF1F, 0xFF20),
        (0xFF3B, 0xFF3D),
        (0xFF3F, 0xFF3F),
        (0xFF5B, 0xFF5B),
        (0xFF5D, 0xFF5D),
        (0xFF5F, 0xFF65),
    ];

    fn py_is_punct_char(c: char) -> bool {
        let cp = c as u32;
        PY_PUNCT
            .binary_search_by(|&(a, b)| {
                if b < cp {
                    std::cmp::Ordering::Less
                } else if a > cp {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }

    /// markdown-it-py `isWhiteSpace` (Zs and \t\n\v\f\r; not U+0085, U+2028, U+2029 as Rust's is_whitespace).
    fn py_is_white_space(c: char) -> bool {
        matches!(
            c,
            '\t' | '\n' | '\x0b' | '\x0c' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
                ..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
        )
    }

    struct Scanned {
        can_open: bool,
        can_close: bool,
        length: usize,
    }

    /// markdown-it-py `StateInline.scanDelims`.
    fn py_scan_delims(
        state: &markdown_it::parser::inline::InlineState,
        start: usize,
        can_split_word: bool,
    ) -> Scanned {
        let last_char = if start > 0 {
            state.src[..start].chars().next_back().unwrap()
        } else {
            ' '
        };
        let mut chars = state.src[start..state.pos_max].chars();
        let marker = chars.next().unwrap();
        let mut length = 1;
        let next_char = loop {
            match chars.next() {
                None => break ' ',
                Some(x) if x == marker => length += 1,
                Some(x) => break x,
            }
        };
        let last_punct = last_char.is_ascii_punctuation() || py_is_punct_char(last_char);
        let next_punct = next_char.is_ascii_punctuation() || py_is_punct_char(next_char);
        let last_ws = py_is_white_space(last_char);
        let next_ws = py_is_white_space(next_char);
        let left = !(next_ws || (next_punct && !(last_ws || last_punct)));
        let right = !(last_ws || (last_punct && !(next_ws || next_punct)));
        if !can_split_word {
            Scanned {
                can_open: left && (!right || last_punct),
                can_close: right && (!left || next_punct),
                length,
            }
        } else {
            Scanned {
                can_open: left,
                can_close: right,
                length,
            }
        }
    }

    // The crate's emph_pair (emphasis, strong, strikethrough), unchanged except that delimiter runs are
    // classified by `py_scan_delims`.

    #[derive(Debug, Default)]
    struct PyPairConfig<const MARKER: char> {
        inserted: bool,
        fns: [Option<fn() -> Node>; 3],
    }
    impl<const MARKER: char> markdown_it::parser::extset::MarkdownItExt for PyPairConfig<MARKER> {}

    #[derive(Debug, Default)]
    struct PyOpenersBottom<const MARKER: char>([usize; 6]);
    impl<const MARKER: char> markdown_it::parser::extset::NodeExt for PyOpenersBottom<MARKER> {}

    #[derive(Debug, Clone)]
    struct PyEmphMarker {
        marker: char,
        length: usize,
        remaining: usize,
        open: bool,
        close: bool,
    }
    impl NodeValue for PyEmphMarker {}

    fn emph_add_with<const MARKER: char, const LENGTH: u8, const CAN_SPLIT_WORD: bool>(
        md: &mut MarkdownIt,
        f: fn() -> Node,
    ) {
        let cfg = md.ext.get_or_insert_default::<PyPairConfig<MARKER>>();
        cfg.fns[LENGTH as usize - 1] = Some(f);
        if !cfg.inserted {
            cfg.inserted = true;
            md.inline
                .add_rule::<PyEmphPairScanner<MARKER, CAN_SPLIT_WORD>>();
        }
        if !md.has_rule::<PyFragmentsJoin>() {
            md.add_rule::<PyFragmentsJoin>()
                .before_all()
                .after::<markdown_it::parser::inline::builtin::InlineParserRule>();
        }
    }

    struct PyEmphPairScanner<const MARKER: char, const CAN_SPLIT_WORD: bool>;
    impl<const MARKER: char, const CAN_SPLIT_WORD: bool> markdown_it::parser::inline::InlineRule
        for PyEmphPairScanner<MARKER, CAN_SPLIT_WORD>
    {
        const MARKER: char = MARKER;

        fn check(_: &mut markdown_it::parser::inline::InlineState) -> Option<usize> {
            None
        }

        fn run(state: &mut markdown_it::parser::inline::InlineState) -> Option<(Node, usize)> {
            let mut chars = state.src[state.pos..state.pos_max].chars();
            if chars.next().unwrap() != MARKER {
                return None;
            }
            let scanned = py_scan_delims(state, state.pos, CAN_SPLIT_WORD);
            let mut node = Node::new(PyEmphMarker {
                marker: MARKER,
                length: scanned.length,
                remaining: scanned.length,
                open: scanned.can_open,
                close: scanned.can_close,
            });
            node.srcmap = state.get_map(state.pos, state.pos + scanned.length);
            node = scan_and_match_delimiters::<MARKER>(state, node);
            // The crate measures the token in source bytes, which differ from content bytes on lazy
            // blockquote lines (an overflow panic in the crate); clamped here, only the source map changes.
            let map = node.srcmap.unwrap().get_byte_offsets();
            let end = state.pos + scanned.length;
            let token_len = map.1.saturating_sub(map.0).min(end);
            state.pos = end - token_len;
            Some((node, token_len))
        }
    }

    fn scan_and_match_delimiters<const MARKER: char>(
        state: &mut markdown_it::parser::inline::InlineState,
        mut closer_token: Node,
    ) -> Node {
        use markdown_it::common::sourcemap::SourcePos;
        use std::cmp::min;
        if state.node.children.is_empty() {
            return closer_token;
        }
        let mut closer = closer_token.cast_mut::<PyEmphMarker>().unwrap().clone();
        if !closer.close {
            return closer_token;
        }
        let openers_for_marker = state
            .node
            .ext
            .get_or_insert_default::<PyOpenersBottom<MARKER>>();
        let openers_parameter = (closer.open as usize) * 3 + closer.length % 3;
        let min_opener_idx = openers_for_marker.0[openers_parameter];
        let mut idx = state.node.children.len() - 1;
        let mut new_min_opener_idx = idx;
        while idx > min_opener_idx {
            idx -= 1;
            let Some(opener) = state.node.children[idx].cast::<PyEmphMarker>() else {
                continue;
            };
            let mut opener = opener.clone();
            if opener.open && opener.marker == closer.marker && !is_odd_match(&opener, &closer) {
                while closer.remaining > 0 && opener.remaining > 0 {
                    let max_marker_len = min(3, min(opener.remaining, closer.remaining));
                    let mut matched_rule = None;
                    let fns = &state.md.ext.get::<PyPairConfig<MARKER>>().unwrap().fns;
                    for marker_len in (1..=max_marker_len).rev() {
                        if let Some(f) = fns[marker_len - 1] {
                            matched_rule = Some((marker_len, f));
                            break;
                        }
                    }
                    let Some((marker_len, marker_fn)) = matched_rule else {
                        break;
                    };
                    closer.remaining -= marker_len;
                    opener.remaining -= marker_len;
                    let mut new_token = marker_fn();
                    new_token.children = state.node.children.split_off(idx + 1);
                    let mut end_map_pos = 0;
                    if let Some(map) = closer_token.srcmap {
                        let (start, end) = map.get_byte_offsets();
                        closer_token.srcmap = Some(SourcePos::new(start + marker_len, end));
                        end_map_pos = start + marker_len;
                    }
                    let mut start_map_pos = 0;
                    let opener_token = state.node.children.last_mut().unwrap();
                    if let Some(map) = opener_token.srcmap {
                        let (start, end) = map.get_byte_offsets();
                        opener_token.srcmap = Some(SourcePos::new(start, end - marker_len));
                        start_map_pos = end - marker_len;
                    }
                    new_token.srcmap = state.get_map(start_map_pos, end_map_pos);
                    if opener.remaining == 0 {
                        state.node.children.pop();
                    }
                    new_min_opener_idx = 0;
                    state.node.children.push(new_token);
                }
            }
            if opener.remaining > 0 {
                state.node.children[idx].replace(opener);
            }
        }
        if new_min_opener_idx != 0 {
            let openers_for_marker = state
                .node
                .ext
                .get_or_insert_default::<PyOpenersBottom<MARKER>>();
            openers_for_marker.0[openers_parameter] = new_min_opener_idx;
        }
        if closer.remaining > 0 {
            closer_token.replace(closer);
            closer_token
        } else {
            state.node.children.pop().unwrap()
        }
    }

    fn is_odd_match(opener: &PyEmphMarker, closer: &PyEmphMarker) -> bool {
        (opener.close || closer.open)
            && (opener.length + closer.length).is_multiple_of(3)
            && (!opener.length.is_multiple_of(3) || !closer.length.is_multiple_of(3))
    }

    struct PyFragmentsJoin;
    impl markdown_it::parser::core::CoreRule for PyFragmentsJoin {
        fn run(node: &mut Node, _: &MarkdownIt) {
            node.walk_mut(|node, _| fragments_join(node));
        }
    }

    fn fragments_join(node: &mut Node) {
        use markdown_it::common::sourcemap::SourcePos;
        use markdown_it::parser::inline::Text;
        for token in node.children.iter_mut() {
            if let Some(data) = token.cast::<PyEmphMarker>() {
                let content = data.marker.to_string().repeat(data.remaining);
                token.replace(Text { content });
            }
        }
        for idx in 1..node.children.len() {
            let (tokens1, tokens2) = node.children.split_at_mut(idx);
            let token1 = tokens1.last_mut().unwrap();
            let Some(t1_data) = token1.cast_mut::<Text>() else {
                continue;
            };
            let token2 = tokens2.first_mut().unwrap();
            let Some(t2_data) = token2.cast_mut::<Text>() else {
                continue;
            };
            let t2_content = std::mem::take(&mut t2_data.content);
            t1_data.content += &t2_content;
            if let (Some(map1), Some(map2)) = (token1.srcmap, token2.srcmap) {
                token1.srcmap = Some(SourcePos::new(
                    map1.get_byte_offsets().0,
                    map2.get_byte_offsets().1,
                ));
            }
            node.children.swap(idx - 1, idx);
        }
        node.children.retain(|token| {
            token
                .cast::<Text>()
                .map(|t| !t.content.is_empty())
                .unwrap_or(true)
        });
    }

    // ---------------------------------------------------------------------------------------- links

    /// markdown-it-py's normalizeLink / normalizeLinkText / validateLink, with its mdurl (parse, format,
    /// encode, decode) and _punycode, ported as written (Python `str.strip`, `str.lower`, regex `$`).
    #[derive(Debug, Default)]
    pub struct PyLinkFormatter;

    impl markdown_it::parser::linkfmt::LinkFormatter for PyLinkFormatter {
        fn validate_link(&self, url: &str) -> Option<()> {
            let url = py_strip(url).to_lowercase();
            let bad = ["vbscript:", "javascript:", "file:", "data:"]
                .iter()
                .any(|p| url.starts_with(p));
            let good_data = ["gif;", "png;", "jpeg;", "webp;"]
                .iter()
                .any(|t| url.starts_with(&format!("data:image/{t}")));
            if bad && !good_data {
                None
            } else {
                Some(())
            }
        }

        fn normalize_link(&self, url: &str) -> String {
            let mut parsed = mdurl_parse(url, true);
            if let Some(h) = parsed.hostname.as_deref().filter(|h| !h.is_empty()) {
                if recode_host(&parsed.protocol) {
                    if let Some(a) = punycode_to_ascii(h) {
                        parsed.hostname = Some(a);
                    }
                }
            }
            mdurl_encode(&mdurl_format(&parsed), ";/?:@&=+$,-_.!~*'()#", true)
        }

        fn normalize_link_text(&self, url: &str) -> String {
            let mut parsed = mdurl_parse(url, true);
            if let Some(h) = parsed.hostname.as_deref().filter(|h| !h.is_empty()) {
                if recode_host(&parsed.protocol) {
                    if let Some(u) = punycode_to_unicode(h) {
                        parsed.hostname = Some(u);
                    }
                }
            }
            mdurl_decode(&mdurl_format(&parsed), ";/?:@&=+$,#%")
        }
    }

    fn recode_host(protocol: &Option<String>) -> bool {
        match protocol.as_deref() {
            None | Some("") => true,
            Some(p) => matches!(p, "http:" | "https:" | "mailto:"),
        }
    }

    #[derive(Debug, Default)]
    struct MdUrl {
        protocol: Option<String>,
        slashes: bool,
        auth: Option<String>,
        port: Option<String>,
        hostname: Option<String>,
        hash: Option<String>,
        search: Option<String>,
        pathname: Option<String>,
    }

    /// Python `re` `$` without MULTILINE: the end, or just before a final "\n".
    fn py_dollar_body(s: &str) -> &str {
        s.strip_suffix('\n').unwrap_or(s)
    }

    fn hostname_part_ok(part: &str) -> bool {
        // ^[+a-z0-9A-Z_-]{0,63}$
        let body = py_dollar_body(part);
        body.chars().count() <= 63
            && body
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '_' | '-'))
    }

    fn find_char(s: &str, c: char) -> Option<usize> {
        s.chars().position(|x| x == c)
    }

    fn chars_slice(s: &str, from: usize, to: usize) -> String {
        s.chars().skip(from).take(to.saturating_sub(from)).collect()
    }

    /// mdurl `url_parse(url, slashes_denote_host=...)` (Python port of the JS url.parse). Indexes are in
    /// code points, as Python's.
    fn mdurl_parse(url: &str, slashes_denote_host: bool) -> MdUrl {
        let mut u = MdUrl::default();
        let mut rest: String = py_strip(url).to_string();
        let mut lower_proto = String::new();
        if !slashes_denote_host && url.split('#').count() == 1 {
            // SIMPLE_PATH_PATTERN = ^(//?(?!/)[^?\s]*)(\?[^\s]*)?$ (never reached: markdown-it passes True)
        }
        let mut proto = String::new();
        // PROTOCOL_PATTERN = ^([a-z0-9.+-]+:) (IGNORECASE)
        {
            let n = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
                .count();
            if n > 0 && rest.chars().nth(n) == Some(':') {
                proto = chars_slice(&rest, 0, n + 1);
                lower_proto = proto.to_lowercase();
                u.protocol = Some(proto.clone());
                rest = rest.chars().skip(n + 1).collect();
            }
        }
        let hostless = |p: &str| matches!(p, "javascript" | "javascript:");
        let slashed = |p: &str| {
            matches!(
                p,
                "http"
                    | "https"
                    | "ftp"
                    | "gopher"
                    | "file"
                    | "http:"
                    | "https:"
                    | "ftp:"
                    | "gopher:"
                    | "file:"
            )
        };
        let mut slashes = false;
        // slashes_denote_host is always True here
        if slashes_denote_host || !proto.is_empty() {
            slashes = rest.starts_with("//");
            if slashes && !(!proto.is_empty() && hostless(&proto)) {
                rest = rest.chars().skip(2).collect();
                u.slashes = true;
            }
        }
        if !hostless(&proto) && (slashes || (!proto.is_empty() && !slashed(&proto))) {
            let mut host_end: Option<usize> = None;
            for c in ['/', '?', '#'] {
                if let Some(h) = find_char(&rest, c) {
                    if host_end.is_none_or(|e| h < e) {
                        host_end = Some(h);
                    }
                }
            }
            let chars: Vec<char> = rest.chars().collect();
            let at_sign = match host_end {
                None => chars.iter().rposition(|&c| c == '@'),
                Some(e) => chars[..(e + 1).min(chars.len())]
                    .iter()
                    .rposition(|&c| c == '@'),
            };
            if let Some(a) = at_sign {
                u.auth = Some(chars[..a].iter().collect());
                rest = chars[a + 1..].iter().collect();
            }
            let non_host = [
                '%', '/', '?', ';', '#', '\'', '{', '}', '|', '\\', '^', '`', '<', '>', '"', '`',
                ' ', '\r', '\n', '\t',
            ];
            let mut host_end: Option<usize> = None;
            for c in non_host {
                if let Some(h) = find_char(&rest, c) {
                    if host_end.is_none_or(|e| h < e) {
                        host_end = Some(h);
                    }
                }
            }
            let chars: Vec<char> = rest.chars().collect();
            let mut host_end = host_end.unwrap_or(chars.len());
            if host_end > 0 && chars[host_end - 1] == ':' {
                host_end -= 1;
            }
            let host: String = chars[..host_end].iter().collect();
            rest = chars[host_end..].iter().collect();
            // parse_host: PORT_PATTERN = :[0-9]*$
            {
                let hc: Vec<char> = host.chars().collect();
                let body_len = if hc.last() == Some(&'\n') {
                    hc.len() - 1
                } else {
                    hc.len()
                };
                let digits = hc[..body_len]
                    .iter()
                    .rev()
                    .take_while(|c| c.is_ascii_digit())
                    .count();
                let mut host_s = host.clone();
                if body_len > digits && hc[body_len - digits - 1] == ':' {
                    // the match is ":digits" (without the final "\n", which `$` allows after it)
                    let port: String = hc[body_len - digits - 1..body_len].iter().collect();
                    if port != ":" {
                        u.port = Some(port[1..].to_string());
                    }
                    // host = host[: -len(port)]: Python cuts len(port) chars from the end (the "\n" too)
                    let keep = hc.len() - port.chars().count();
                    host_s = hc[..keep].iter().collect();
                }
                if !host_s.is_empty() {
                    u.hostname = Some(host_s);
                }
            }
            let hostname = u.hostname.clone().unwrap_or_default();
            let ipv6 = hostname.starts_with('[') && hostname.ends_with(']');
            let mut hostname = hostname;
            if !ipv6 {
                let parts: Vec<String> = hostname.split('.').map(str::to_string).collect();
                for i in 0..parts.len() {
                    let part = &parts[i];
                    if part.is_empty() {
                        continue;
                    }
                    if !hostname_part_ok(part) {
                        let newpart: String = part
                            .chars()
                            .map(|c| if (c as u32) > 127 { 'x' } else { c })
                            .collect();
                        if !hostname_part_ok(&newpart) {
                            let mut valid: Vec<String> = parts[..i].to_vec();
                            let mut not_host: Vec<String> = parts[i + 1..].to_vec();
                            // HOSTNAME_PART_START = ^([+a-z0-9A-Z_-]{0,63})(.*)$  ('.' excludes "\n")
                            let lead: String = part
                                .chars()
                                .take_while(|c| {
                                    c.is_ascii_alphanumeric() || matches!(c, '+' | '_' | '-')
                                })
                                .take(63)
                                .collect();
                            let tail: String = part.chars().skip(lead.chars().count()).collect();
                            let tail_ok = !py_dollar_body(&tail).contains('\n');
                            if tail_ok {
                                valid.push(lead);
                                not_host.insert(0, py_dollar_body(&tail).to_string());
                            }
                            if !not_host.is_empty() {
                                rest = not_host.join(".") + &rest;
                            }
                            hostname = valid.join(".");
                            break;
                        }
                    }
                }
            }
            if hostname.chars().count() > 255 {
                hostname = String::new();
            }
            if ipv6 {
                let n = hostname.chars().count();
                hostname = chars_slice(&hostname, 1, n - 1);
            }
            u.hostname = Some(hostname);
        }
        if let Some(h) = find_char(&rest, '#') {
            u.hash = Some(rest.chars().skip(h).collect());
            rest = rest.chars().take(h).collect();
        }
        if let Some(q) = find_char(&rest, '?') {
            u.search = Some(rest.chars().skip(q).collect());
            rest = rest.chars().take(q).collect();
        }
        if !rest.is_empty() {
            u.pathname = Some(rest);
        }
        if slashed(&lower_proto)
            && u.hostname.as_deref().is_some_and(|h| !h.is_empty())
            && u.pathname.as_deref().is_none_or(str::is_empty)
        {
            u.pathname = Some(String::new());
        }
        u
    }

    fn mdurl_format(u: &MdUrl) -> String {
        let mut r = String::new();
        r += u.protocol.as_deref().unwrap_or("");
        if u.slashes {
            r += "//";
        }
        if let Some(a) = u.auth.as_deref().filter(|a| !a.is_empty()) {
            r += a;
            r += "@";
        }
        match u.hostname.as_deref() {
            Some(h) if h.contains(':') => {
                r += "[";
                r += h;
                r += "]";
            }
            Some(h) => r += h,
            None => {}
        }
        if let Some(p) = u.port.as_deref().filter(|p| !p.is_empty()) {
            r += ":";
            r += p;
        }
        r += u.pathname.as_deref().unwrap_or("");
        r += u.search.as_deref().unwrap_or("");
        r += u.hash.as_deref().unwrap_or("");
        r
    }

    fn mdurl_encode(s: &str, exclude: &str, keep_escaped: bool) -> String {
        let chars: Vec<char> = s.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if keep_escaped
                && c == '%'
                && i + 2 < chars.len()
                && chars[i + 1].is_ascii_hexdigit()
                && chars[i + 2].is_ascii_hexdigit()
            {
                out.extend(&chars[i..i + 3]);
                i += 3;
                continue;
            }
            if (c as u32) < 128 {
                if c.is_ascii_alphanumeric() || exclude.contains(c) {
                    out.push(c);
                } else {
                    out.push_str(&format!("%{:02X}", c as u32));
                }
            } else {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
            i += 1;
        }
        out
    }

    fn mdurl_decode(s: &str, exclude: &str) -> String {
        // re.sub(r"(%[a-f0-9]{2})+", ..., flags=re.I)
        let b = s.as_bytes();
        let mut out = String::new();
        let mut i = 0;
        let is_esc = |j: usize| {
            j + 2 < b.len()
                && b[j] == b'%'
                && b[j + 1].is_ascii_hexdigit()
                && b[j + 2].is_ascii_hexdigit()
        };
        let mut last = 0;
        while i < b.len() {
            if i + 2 < b.len() && is_esc(i) || (i + 2 == b.len() && false) {
                // collect the run
                let start = i;
                let mut bytes = Vec::new();
                while i + 2 < b.len() && is_esc(i) {
                    bytes.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                    i += 3;
                }
                out.push_str(&s[last..start]);
                out.push_str(&decode_seq(&bytes, exclude));
                last = i;
                continue;
            }
            i += 1;
        }
        out.push_str(&s[last..]);
        out
    }

    fn decode_seq(bytes: &[u8], exclude: &str) -> String {
        let mut r = String::new();
        let l = bytes.len();
        let mut i = 0;
        while i < l {
            let b1 = bytes[i];
            if b1 < 0x80 {
                let ch = b1 as char;
                if exclude.contains(ch) {
                    r.push_str(&format!("%{:02X}", b1));
                } else {
                    r.push(ch);
                }
                i += 1;
                continue;
            }
            if b1 & 0xE0 == 0xC0 && i + 1 < l && bytes[i + 1] & 0xC0 == 0x80 {
                match std::str::from_utf8(&bytes[i..i + 2]) {
                    Ok(t) => r.push_str(t),
                    Err(_) => r.push_str("\u{FFFD}\u{FFFD}"),
                }
                i += 2;
                continue;
            }
            if b1 & 0xF0 == 0xE0
                && i + 2 < l
                && bytes[i + 1] & 0xC0 == 0x80
                && bytes[i + 2] & 0xC0 == 0x80
            {
                match std::str::from_utf8(&bytes[i..i + 3]) {
                    Ok(t) => r.push_str(t),
                    Err(_) => r.push_str("\u{FFFD}\u{FFFD}\u{FFFD}"),
                }
                i += 3;
                continue;
            }
            if b1 & 0xF8 == 0xF0
                && i + 3 < l
                && bytes[i + 1] & 0xC0 == 0x80
                && bytes[i + 2] & 0xC0 == 0x80
                && bytes[i + 3] & 0xC0 == 0x80
            {
                match std::str::from_utf8(&bytes[i..i + 4]) {
                    Ok(t) => r.push_str(t),
                    Err(_) => r.push_str("\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}"),
                }
                i += 4;
                continue;
            }
            r.push('\u{FFFD}');
            i += 1;
        }
        r
    }

    // _punycode: map_domain over "@" and the four dot separators; Python's "punycode" codec per label.

    fn map_domain(s: &str, f: impl Fn(&str) -> Option<String>) -> Option<String> {
        let parts: Vec<&str> = s.split('@').collect();
        let (mut result, s) = if parts.len() > 1 {
            (format!("{}@", parts[0]), parts[1])
        } else {
            (String::new(), s)
        };
        let labels: Vec<&str> = s.split(['.', '\u{3002}', '\u{FF0E}', '\u{FF61}']).collect();
        let mut enc = Vec::new();
        for l in labels {
            enc.push(f(l)?);
        }
        result += &enc.join(".");
        Some(result)
    }

    fn punycode_to_ascii(s: &str) -> Option<String> {
        map_domain(s, |label| {
            if label.chars().any(|c| (c as u32) > 0x7E) {
                Some(format!("xn--{}", punycode_encode(label)))
            } else {
                Some(label.to_string())
            }
        })
    }

    fn punycode_to_unicode(s: &str) -> Option<String> {
        map_domain(s, |label| {
            if let Some(rest) = label.strip_prefix("xn--") {
                punycode_decode(&rest.to_lowercase())
            } else {
                Some(label.to_string())
            }
        })
    }

    const PC_BASE: u64 = 36;
    const PC_TMIN: u64 = 1;
    const PC_TMAX: u64 = 26;

    fn pc_adapt(mut delta: u64, numpoints: u64, first: bool) -> u64 {
        delta = if first { delta / 700 } else { delta / 2 };
        delta += delta / numpoints;
        let mut k = 0;
        while delta > ((PC_BASE - PC_TMIN) * PC_TMAX) / 2 {
            delta /= PC_BASE - PC_TMIN;
            k += PC_BASE;
        }
        k + (PC_BASE * delta) / (delta + 38)
    }

    fn pc_digit(d: u64) -> char {
        // Python: "abcdefghijklmnopqrstuvwxyz0123456789"
        if d < 26 {
            (b'a' + d as u8) as char
        } else {
            (b'0' + (d - 26) as u8) as char
        }
    }

    /// Python's punycode codec encode (RFC 3492; basic code points < 0x80 kept as they are, "-" only when
    /// there are some).
    fn punycode_encode(text: &str) -> String {
        let chars: Vec<u32> = text.chars().map(|c| c as u32).collect();
        let mut out: String = text.chars().filter(|c| (*c as u32) < 0x80).collect();
        let b = out.chars().count() as u64;
        if b > 0 {
            out.push('-');
        }
        let mut n: u64 = 0x80;
        let mut delta: u64 = 0;
        let mut bias: u64 = 72;
        let mut h = b;
        let total = chars.len() as u64;
        let mut first = true;
        while h < total {
            let m = chars
                .iter()
                .filter(|&&c| c as u64 >= n)
                .min()
                .copied()
                .unwrap() as u64;
            delta += (m - n) * (h + 1);
            n = m;
            for &c in &chars {
                let c = c as u64;
                if c < n {
                    delta += 1;
                }
                if c == n {
                    let mut q = delta;
                    let mut k = PC_BASE;
                    loop {
                        let t = if k <= bias {
                            PC_TMIN
                        } else if k >= bias + PC_TMAX {
                            PC_TMAX
                        } else {
                            k - bias
                        };
                        if q < t {
                            break;
                        }
                        out.push(pc_digit(t + (q - t) % (PC_BASE - t)));
                        q = (q - t) / (PC_BASE - t);
                        k += PC_BASE;
                    }
                    out.push(pc_digit(q));
                    bias = pc_adapt(delta, h + 1, first);
                    first = false;
                    delta = 0;
                    h += 1;
                }
            }
            delta += 1;
            n += 1;
        }
        out
    }

    /// Python's punycode codec decode (strict): None where Python raises.
    fn punycode_decode(text: &str) -> Option<String> {
        if !text.is_ascii() {
            return None;
        }
        let (base, ext) = match text.rfind('-') {
            Some(p) => (&text[..p], text[p + 1..].to_uppercase()),
            None => ("", text.to_uppercase()),
        };
        let mut out: Vec<u32> = base.chars().map(|c| c as u32).collect();
        let ext: Vec<u8> = ext.bytes().collect();
        let mut n: u64 = 0x80;
        let mut i: u64 = 0;
        let mut bias: u64 = 72;
        let mut pos = 0usize;
        let mut first = true;
        while pos < ext.len() {
            let old_i = i;
            let mut w: u64 = 1;
            let mut k = PC_BASE;
            loop {
                let c = *ext.get(pos)?;
                pos += 1;
                let digit = match c {
                    b'A'..=b'Z' => (c - b'A') as u64,
                    b'0'..=b'9' => (c - b'0') as u64 + 26,
                    _ => return None,
                };
                i = i.checked_add(digit.checked_mul(w)?)?;
                let t = if k <= bias {
                    PC_TMIN
                } else if k >= bias + PC_TMAX {
                    PC_TMAX
                } else {
                    k - bias
                };
                if digit < t {
                    break;
                }
                w = w.checked_mul(PC_BASE - t)?;
                k += PC_BASE;
            }
            let len = out.len() as u64 + 1;
            bias = pc_adapt(i - old_i, len, first);
            first = false;
            n = n.checked_add(i / len)?;
            i %= len;
            if n > 0x10FFFF {
                return None;
            }
            out.insert(i as usize, n as u32);
            i += 1;
        }
        out.into_iter().map(char::from_u32).collect()
    }

    // ---------------------------------------------------------------------------------------- references

    /// markdown-it-py `normalizeReference`: strip, collapse `\s+` (Python whitespace) to " ", lower().upper().
    fn py_normalize_reference(label: &str) -> String {
        let mut out = String::new();
        let mut in_ws = false;
        for c in py_strip(label).chars() {
            if py_is_space(c) {
                if !in_ws {
                    out.push(' ');
                }
                in_ws = true;
            } else {
                out.push(c);
                in_ws = false;
            }
        }
        out.to_lowercase().to_uppercase()
    }

    /// Reference definitions keyed by markdown-it-py's normalisation; the first definition of a label wins.
    #[derive(Debug, Default)]
    struct PyRefMap(std::collections::HashMap<String, (String, Option<String>)>);

    impl markdown_it::plugins::cmark::block::reference::CustomReferenceMap for PyRefMap {
        fn insert(&mut self, label: String, destination: String, title: Option<String>) -> bool {
            let key = py_normalize_reference(&label);
            if key.is_empty() || self.0.contains_key(&key) {
                return false;
            }
            self.0.insert(key, (destination, title));
            true
        }

        fn get(&self, label: &str) -> Option<(&str, Option<&str>)> {
            let key = py_normalize_reference(label);
            self.0.get(&key).map(|(d, t)| (d.as_str(), t.as_deref()))
        }
    }

    struct PyInstallRefMap;
    impl markdown_it::parser::core::CoreRule for PyInstallRefMap {
        fn run(root: &mut Node, _: &MarkdownIt) {
            use markdown_it::plugins::cmark::block::reference::ReferenceMap;
            if let Some(data) = root.cast_mut::<markdown_it::parser::core::Root>() {
                data.ext.insert(ReferenceMap::new(PyRefMap::default()));
            }
        }
    }

    struct LinkRes {
        pos: usize,
        lines: usize,
        s: String,
    }

    /// markdown-it-py `parseLinkDestination` (code point indexes).
    fn py_parse_link_destination(s: &[char], mut pos: usize, max: usize) -> Option<LinkRes> {
        let start = pos;
        if s.get(pos) == Some(&'<') {
            pos += 1;
            while pos < max {
                let c = s[pos];
                if c == '\n' || c == '<' {
                    return None;
                }
                if c == '>' {
                    let inner: String = s[start + 1..pos].iter().collect();
                    return Some(LinkRes {
                        pos: pos + 1,
                        lines: 0,
                        s: unescape_all(&inner).into_owned(),
                    });
                }
                if c == '\\' && pos + 1 < max {
                    pos += 2;
                    continue;
                }
                pos += 1;
            }
            return None;
        }
        let mut level = 0;
        while pos < max {
            let c = s[pos];
            if c == ' ' || (c as u32) < 0x20 || c == '\x7f' {
                break;
            }
            if c == '\\' && pos + 1 < max {
                if s[pos + 1] == ' ' {
                    break;
                }
                pos += 2;
                continue;
            }
            if c == '(' {
                level += 1;
                if level > 32 {
                    return None;
                }
            }
            if c == ')' {
                if level == 0 {
                    break;
                }
                level -= 1;
            }
            pos += 1;
        }
        if start == pos || level != 0 {
            return None;
        }
        let raw: String = s[start..pos].iter().collect();
        Some(LinkRes {
            pos,
            lines: 0,
            s: unescape_all(&raw).into_owned(),
        })
    }

    /// markdown-it-py `parseLinkTitle`.
    fn py_parse_link_title(s: &[char], mut pos: usize, max: usize) -> Option<LinkRes> {
        let start = pos;
        if pos >= max {
            return None;
        }
        let mut marker = s[pos];
        if !matches!(marker, '"' | '\'' | '(') {
            return None;
        }
        pos += 1;
        if marker == '(' {
            marker = ')';
        }
        let mut lines = 0;
        while pos < max {
            let c = s[pos];
            if c == marker {
                let raw: String = s[start + 1..pos].iter().collect();
                return Some(LinkRes {
                    pos: pos + 1,
                    lines,
                    s: unescape_all(&raw).into_owned(),
                });
            } else if c == '(' && marker == ')' {
                return None;
            } else if c == '\n' {
                lines += 1;
            } else if c == '\\' && pos + 1 < max {
                pos += 1;
                if s[pos] == '\n' {
                    lines += 1;
                }
            }
            pos += 1;
        }
        None
    }

    /// The terminators markdown-it-py's reference rule uses (`getRules("reference")`: table, fence, blockquote,
    /// hr, list, html_block (off), heading), with `parentType` "reference".
    fn reference_terminators(state: &mut BlockState) -> bool {
        PyTableScanner::check(state).is_some() || blockquote_terminators(state)
    }

    /// markdown-it-py's reference rule, ported as written.
    pub struct PyReferenceScanner;

    impl BlockRule for PyReferenceScanner {
        fn check(_: &mut BlockState) -> Option<()> {
            None // a reference never interrupts anything
        }

        fn run(state: &mut BlockState) -> Option<(Node, usize)> {
            use markdown_it::plugins::cmark::block::reference::{Definition, ReferenceMap};
            let start_line = state.line;
            if state.line_indent(start_line) >= 4 {
                return None;
            }
            let first: Vec<char> = state.get_line(start_line).chars().collect();
            if first.first() != Some(&'[') {
                return None;
            }
            let mut p = 0;
            while p < first.len() {
                if first[p] == ']' && p > 0 && first[p - 1] != '\\' {
                    if p + 1 == first.len() || first[p + 1] != ':' {
                        return None;
                    }
                    break;
                }
                p += 1;
            }
            let end_line = state.line_max;
            let mut next_line = start_line + 1;
            while next_line < end_line && !state.is_empty(next_line) {
                if state.line_indent(next_line) > 3 {
                    next_line += 1;
                    continue;
                }
                if state.line_offsets[next_line].indent_nonspace < 0 {
                    next_line += 1;
                    continue;
                }
                let old = state.line;
                state.line = next_line;
                let terminate = reference_terminators(state);
                state.line = old;
                if terminate {
                    break;
                }
                next_line += 1;
            }
            let (raw, _) = state.get_lines(start_line, next_line, state.blk_indent, false);
            let s: Vec<char> = py_strip(&raw).chars().collect();
            let max = s.len();
            let mut lines = 0usize;
            let mut label_end = None;
            let mut pos = 1;
            while pos < max {
                match s[pos] {
                    '[' => return None,
                    ']' => {
                        label_end = Some(pos);
                        break;
                    }
                    '\n' => lines += 1,
                    '\\' => {
                        pos += 1;
                        if pos < max && s[pos] == '\n' {
                            lines += 1;
                        }
                    }
                    _ => {}
                }
                pos += 1;
            }
            let label_end = label_end?;
            if s.get(label_end + 1) != Some(&':') {
                return None;
            }
            pos = label_end + 2;
            while pos < max {
                match s[pos] {
                    '\n' => lines += 1,
                    ' ' | '\t' => {}
                    _ => break,
                }
                pos += 1;
            }
            let res = py_parse_link_destination(&s, pos, max)?;
            let href = state.md.link_formatter.normalize_link(&res.s);
            state.md.link_formatter.validate_link(&href)?;
            pos = res.pos;
            lines += res.lines;
            let dest_end_pos = pos;
            let dest_end_lines = lines;
            let start = pos;
            while pos < max {
                match s[pos] {
                    '\n' => lines += 1,
                    ' ' | '\t' => {}
                    _ => break,
                }
                pos += 1;
            }
            let title_res = py_parse_link_title(&s, pos, max);
            let mut title = String::new();
            match title_res {
                Some(t) if pos < max && start != pos => {
                    title = t.s;
                    pos = t.pos;
                    lines += t.lines;
                }
                _ => {
                    pos = dest_end_pos;
                    lines = dest_end_lines;
                }
            }
            while pos < max && matches!(s[pos], ' ' | '\t') {
                pos += 1;
            }
            if pos < max && s[pos] != '\n' && !title.is_empty() {
                title = String::new();
                pos = dest_end_pos;
                lines = dest_end_lines;
                while pos < max && matches!(s[pos], ' ' | '\t') {
                    pos += 1;
                }
            }
            if pos < max && s[pos] != '\n' {
                return None;
            }
            let raw_label: String = s[1..label_end].iter().collect();
            if py_normalize_reference(&raw_label).is_empty() {
                return None;
            }
            let refs = state
                .root_ext
                .get_or_insert_with::<ReferenceMap>(|| ReferenceMap::new(PyRefMap::default()));
            let t = if title.is_empty() {
                None
            } else {
                Some(title.clone())
            };
            refs.insert(raw_label.clone(), href.clone(), t.clone());
            Some((
                Node::new(Definition {
                    label: raw_label,
                    destination: href,
                    title: t,
                }),
                lines + 1,
            ))
        }
    }

    // ---------------------------------------------------------------------------------------- paragraph

    /// The crate's paragraph rule, except for the text of a lazy blockquote line: markdown-it-py's getLines
    /// keeps its leading whitespace beyond the block indent (the crate drops all of it), which shows inside
    /// code spans.
    pub struct PyParagraphScanner;

    impl BlockRule for PyParagraphScanner {
        fn check(_: &mut BlockState) -> Option<()> {
            None
        }

        fn run(state: &mut BlockState) -> Option<(Node, usize)> {
            use markdown_it::plugins::cmark::block::paragraph::Paragraph;
            let start_line = state.line;
            let mut next_line = start_line;
            loop {
                next_line += 1;
                if next_line >= state.line_max || state.is_empty(next_line) {
                    break;
                }
                if state.line_indent(next_line) >= state.md.max_indent {
                    continue;
                }
                if state.line_offsets[next_line].indent_nonspace < 0 {
                    continue;
                }
                let old = state.line;
                state.line = next_line;
                let stop = state.test_rules_at_line();
                state.line = old;
                if stop {
                    break;
                }
            }
            let mut content = String::new();
            let mut mapping = Vec::new();
            for line in start_line..next_line {
                let lf = line + 1 < next_line;
                if state.line_offsets[line].indent_nonspace < 0 {
                    let off = state.line_offsets[line].clone();
                    // whitespace columns before the text, in the container that marked the line lazy
                    let orig = state
                        .root_ext
                        .get::<LazyIndents>()
                        .and_then(|m| m.0.get(&off.line_start))
                        .copied()
                        .unwrap_or(0);
                    let mut start = off.first_nonspace;
                    let mut cols = 0i32;
                    while start > off.line_start && cols < orig {
                        match state.src.as_bytes()[start - 1] {
                            b' ' => cols += 1,
                            b'\t' => cols += 4,
                            _ => break,
                        }
                        start -= 1;
                    }
                    let text = &state.src[start..off.line_end];
                    let off = markdown_it::parser::block::LineOffset {
                        line_start: start,
                        ..off
                    };
                    let mut col = 0usize;
                    let mut skip = 0usize;
                    for ch in text.chars() {
                        if col >= state.blk_indent {
                            break;
                        }
                        match ch {
                            ' ' => col += 1,
                            '\t' => col += 4 - col % 4,
                            _ => break,
                        }
                        skip += 1;
                    }
                    mapping.push((content.len(), off.line_start + skip));
                    content.push_str(&text[skip..]);
                    if lf {
                        content.push('\n');
                    }
                } else {
                    let (part, map) = state.get_lines(line, line + 1, state.blk_indent, lf);
                    for (d, s) in map {
                        mapping.push((content.len() + d, s));
                    }
                    content.push_str(&part);
                }
            }
            let mut node = Node::new(Paragraph);
            node.children
                .push(Node::new(InlineRoot::new(content, mapping)));
            Some((node, next_line - start_line))
        }
    }

    #[derive(Debug, Default)]
    struct LazyIndents(std::collections::HashMap<usize, i32>);
    impl markdown_it::parser::extset::RootExt for LazyIndents {}
}

// 23 Oct 2026: the draft PDF of a document as it stands (no QR, nothing issued), shared by the proposal preview
// (routers/sales_documents.rs) and Nary's render_document_page.

/// What the PDF service is asked to draw for a draft: built from the database first, rendered after (the
/// preview route lets its connection go in between).
pub struct DraftPdf {
    pub template: String,
    pub data: Value,
    pub footer: Value,
    pub header: Option<Value>,
    /// The document's number (full number when it has one), also the file name.
    pub code: String,
}

/// The draft of a proposal, a quotation or any document with a verification builder: the same data as the
/// verified copy, the footer "Draft, not for distribution" and no QR.
pub async fn draft_job(
    db: &mut PgConnection,
    entity_type: &str,
    entity: &crate::approvals::adapters::Entity,
) -> ApiResult<DraftPdf> {
    use crate::approvals::adapters::{get_adapter, Entity};
    let adapter = get_adapter(entity_type)?;
    let (template, mut data, _) =
        crate::verification::render::build(db, entity_type, entity, None).await?;
    let header = match data.get("pdf_header") {
        Some(h) if truthy(h) => {
            let h = data
                .as_object_mut()
                .and_then(|m| m.shift_remove("pdf_header"))
                .unwrap_or(Value::Null);
            Some(json!({ "text": h }))
        }
        _ => None,
    };
    let code = adapter.number(entity);
    let title = match entity {
        Entity::SalesDocument(d) => d.title.clone(),
        other => format!("{} {}", adapter.label, adapter.number(other)),
    };
    let footer = json!({"qr_data_uri": "", "title": title, "line": "Draft, not for distribution",
                        "code": code});
    Ok(DraftPdf {
        template,
        data,
        footer,
        header,
        code,
    })
}

impl DraftPdf {
    /// The PDF bytes from the PDF service.
    pub async fn render(&self) -> ApiResult<Vec<u8>> {
        crate::pdf::generate_pdf(
            &self.template,
            &self.data,
            Some(&self.footer),
            self.header.as_ref(),
        )
        .await
        .map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))
    }
}

/// The draft PDF of a Solution Proposal and its number (the preview).
pub async fn draft_pdf(db: &mut PgConnection, doc: &SalesDocument) -> ApiResult<(Vec<u8>, String)> {
    let entity = crate::approvals::adapters::Entity::SalesDocument(Box::new(doc.clone()));
    let job = draft_job(db, "sales_document", &entity).await?;
    Ok((job.render().await?, job.code))
}
