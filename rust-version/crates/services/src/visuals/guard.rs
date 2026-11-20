//! The rules a figure must pass before it is drawn for a customer. Used by the proposal save path, the
//! drafting job and the chat / MCP tools, so every surface refuses the same things:
//!
//! - schema and ranges: series match the categories, finite numbers, non-negative where it makes sense,
//!   gantt tasks inside the timeline, diagram ids unique, edges between existing nodes, no cycles in a
//!   `process`, known icons, sizes a reader can still read;
//! - no money: a unit, title or label with a currency code or symbol (USD, EUR, JPY, $, ...) is refused;
//!   prices live only in the generated commercial section;
//! - no accuracy, precision or success-rate figures (the proposals.rs `strip_accuracy` rule);
//! - catalog products: a node with `sku` must be a catalog product and its label the product's name or
//!   short name; a label that names a catalog product without a `sku` gets it filled in;
//! - sources: every chart says where its numbers come from; `customer` and `analysis` need a note; `erp`
//!   refs are resolved here and their numbers replace whatever the model typed;
//! - `custom_svg`: sanitised to the allow-list, size capped, and all of its text run through the same
//!   text rules.
//!
//! `guard` is pure (no database): [`load_context`] reads what it needs once.

use std::collections::{BTreeMap, HashMap, HashSet};

use granary_core::errors::ApiResult;
use granary_core::settings_store::require_json;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use super::icons;
use super::sanitize;
use super::spec::*;

/// A catalog product as the guard sees it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CatalogProduct {
    pub sku: String,
    pub name: String,
    pub short_name: Option<String>,
    pub family: Option<String>,
}

/// Numbers an `erp` ref resolves to. The guard copies the non-empty fields into the spec.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ErpData {
    pub categories: Vec<String>,
    pub series: Vec<Series>,
    pub tasks: Vec<GanttTask>,
    pub milestones: Vec<Milestone>,
    pub time_unit: Option<TimeUnit>,
    pub unit: Option<String>,
}

/// What the guard checks against.
#[derive(Clone, Debug, PartialEq)]
pub struct GuardContext {
    pub catalog: Vec<CatalogProduct>,
    /// Resolved `erp` refs by ref text (`proposal.phases`, `requirements.<key>`). [`load_context`] fills
    /// requirements refs; the proposal code adds `proposal.phases` with [`GuardContext::with_erp`].
    pub erp: BTreeMap<String, ErpData>,
    /// `proposal.visuals.custom_svg_max_kb`.
    pub custom_svg_max_kb: usize,
}

impl Default for GuardContext {
    fn default() -> Self {
        GuardContext {
            catalog: vec![],
            erp: BTreeMap::new(),
            custom_svg_max_kb: sanitize::DEFAULT_MAX_KB,
        }
    }
}

impl GuardContext {
    pub fn with_erp(mut self, reference: &str, data: ErpData) -> Self {
        self.erp.insert(reference.to_string(), data);
        self
    }
}

/// Errors refuse the figure; warnings are kept with it (and shown on cards and in the editor).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GuardReport {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl GuardReport {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
    pub(crate) fn err(&mut self, s: impl Into<String>) {
        let s = s.into();
        if !self.errors.contains(&s) {
            self.errors.push(s);
        }
    }
    pub(crate) fn warn(&mut self, s: impl Into<String>) {
        let s = s.into();
        if !self.warnings.contains(&s) {
            self.warnings.push(s);
        }
    }
    /// All messages, errors first (for a tool result or a card).
    pub fn messages(&self) -> Vec<String> {
        self.errors
            .iter()
            .chain(self.warnings.iter())
            .cloned()
            .collect()
    }
}

// Readability limits: beyond these a figure cannot be read at A4 width.
pub const MAX_CATEGORIES: usize = 40;
pub const MAX_SERIES: usize = 10;
pub const MAX_TASKS: usize = 30;
pub const MAX_MILESTONES: usize = 12;
pub const MAX_POINTS: usize = 400;
pub const MAX_NODES: usize = 40;
pub const MAX_EDGES: usize = 80;
pub const MAX_GROUPS: usize = 10;
pub const MAX_TIMELINE_UNITS: f64 = 260.0;
pub const MAX_TEXT: usize = 200;

