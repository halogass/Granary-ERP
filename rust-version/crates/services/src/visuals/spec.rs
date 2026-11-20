//! The figure spec: the shared contract for `chart` and `diagram` blocks in `sales_document_blocks.data`.
//!
//! A figure block stores `{"spec": {...}, "caption", "size", "brief", "svg_asset_id", "png_asset_id",
//! "checked"}`. The spec is data: every string in it is customer or model text and is escaped when drawn.
//! Chart kinds, diagram kinds and `custom_svg` have distinct `kind` values, so [`Spec`] is told apart by
//! `kind` alone.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

// ---------------------------------------------------------------------------------------------------------
// Figure block
// ---------------------------------------------------------------------------------------------------------

/// The whole `data` of a `chart` / `diagram` block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FigureBlock {
    pub spec: Spec,
    #[serde(default)]
    pub caption: String,
    #[serde(default)]
    pub size: FigureSize,
    /// What the figure must show (used by the vision check).
    #[serde(default)]
    pub brief: Option<String>,
    #[serde(default)]
    pub svg_asset_id: Option<Uuid>,
    #[serde(default)]
    pub png_asset_id: Option<Uuid>,
    #[serde(default)]
    pub checked: Option<Checked>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FigureSize {
    #[default]
    Full,
    Half,
}

/// Result of the last check: `{"ok": true, "notes": [], "at": "iso"}`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Checked {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub at: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
}

// ---------------------------------------------------------------------------------------------------------
// Spec
// ---------------------------------------------------------------------------------------------------------

/// A figure spec. Serialises as the inner object (no wrapper); deserialises by `kind`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Spec {
    Chart(ChartSpec),
    Diagram(DiagramSpec),
    CustomSvg(CustomSvgSpec),
}

pub const CHART_KINDS: [&str; 13] = [
    "bar",
    "stacked_bar",
    "line",
    "area",
    "donut",
    "gantt",
    "milestones",
    "hbar",
    "combo",
    "scatter",
    "heatmap",
    "waterfall",
    "radar",
];
pub const DIAGRAM_KINDS: [&str; 3] = ["architecture", "network", "process"];
pub const CUSTOM_SVG_KIND: &str = "custom_svg";

impl<'de> Deserialize<'de> for Spec {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Spec::from_value(v).map_err(D::Error::custom)
    }
}

impl Spec {
    /// Parse a spec object; the error names the problem in plain words.
    pub fn from_value(v: Value) -> Result<Spec, String> {
        let kind = v
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| "the spec needs a \"kind\"".to_string())?
            .to_string();
        if CHART_KINDS.contains(&kind.as_str()) {
            serde_json::from_value(v)
                .map(Spec::Chart)
                .map_err(|e| format!("chart spec: {e}"))
        } else if DIAGRAM_KINDS.contains(&kind.as_str()) {
            serde_json::from_value(v)
                .map(Spec::Diagram)
                .map_err(|e| format!("diagram spec: {e}"))
        } else if kind == CUSTOM_SVG_KIND {
            serde_json::from_value(v)
                .map(Spec::CustomSvg)
                .map_err(|e| format!("custom_svg spec: {e}"))
        } else {
            Err(format!(
                "unknown kind '{kind}' (charts: {}; diagrams: {}; or custom_svg)",
                CHART_KINDS.join(", "),
                DIAGRAM_KINDS.join(", ")
            ))
        }
    }

    /// Parse the spec of a block of type `chart` or `diagram`; the kind must fit the block type
    /// (`custom_svg` fits both).
    pub fn from_block(block_type: &str, v: Value) -> Result<Spec, String> {
        let spec = Spec::from_value(v)?;
        match (block_type, &spec) {
            ("chart", Spec::Chart(_)) | ("diagram", Spec::Diagram(_)) | (_, Spec::CustomSvg(_)) => {
                Ok(spec)
            }
            ("chart", _) => Err("a chart block needs a chart kind".into()),
            ("diagram", _) => Err("a diagram block needs a diagram kind".into()),
            _ => Err(format!("'{block_type}' is not a figure block type")),
        }
    }

    /// The block type this spec belongs in ("chart" or "diagram"; custom_svg is "chart").
    pub fn block_type(&self) -> &'static str {
        match self {
            Spec::Diagram(_) => "diagram",
            _ => "chart",
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Spec::Chart(c) => c.kind.as_str(),
            Spec::Diagram(d) => d.kind.as_str(),
            Spec::CustomSvg(_) => CUSTOM_SVG_KIND,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Spec::Chart(c) => &c.title,
            Spec::Diagram(d) => &d.title,
            Spec::CustomSvg(s) => &s.title,
        }
    }

    pub fn language(&self) -> Language {
        match self {
            Spec::Chart(c) => c.language,
            Spec::Diagram(d) => d.language,
            Spec::CustomSvg(s) => s.language,
        }
    }

    pub fn source(&self) -> Option<&Source> {
        match self {
            Spec::Chart(c) => c.source.as_ref(),
            Spec::Diagram(_) => None,
            Spec::CustomSvg(s) => s.source.as_ref(),
        }
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// The line printed under the caption (PDF, editor, tools): "Source: ..." for customer and analysis
    /// numbers.
    pub fn caption_line(&self) -> Option<String> {
        self.source()
            .and_then(|src| src.caption_line(self.language()))
    }
}