static MONEY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(\b(USD|EUR|GBP|JPY|CNY|AUD|CAD|CHF|SGD|INR|dollars?|euros?)\b|[$€£¥₹])")
        .unwrap()
});

static ACCURACY_WORD: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(accura\w*|precision|success rate|error rate)\b").unwrap());

/// `guard(spec, ctx)`: check (and fix what can be fixed in place: ERP numbers, missing skus, em dashes,
/// a sanitised custom SVG) one spec.
pub fn guard(spec: &mut Spec, ctx: &GuardContext) -> GuardReport {
    let mut r = GuardReport::default();
    match spec {
        Spec::Chart(c) => guard_chart(c, ctx, &mut r),
        Spec::Diagram(d) => guard_diagram(d, ctx, &mut r),
        Spec::CustomSvg(s) => guard_custom(s, ctx, &mut r),
    }
    r
}

// ---------------------------------------------------------------------------------------------------------
// Text rules
// ---------------------------------------------------------------------------------------------------------

pub(crate) fn clean(s: &mut String) {
    if s.contains('\u{2014}') {
        *s = s.replace(" \u{2014} ", ", ").replace('\u{2014}', ", ");
    }
}

pub(crate) fn clean_opt(s: &mut Option<String>) {
    if let Some(s) = s {
        clean(s);
    }
}

/// Money, accuracy and length checks on one piece of figure text. `numeric` = the figure carries numbers
/// (a chart), so an accuracy word alone is enough to refuse.
pub(crate) fn text_rules(what: &str, s: &str, numeric: bool, r: &mut GuardReport) {
    if s.is_empty() {
        return;
    }
    if MONEY.is_match(s) {
        r.err(format!(
            "{what} '{}' shows money; prices only appear in the commercial section",
            short(s)
        ));
    }
    let (_, removed) = crate::nary::proposals::strip_accuracy(s);
    if !removed.is_empty() || (numeric && ACCURACY_WORD.is_match(s)) {
        r.err(format!(
            "{what} '{}' states an accuracy, precision or success-rate figure; not allowed in a proposal",
            short(s)
        ));
    }
    if s.chars().count() > MAX_TEXT {
        r.err(format!(
            "{what} is too long ({} characters, at most {MAX_TEXT})",
            s.chars().count()
        ));
    }
}

/// 23 Oct (propose_proposal_from_sections): the caption guards on a proposal's own prose (a text block, a table
/// cell, a heading) written by the chat or the MCP client: no money, no accuracy figure, no product name the
/// catalog does not have. No length limit (a paragraph is long). The text is checked with its em dashes removed.
pub fn prose_problems(what: &str, text: &str, ctx: &GuardContext) -> Vec<String> {
    prose_problems_with(what, text, ctx, false)
}

/// 1 Nov 2026 (direct proposal authoring): whether a text states money (a currency code, $, ...).
pub fn shows_money(text: &str) -> bool {
    MONEY.is_match(text)
}

/// [`prose_problems`]; `allow_money`: the text is in a price section (proposal.price_text_sections) and the
/// writer may state prices there (the Owner or sales:apply_discount), so money passes; the other rules stay.
pub fn prose_problems_with(
    what: &str,
    text: &str,
    _ctx: &GuardContext,
    allow_money: bool,
) -> Vec<String> {
    let mut r = GuardReport::default();
    let mut s = text.trim().to_string();
    clean(&mut s);
    if s.is_empty() {
        return vec![];
    }
    if !allow_money && MONEY.is_match(&s) {
        r.err(format!(
            "{what} '{}' shows money; prices only appear in the commercial section (from the quotation)",
            short(&s)
        ));
    }
    let (_, removed) = crate::nary::proposals::strip_accuracy(&s);
    for sentence in removed {
        r.err(format!(
            "{what} '{}' states an accuracy, precision or success-rate figure; not allowed in a proposal",
            short(&sentence)
        ));
    }
    r.errors
}

pub(crate) fn short(s: &str) -> String {
    let t: String = s.chars().take(60).collect();
    if t.len() < s.len() {
        format!("{t}...")
    } else {
        t
    }
}

// ---------------------------------------------------------------------------------------------------------
// Charts
// ---------------------------------------------------------------------------------------------------------

fn finite(v: f64) -> bool {
    v.is_finite()
}

fn guard_chart(c: &mut ChartSpec, ctx: &GuardContext, r: &mut GuardReport) {
    // Source first: an ERP ref replaces the numbers before they are checked.
    match &c.source {
        None => r.err("a chart needs a source: an ERP reference, the customer's numbers with a note, or an analysis note"),
        Some(Source::Customer { note }) if note.trim().is_empty() => {
            r.err("source 'customer' needs a note saying where the numbers come from")
        }
        Some(Source::Analysis { note, from }) => {
            if note.trim().is_empty() {
                r.err("source 'analysis' needs a note saying how the numbers were worked out");
            }
            if from.is_empty() {
                r.warn("source 'analysis' lists no lookups in 'from'");
            }
        }
        Some(Source::Erp { reference }) => {
            let reference = reference.trim().to_string();
            match ctx.erp.get(&reference) {
                Some(data) => apply_erp(c, data),
                None => r.err(format!(
                    "the ERP reference '{reference}' could not be resolved (known here: {})",
                    if ctx.erp.is_empty() { "none".to_string() } else { ctx.erp.keys().cloned().collect::<Vec<_>>().join(", ") }
                )),
            }
        }
        _ => {}
    }
    if let Some(Source::Customer { note }) | Some(Source::Analysis { note, .. }) = &mut c.source {
        clean(note);
        let n = note.clone();
        text_rules("source note", &n, false, r);
    }

    clean(&mut c.title);
    clean_opt(&mut c.unit);
    clean_opt(&mut c.x_label);
    clean_opt(&mut c.y_label);
    text_rules("title", &c.title.clone(), true, r);
    for (what, t) in [
        ("unit", c.unit.clone()),
        ("x label", c.x_label.clone()),
        ("y label", c.y_label.clone()),
    ] {
        if let Some(t) = t {
            text_rules(what, &t, true, r);
        }
    }
    for cat in c.categories.iter_mut() {
        clean(cat);
    }
    for cat in c.categories.clone() {
        text_rules("category", &cat, true, r);
    }
    for s in c.series.iter_mut() {
        clean(&mut s.name);
        for p in s.points.iter_mut() {
            clean_opt(&mut p.label);
        }
    }
    for s in c.series.clone() {
        text_rules("series", &s.name, true, r);
        for p in &s.points {
            if let Some(l) = &p.label {
                text_rules("point label", l, true, r);
            }
        }
    }
    for t in c.tasks.iter_mut() {
        clean(&mut t.name);
        clean_opt(&mut t.group);
    }
    for m in c.milestones.iter_mut() {
        clean(&mut m.name);
    }
    for t in c.tasks.clone() {
        text_rules("task", &t.name, false, r);
    }
    for m in c.milestones.clone() {
        text_rules("milestone", &m.name, false, r);
    }
    for a in c.annotations.iter_mut() {
        match a {
            Annotation::RefLine { label, .. } | Annotation::Highlight { label, .. } => clean(label),
            Annotation::Note { text, .. } => clean(text),
        }
    }
    for a in c.annotations.clone() {
        match a {
            Annotation::RefLine { label, .. } | Annotation::Highlight { label, .. } => {
                text_rules("annotation", &label, true, r)
            }
            Annotation::Note { text, .. } => text_rules("note", &text, true, r),
        }
    }

    // Colours: unknown ones fall back to the palette.
    let bad: Vec<String> = c
        .colors
        .iter()
        .filter(|x| super::theme::color(x).is_none())
        .cloned()
        .collect();
    if !bad.is_empty() {
        r.warn(format!(
            "colours {} are not theme colours or #rrggbb; the palette is used",
            bad.join(", ")
        ));
    }

    let kind = c.kind;
    use ChartKind as K;
    let category_kinds = matches!(
        kind,
        K::Bar
            | K::StackedBar
            | K::Line
            | K::Area
            | K::Donut
            | K::Hbar
            | K::Combo
            | K::Heatmap
            | K::Waterfall
            | K::Radar
    );
    if c.series.len() > MAX_SERIES {
        r.err(format!(
            "{} series is too many to read (at most {MAX_SERIES})",
            c.series.len()
        ));
    }
    if category_kinds {
        if c.categories.is_empty() {
            r.err("the chart has no categories");
        }
        if c.categories.len() > MAX_CATEGORIES {
            r.err(format!(
                "{} categories is too many to read (at most {MAX_CATEGORIES})",
                c.categories.len()
            ));
        }
        if c.series.is_empty() {
            r.err("the chart has no series");
        }
        for s in &c.series {
            if s.values.len() != c.categories.len() {
                r.err(format!(
                    "series '{}' has {} values for {} categories",
                    short(&s.name),
                    s.values.len(),
                    c.categories.len()
                ));
            }
            if s.values.iter().any(|v| !finite(*v)) {
                r.err(format!(
                    "series '{}' has a value that is not a number",
                    short(&s.name)
                ));
            }
        }
        let mut seen = HashSet::new();
        for cat in &c.categories {
            if !seen.insert(cat.trim().to_lowercase()) {
                r.warn(format!("category '{}' appears twice", short(cat)));
            }
        }
    }
    let nonneg = matches!(kind, K::StackedBar | K::Donut | K::Heatmap | K::Radar)
        || c.percent
        || (c.stacked && matches!(kind, K::Bar | K::Hbar | K::Area));
    if nonneg && c.series.iter().any(|s| s.values.iter().any(|v| *v < 0.0)) {
        r.err(format!(
            "a {} chart cannot show negative values",
            kind.as_str()
        ));
    }
    match kind {
        K::Donut => {
            if c.series.len() > 1 {
                r.warn("a donut shows the first series only");
            }
            if c.series
                .first()
                .is_none_or(|s| s.values.iter().sum::<f64>() <= 0.0)
            {
                r.err("a donut needs values above zero");
            }
            if c.categories.len() > 12 {
                r.err("a donut with more than 12 slices cannot be read; use a bar chart");
            }
        }
        K::Waterfall => {
            if c.series.len() != 1 {
                r.err("a waterfall takes exactly one series of changes");
            }
        }
        K::Radar => {
            if c.categories.len() < 3 {
                r.err("a radar chart needs at least 3 categories");
            }
            if c.categories.len() > 12 {
                r.err("a radar chart with more than 12 axes cannot be read");
            }
        }
        K::Combo => {
            if !c
                .series
                .iter()
                .any(|s| s.series_type == Some(SeriesType::Line))
            {
                r.warn("a combo chart with no line series draws as a bar chart");
            }
        }
        K::Heatmap => {
            if c.categories.len() > 24 || c.series.len() > MAX_SERIES {
                r.err("a heatmap is at most 24 columns by 10 rows");
            }
        }
        K::Scatter => {
            if c.series.iter().all(|s| s.points.is_empty()) {
                r.err("a scatter chart needs points");
            }
            let n: usize = c.series.iter().map(|s| s.points.len()).sum();
            if n > MAX_POINTS {
                r.err(format!("{n} points is too many (at most {MAX_POINTS})"));
            }
            if c.series
                .iter()
                .flat_map(|s| s.points.iter())
                .any(|p| !finite(p.x) || !finite(p.y))
            {
                r.err("a point is not a number");
            }
        }
        K::Gantt => guard_gantt(c, r),
        K::Milestones => {
            if c.milestones.is_empty() {
                r.err("a milestone strip needs milestones");
            }
            if c.milestones.len() > MAX_MILESTONES {
                r.err(format!("at most {MAX_MILESTONES} milestones"));
            }
            for m in &c.milestones {
                if !finite(m.at) || m.at < 0.0 || m.at > MAX_TIMELINE_UNITS {
                    r.err(format!(
                        "milestone '{}' is outside the timeline",
                        short(&m.name)
                    ));
                }
            }
            if c.time_unit.is_none() {
                c.time_unit = Some(TimeUnit::Week);
                r.warn("no time unit given; weeks are used");
            }
        }
        _ => {}
    }
    // Annotations point at things that exist.
    for a in &c.annotations {
        match a {
            Annotation::Highlight { series, index, .. } => {
                match c.series.iter().find(|s| &s.name == series) {
                    None => r.err(format!("highlight: no series '{}'", short(series))),
                    Some(s) if *index >= s.values.len().max(s.points.len()) => r.err(format!(
                        "highlight: series '{}' has no value {index}",
                        short(series)
                    )),
                    _ => {}
                }
            }
            Annotation::RefLine { value, axis, .. } => {
                if !finite(*value) {
                    r.err("a reference line value is not a number");
                }
                if axis != "x" && axis != "y" {
                    r.err("a reference line axis is x or y");
                }
            }
            Annotation::Note { at, .. } => {
                if !finite(at[0]) || !finite(at[1]) {
                    r.err("a note position is not a number");
                }
            }
        }
    }
}