// ---------------------------------------------------------------------------------------------------------
// Chart
// ---------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartKind {
    Bar,
    StackedBar,
    Line,
    Area,
    Donut,
    Gantt,
    Milestones,
    Hbar,
    Combo,
    Scatter,
    Heatmap,
    Waterfall,
    Radar,
}

impl ChartKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChartKind::Bar => "bar",
            ChartKind::StackedBar => "stacked_bar",
            ChartKind::Line => "line",
            ChartKind::Area => "area",
            ChartKind::Donut => "donut",
            ChartKind::Gantt => "gantt",
            ChartKind::Milestones => "milestones",
            ChartKind::Hbar => "hbar",
            ChartKind::Combo => "combo",
            ChartKind::Scatter => "scatter",
            ChartKind::Heatmap => "heatmap",
            ChartKind::Waterfall => "waterfall",
            ChartKind::Radar => "radar",
        }
    }
}

/// A chart. Which fields matter depends on `kind`:
/// - bar / stacked_bar / hbar / line / area / combo / waterfall / radar: `categories` + `series[].values`
///   (one value per category); combo: `series[].type` bar|line and `series[].axis` left|right;
///   waterfall: one series, the first value a starting total unless `start_total` is false, then changes,
///   a total bar at the end unless `show_total` is false;
/// - donut: `categories` + the first series;
/// - heatmap: `categories` are the columns, each series is a row (`name` = row label);
/// - scatter: `series[].points` [{x, y, label}];
/// - gantt: `tasks` + `milestones` + `time_unit`; milestones: `milestones` + `time_unit`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChartSpec {
    pub kind: ChartKind,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub language: Language,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub x_label: Option<String>,
    #[serde(default)]
    pub y_label: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub series: Vec<Series>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<GanttTask>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub milestones: Vec<Milestone>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_unit: Option<TimeUnit>,
    #[serde(default)]
    pub source: Option<Source>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<Annotation>,
    /// Per series colours, in series order: theme palette names (red, blue, slate, green, amber, maroon,
    /// navy, sand) or #rrggbb.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub colors: Vec<String>,
    /// bar / hbar / area: stack the series.
    #[serde(default, skip_serializing_if = "is_false")]
    pub stacked: bool,
    /// bar / hbar / area (stacked): each category as 100 %.
    #[serde(default, skip_serializing_if = "is_false")]
    pub percent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legend: Option<LegendPosition>,
    /// waterfall: draw the total bar (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_total: Option<bool>,
    /// waterfall: the first value is a starting total drawn from 0 (default true); false = every value is
    /// a change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_total: Option<bool>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Series {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub values: Vec<f64>,
    /// scatter only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<Point>,
    /// combo only: bar (default) or line.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub series_type: Option<SeriesType>,
    /// combo only: left (default) or right axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<Axis>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeriesType {
    Bar,
    Line,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LegendPosition {
    Bottom,
    Top,
    Right,
    None,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GanttTask {
    pub name: String,
    /// Offset from the start of the timeline, in `time_unit`s.
    pub start: f64,
    /// Duration in `time_unit`s.
    pub length: f64,
    #[serde(default)]
    pub group: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Milestone {
    pub name: String,
    pub at: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeUnit {
    Day,
    #[default]
    Week,
    Month,
}

/// Where the numbers come from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Source {
    /// `proposal.phases | requirements.<key>`; resolved by the guard.
    Erp {
        #[serde(rename = "ref")]
        reference: String,
    },
    /// The customer's own numbers; `note` (required) says where they come from.
    Customer {
        #[serde(default)]
        note: String,
    },
    /// Numbers Nary worked out from ERP data or a customer file it read.
    Analysis {
        #[serde(default)]
        note: String,
        #[serde(default)]
        from: Vec<String>,
    },
}

impl Source {
    /// The "Source: ..." line printed under the caption (customer and analysis sources).
    pub fn caption_line(&self, language: Language) -> Option<String> {
        let word = crate::visuals::i18n::word("source", language);
        match self {
            Source::Customer { note } | Source::Analysis { note, .. }
                if !note.trim().is_empty() =>
            {
                Some(format!("{word}: {}", note.trim()))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Annotation {
    /// A horizontal (axis y) or vertical (axis x) reference line at `value`.
    RefLine {
        #[serde(default = "default_axis_y")]
        axis: String,
        value: f64,
        #[serde(default)]
        label: String,
    },
    /// Emphasise one data point: series name and category index.
    Highlight {
        series: String,
        index: usize,
        #[serde(default)]
        label: String,
    },
    /// A free note at data coordinates [x, y] (x = category index for category charts).
    Note { text: String, at: [f64; 2] },
}

fn default_axis_y() -> String {
    "y".into()
}

// ---------------------------------------------------------------------------------------------------------
// Diagram
// ---------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagramKind {
    Architecture,
    Network,
    Process,
}

impl DiagramKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            DiagramKind::Architecture => "architecture",
            DiagramKind::Network => "network",
            DiagramKind::Process => "process",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    #[default]
    LR,
    TB,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiagramSpec {
    pub kind: DiagramKind,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub language: Language,
    #[serde(default)]
    pub direction: Direction,
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupStyle {
    #[default]
    Site,
    CustomerSystem,
    Cloud,
    Zone,
    Lane,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub style: GroupStyle,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    #[serde(default)]
    pub label: String,
    /// One of [`crate::visuals::icons::ICON_NAMES`].
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default = "one")]
    pub count: u32,
}

fn one() -> u32 {
    1
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    Ethernet,
    Poe,
    Fiber,
    #[serde(rename = "4g")]
    FourG,
    Wifi,
    Power,
    #[default]
    Data,
    Api,
    Relay,
}

impl EdgeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeKind::Ethernet => "ethernet",
            EdgeKind::Poe => "poe",
            EdgeKind::Fiber => "fiber",
            EdgeKind::FourG => "4g",
            EdgeKind::Wifi => "wifi",
            EdgeKind::Power => "power",
            EdgeKind::Data => "data",
            EdgeKind::Api => "api",
            EdgeKind::Relay => "relay",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub kind: EdgeKind,
    #[serde(default)]
    pub label: Option<String>,
}

// ---------------------------------------------------------------------------------------------------------
// Custom SVG
// ---------------------------------------------------------------------------------------------------------

/// A drawing authored by Nary or the MCP client. Sanitised to an allow-list before it is stored or drawn
/// (`crate::visuals::sanitize`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomSvgSpec {
    pub kind: CustomSvgKind,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub language: Language,
    pub svg: String,
    #[serde(default)]
    pub source: Option<Source>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomSvgKind {
    #[default]
    CustomSvg,
}

// ---------------------------------------------------------------------------------------------------------
// JSON schemas for the model (structured output / tool input)
// ---------------------------------------------------------------------------------------------------------

fn nullable(t: &str) -> Value {
    json!({"type": [t, "null"]})
}

fn source_schema() -> Value {
    json!({"anyOf": [
        {"type": "object", "properties": {"type": {"type": "string", "enum": ["erp"]},
            "ref": {"type": "string", "description": "proposal.phases | requirements.<key>"}},
         "required": ["type", "ref"], "additionalProperties": false},
        {"type": "object", "properties": {"type": {"type": "string", "enum": ["customer"]},
            "note": {"type": "string", "description": "where the customer's numbers come from (required)"}},
         "required": ["type", "note"], "additionalProperties": false},
        {"type": "object", "properties": {"type": {"type": "string", "enum": ["analysis"]},
            "note": {"type": "string", "description": "how the numbers were worked out (required)"},
            "from": {"type": "array", "items": {"type": "string"}, "description": "tools/files used, e.g. get_pipeline, read_attachment:<file id>"}},
         "required": ["type", "note", "from"], "additionalProperties": false},
        {"type": "null"}
    ]})
}

/// JSON schema of a chart spec.
pub fn chart_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "enum": CHART_KINDS},
            "title": {"type": "string"},
            "language": {"type": "string", "enum": ["en"]},
            "unit": nullable("string"),
            "x_label": nullable("string"),
            "y_label": nullable("string"),
            "categories": {"type": "array", "items": {"type": "string"}},
            "series": {"type": "array", "items": {"type": "object", "properties": {
                "name": {"type": "string"},
                "values": {"type": "array", "items": {"type": "number"}},
                "points": {"type": "array", "items": {"type": "object", "properties": {
                    "x": {"type": "number"}, "y": {"type": "number"}, "label": nullable("string")},
                    "required": ["x", "y"], "additionalProperties": false}},
                "type": {"anyOf": [{"type": "string", "enum": ["bar", "line"]}, {"type": "null"}]},
                "axis": {"anyOf": [{"type": "string", "enum": ["left", "right"]}, {"type": "null"}]}
            }, "required": ["name", "values"], "additionalProperties": false}},
            "tasks": {"type": "array", "items": {"type": "object", "properties": {
                "name": {"type": "string"}, "start": {"type": "number"}, "length": {"type": "number"},
                "group": nullable("string")}, "required": ["name", "start", "length"], "additionalProperties": false}},
            "milestones": {"type": "array", "items": {"type": "object", "properties": {
                "name": {"type": "string"}, "at": {"type": "number"}}, "required": ["name", "at"], "additionalProperties": false}},
            "time_unit": {"anyOf": [{"type": "string", "enum": ["day", "week", "month"]}, {"type": "null"}]},
            "source": source_schema(),
            "annotations": {"type": "array", "items": {"anyOf": [
                {"type": "object", "properties": {"type": {"type": "string", "enum": ["ref_line"]},
                    "axis": {"type": "string", "enum": ["x", "y"]}, "value": {"type": "number"}, "label": {"type": "string"}},
                 "required": ["type", "value"], "additionalProperties": false},
                {"type": "object", "properties": {"type": {"type": "string", "enum": ["highlight"]},
                    "series": {"type": "string"}, "index": {"type": "integer"}, "label": {"type": "string"}},
                 "required": ["type", "series", "index"], "additionalProperties": false},
                {"type": "object", "properties": {"type": {"type": "string", "enum": ["note"]},
                    "text": {"type": "string"}, "at": {"type": "array", "items": {"type": "number"}}},
                 "required": ["type", "text", "at"], "additionalProperties": false}
            ]}},
            "colors": {"type": "array", "items": {"type": "string"}, "description": "per series: red, blue, slate, green, amber, maroon, navy, sand or #rrggbb"},
            "stacked": {"type": "boolean"},
            "percent": {"type": "boolean"},
            "legend": {"anyOf": [{"type": "string", "enum": ["bottom", "top", "right", "none"]}, {"type": "null"}]},
            "show_total": {"type": ["boolean", "null"]},
            "start_total": {"type": ["boolean", "null"], "description": "waterfall: the first value is a starting total (default true)"}
        },
        "required": ["kind", "title", "language", "categories", "series", "source"],
        "additionalProperties": false
    })
}