fn guard_gantt(c: &mut ChartSpec, r: &mut GuardReport) {
    if c.tasks.is_empty() {
        r.err("a timeline needs tasks");
        return;
    }
    if c.tasks.len() > MAX_TASKS {
        r.err(format!(
            "{} tasks is too many to read (at most {MAX_TASKS})",
            c.tasks.len()
        ));
    }
    if c.milestones.len() > MAX_MILESTONES {
        r.err(format!("at most {MAX_MILESTONES} milestones"));
    }
    if c.time_unit.is_none() {
        c.time_unit = Some(TimeUnit::Week);
        r.warn("no time unit given; weeks are used");
    }
    let mut end: f64 = 0.0;
    for t in &c.tasks {
        if !finite(t.start) || !finite(t.length) || t.start < 0.0 {
            r.err(format!(
                "task '{}' starts before the timeline",
                short(&t.name)
            ));
        } else if t.length <= 0.0 {
            r.err(format!(
                "task '{}' needs a length above zero",
                short(&t.name)
            ));
        } else {
            end = end.max(t.start + t.length);
        }
        if t.name.trim().is_empty() {
            r.err("a task has no name");
        }
    }
    if end > MAX_TIMELINE_UNITS {
        r.err(format!(
            "the timeline is longer than {MAX_TIMELINE_UNITS} units; use a larger time unit"
        ));
    }
    for m in &c.milestones {
        if !finite(m.at) || m.at < 0.0 || m.at > end + 1e-9 {
            r.err(format!(
                "milestone '{}' is outside the timeline (0 to {})",
                short(&m.name),
                super::svg::n(end)
            ));
        }
    }
}