/// JSON schema of a diagram spec.
pub fn diagram_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "enum": DIAGRAM_KINDS},
            "title": {"type": "string"},
            "language": {"type": "string", "enum": ["en"]},
            "direction": {"type": "string", "enum": ["LR", "TB"]},
            "groups": {"type": "array", "items": {"type": "object", "properties": {
                "id": {"type": "string"}, "label": {"type": "string"},
                "style": {"type": "string", "enum": ["site", "customer_system", "cloud", "zone", "lane"]}},
                "required": ["id", "label", "style"], "additionalProperties": false}},
            "nodes": {"type": "array", "items": {"type": "object", "properties": {
                "id": {"type": "string"}, "label": {"type": "string"},
                "icon": {"type": "string", "enum": crate::visuals::icons::ICON_NAMES.to_vec()},
                "sku": nullable("string"), "group": nullable("string"), "count": {"type": "integer"}},
                "required": ["id", "label", "icon", "sku", "group", "count"], "additionalProperties": false}},
            "edges": {"type": "array", "items": {"type": "object", "properties": {
                "from": {"type": "string"}, "to": {"type": "string"},
                "kind": {"type": "string", "enum": ["ethernet", "poe", "fiber", "4g", "wifi", "power", "data", "api", "relay"]},
                "label": nullable("string")}, "required": ["from", "to", "kind", "label"], "additionalProperties": false}}
        },
        "required": ["kind", "title", "language", "direction", "groups", "nodes", "edges"],
        "additionalProperties": false
    })
}

/// JSON schema of a custom SVG spec.
pub fn custom_svg_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "enum": [CUSTOM_SVG_KIND]},
            "title": {"type": "string"},
            "language": {"type": "string", "enum": ["en"]},
            "svg": {"type": "string", "description": "one <svg> with a viewBox; shapes, paths and text only (no scripts, images, external links, foreignObject, CSS)"},
            "source": source_schema()
        },
        "required": ["kind", "title", "svg", "source"],
        "additionalProperties": false
    })
}

/// JSON schema of any spec (chart, diagram or custom_svg).
pub fn spec_schema() -> Value {
    json!({"anyOf": [chart_schema(), diagram_schema(), custom_svg_schema()]})
}

/// Every spec a tool accepts.
pub fn any_spec_schema() -> Value {
    spec_schema()
}