fn apply_erp(c: &mut ChartSpec, d: &ErpData) {
    if !d.categories.is_empty() || !d.series.is_empty() {
        c.categories = d.categories.clone();
        c.series = d.series.clone();
    }
    if !d.tasks.is_empty() {
        c.tasks = d.tasks.clone();
        c.milestones = d.milestones.clone();
    } else if !d.milestones.is_empty() {
        c.milestones = d.milestones.clone();
    }
    if d.time_unit.is_some() {
        c.time_unit = d.time_unit;
    }
    if d.unit.is_some() {
        c.unit = d.unit.clone();
    }
}

// ---------------------------------------------------------------------------------------------------------
// Diagrams
// ---------------------------------------------------------------------------------------------------------

pub(crate) fn norm(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub(crate) fn product_by_sku<'a>(ctx: &'a GuardContext, sku: &str) -> Option<&'a CatalogProduct> {
    ctx.catalog
        .iter()
        .find(|p| p.sku.eq_ignore_ascii_case(sku.trim()))
}

pub(crate) fn product_by_label<'a>(
    ctx: &'a GuardContext,
    label: &str,
) -> Option<&'a CatalogProduct> {
    let l = norm(label);
    if l.is_empty() {
        return None;
    }
    ctx.catalog.iter().find(|p| {
        norm(&p.name) == l || p.short_name.as_deref().map(norm).as_deref() == Some(l.as_str())
    })
}

fn guard_diagram(d: &mut DiagramSpec, ctx: &GuardContext, r: &mut GuardReport) {
    clean(&mut d.title);
    text_rules("title", &d.title.clone(), false, r);
    if d.nodes.is_empty() {
        r.err("the diagram has no nodes");
    }
    if d.nodes.len() > MAX_NODES {
        r.err(format!(
            "{} nodes is too many to read (at most {MAX_NODES}); use count badges",
            d.nodes.len()
        ));
    }
    if d.edges.len() > MAX_EDGES {
        r.err(format!(
            "{} links is too many to read (at most {MAX_EDGES})",
            d.edges.len()
        ));
    }
    if d.groups.len() > MAX_GROUPS {
        r.err(format!("at most {MAX_GROUPS} groups"));
    }
    let mut group_ids = HashSet::new();
    for g in d.groups.iter_mut() {
        clean(&mut g.label);
        if g.id.trim().is_empty() {
            r.err("a group has no id");
        } else if !group_ids.insert(g.id.clone()) {
            r.err(format!("group id '{}' is used twice", short(&g.id)));
        }
    }
    for g in d.groups.clone() {
        text_rules("group", &g.label, false, r);
    }
    let mut node_ids = HashSet::new();
    for node in d.nodes.iter_mut() {
        clean(&mut node.label);
        if node.id.trim().is_empty() {
            r.err("a node has no id");
        } else if !node_ids.insert(node.id.clone()) {
            r.err(format!("node id '{}' is used twice", short(&node.id)));
        }
        if let Some(g) = &node.group {
            if !group_ids.contains(g) {
                r.err(format!(
                    "node '{}' is in group '{}', which does not exist",
                    short(&node.id),
                    short(g)
                ));
            }
        }
        if !icons::is_known(&node.icon) {
            r.err(format!(
                "node '{}': unknown icon '{}' (one of: {})",
                short(&node.id),
                short(&node.icon),
                icons::ICON_NAMES.join(", ")
            ));
        }
        if node.count == 0 || node.count > 9999 {
            r.err(format!(
                "node '{}': count must be 1 to 9999",
                short(&node.id)
            ));
        }
        // Catalog products.
        if node.sku.as_deref().is_some_and(|s| s.trim().is_empty()) {
            node.sku = None;
        }
        match node.sku.clone() {
            Some(sku) => match product_by_sku(ctx, &sku) {
                None => r.err(format!(
                    "node '{}': sku '{}' is not in the catalog",
                    short(&node.id),
                    short(&sku)
                )),
                Some(p) => {
                    if node.label.trim().is_empty() {
                        node.label = p
                            .short_name
                            .clone()
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or(p.name.clone());
                    } else if norm(&node.label) != norm(&p.name)
                        && p.short_name.as_deref().map(norm) != Some(norm(&node.label))
                    {
                        r.err(format!(
                            "node '{}': label '{}' must be the product's name '{}'{}",
                            short(&node.id),
                            short(&node.label),
                            p.name,
                            p.short_name
                                .as_deref()
                                .map(|s| format!(" or '{s}'"))
                                .unwrap_or_default()
                        ));
                    }
                }
            },
            None => {
                if let Some(p) = product_by_label(ctx, &node.label) {
                    node.sku = Some(p.sku.clone());
                    r.warn(format!(
                        "node '{}': linked to catalog product {}",
                        short(&node.id),
                        p.sku
                    ));
                }
            }
        }
    }
    for node in d.nodes.clone() {
        text_rules("node", &node.label, false, r);
        if node.label.trim().is_empty() {
            r.err(format!("node '{}' has no label", short(&node.id)));
        }
    }
    let mut seen_edges = HashSet::new();
    for e in d.edges.iter_mut() {
        clean_opt(&mut e.label);
        if !node_ids.contains(&e.from) || !node_ids.contains(&e.to) {
            r.err(format!(
                "link {} to {} points at a node that does not exist",
                short(&e.from),
                short(&e.to)
            ));
        }
        if e.from == e.to {
            r.err(format!("link from '{}' to itself", short(&e.from)));
        }
        if !seen_edges.insert((e.from.clone(), e.to.clone(), e.kind)) {
            r.warn(format!(
                "link {} to {} appears twice",
                short(&e.from),
                short(&e.to)
            ));
        }
    }
    for e in d.edges.clone() {
        if let Some(l) = &e.label {
            text_rules("link label", l, false, r);
        }
    }
    if d.kind == DiagramKind::Process && has_cycle(&d.nodes, &d.edges) {
        r.err("a process flow cannot loop back; remove the link that closes the cycle");
    }
    if d.kind == DiagramKind::Process && d.groups.iter().any(|g| g.style != GroupStyle::Lane) {
        r.warn("in a process flow every group is drawn as a lane");
    }
}

/// True when the directed graph has a cycle.
pub fn has_cycle(nodes: &[Node], edges: &[Edge]) -> bool {
    let idx: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    let mut adj = vec![Vec::new(); nodes.len()];
    for e in edges {
        if let (Some(&a), Some(&b)) = (idx.get(e.from.as_str()), idx.get(e.to.as_str())) {
            adj[a].push(b);
        }
    }
    // 0 = new, 1 = on stack, 2 = done
    let mut state = vec![0u8; nodes.len()];
    for s in 0..nodes.len() {
        if state[s] != 0 {
            continue;
        }
        let mut stack = vec![(s, 0usize)];
        state[s] = 1;
        while let Some((v, i)) = stack.pop() {
            if i < adj[v].len() {
                stack.push((v, i + 1));
                let w = adj[v][i];
                if state[w] == 1 {
                    return true;
                }
                if state[w] == 0 {
                    state[w] = 1;
                    stack.push((w, 0));
                }
            } else {
                state[v] = 2;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------------------------------------
// Custom SVG
// ---------------------------------------------------------------------------------------------------------

fn guard_custom(s: &mut CustomSvgSpec, ctx: &GuardContext, r: &mut GuardReport) {
    clean(&mut s.title);
    text_rules("title", &s.title.clone(), false, r);
    match &s.source {
        Some(Source::Customer { note }) | Some(Source::Analysis { note, .. })
            if note.trim().is_empty() =>
        {
            r.err("the source needs a note saying where the numbers come from")
        }
        Some(Source::Erp { reference }) => r.warn(format!(
            "ERP reference '{}' is not checked inside a custom drawing",
            short(reference)
        )),
        _ => {}
    }
    let clean_svg = match sanitize::sanitize(&s.svg, ctx.custom_svg_max_kb) {
        Ok(out) => out,
        Err(e) => {
            r.err(e.to_string());
            return;
        }
    };
    for w in &clean_svg.removed {
        r.warn(format!("removed from the drawing: {w}"));
    }
    s.svg = clean_svg.svg;
    let numeric = clean_svg
        .texts
        .iter()
        .any(|t| t.chars().any(|c| c.is_ascii_digit()));
    for t in &clean_svg.texts {
        text_rules("drawing text", t, false, r);
    }
    if numeric && s.source.is_none() {
        r.warn("the drawing shows numbers but gives no source");
    }
}

// ---------------------------------------------------------------------------------------------------------
// Context from the database
// ---------------------------------------------------------------------------------------------------------

const WHERE: &str = "Sales & CRM → Proposals";

/// `load_context(db, document_id)`: the catalog, `requirements.<key>` refs and the custom SVG cap. `proposal.phases` is added by the proposal code (`with_erp`).
pub async fn load_context(
    db: &mut PgConnection,
    document_id: Option<Uuid>,
) -> ApiResult<GuardContext> {
    let mut ctx = GuardContext::default();
    let rows: Vec<(String, String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT sku, name, short_name, family FROM product_catalog ORDER BY sku")
            .fetch_all(&mut *db)
            .await?;
    for (sku, name, short_name, family) in rows {
        ctx.catalog.push(CatalogProduct {
            sku,
            name,
            short_name,
            family,
        });
    }
    if let Ok(v) = require_json(db, "proposal.visuals.custom_svg_max_kb", WHERE).await {
        if let Some(n) = v
            .as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        {
            if n > 0 {
                ctx.custom_svg_max_kb = n as usize;
            }
        }
    }

    if let Some(doc_id) = document_id {
        let doc: Option<(Option<Uuid>,)> =
            sqlx::query_as("SELECT deal_id FROM sales_documents WHERE id = $1")
                .bind(doc_id)
                .fetch_optional(&mut *db)
                .await?;
        if let Some((Some(deal_id),)) = doc {
            if let Some(sheet) = crate::nary::proposals::sheet(db, deal_id).await? {
                if let Some(Value::Object(fields)) = sheet.content.get("fields") {
                    for (k, f) in fields {
                        let text = match f.get("value") {
                            Some(Value::String(s)) => s.clone(),
                            Some(Value::Number(n)) => n.to_string(),
                            _ => continue,
                        };
                        if let Some(data) = requirement_numbers(&text) {
                            ctx.erp.insert(format!("requirements.{k}"), data);
                        }
                    }
                }
            }
        }
    }
    Ok(ctx)
}

static PAIR: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*([^:=;,]{1,60}?)\s*[:=]\s*(-?\d+(?:[.,]\d+)?)\s*[A-Za-z ]{0,20}$").unwrap()
});
static SINGLE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*(-?\d+(?:[.,]\d+)?)\s*([A-Za-z ]{0,30})$").unwrap());

/// Numbers in a requirement answer: "Gate A: 4; Gate B: 6" (label: number pairs) or a single "40 users".
pub fn requirement_numbers(text: &str) -> Option<ErpData> {
    let parse = |s: &str| s.replace(',', ".").parse::<f64>().ok();
    let parts: Vec<&str> = text
        .split([';', '\n'])
        .flat_map(|p| p.split(", "))
        .collect();
    let pairs: Option<Vec<(String, f64)>> = parts
        .iter()
        .map(|p| {
            let c = PAIR.captures(p)?;
            Some((c[1].trim().to_string(), parse(&c[2])?))
        })
        .collect();
    if let Some(pairs) = pairs.filter(|p| !p.is_empty()) {
        return Some(ErpData {
            categories: pairs.iter().map(|(c, _)| c.clone()).collect(),
            series: vec![Series {
                name: String::new(),
                values: pairs.iter().map(|(_, v)| *v).collect(),
                ..Default::default()
            }],
            ..Default::default()
        });
    }
    let c = SINGLE.captures(text)?;
    let unit = c[2].trim().to_string();
    Some(ErpData {
        categories: vec![if unit.is_empty() {
            "Value".into()
        } else {
            unit.clone()
        }],
        series: vec![Series {
            name: String::new(),
            values: vec![parse(&c[1])?],
            ..Default::default()
        }],
        unit: if unit.is_empty() { None } else { Some(unit) },
        ..Default::default()
    })
}

// visuals3 P3B: the caption of a picture put into a proposal (an image block has no spec to guard).
/// Money and accuracy figures in one caption. Errors refuse it.
pub fn caption_rules(what: &str, caption: &str, _ctx: &GuardContext) -> GuardReport {
    let mut r = GuardReport::default();
    let mut s = caption.trim().to_string();
    clean(&mut s);
    text_rules(what, &s, false, &mut r);
    r
}
