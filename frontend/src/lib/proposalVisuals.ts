/**
 * Proposal figures (charts and diagrams, PROPOSAL_VISUALS_P1_BRIEF): the spec types the editor and the chat
 * cards share, and the pure edits the data panels make. The server renders and guards the spec; nothing here
 * draws or trusts model SVG (a figure is always shown as an <img> of the stored asset or a data URL).
 */

export type ChartKind =
  | "bar" | "stacked_bar" | "line" | "area" | "donut" | "gantt" | "milestones"
  | "hbar" | "combo" | "scatter" | "heatmap" | "waterfall" | "radar";
export type DiagramKind = "architecture" | "network" | "process";
export type FigureKind = ChartKind | DiagramKind | "custom_svg";

export interface Point { x: number; y: number; label?: string | null }
export interface Series {
  name: string; values: number[];
  /** scatter only */ points?: Point[];
  /** combo only */ type?: "bar" | "line"; axis?: "left" | "right";
  [k: string]: unknown;
}
export interface GanttTask { name: string; start: number; length: number; group?: string | null }
export interface Milestone { name: string; at: number }
export type Source =
  | { type: "erp"; ref: string }
  | { type: "customer"; note: string }
  | { type: "analysis"; note: string; from?: string[] };

export interface ChartSpec {
  kind: ChartKind; title: string; language?: string;
  unit?: string | null; x_label?: string | null; y_label?: string | null;
  categories?: string[]; series?: Series[];
  tasks?: GanttTask[]; milestones?: Milestone[]; time_unit?: "day" | "week" | "month";
  annotations?: Record<string, unknown>[];
  colors?: string[]; stacked?: boolean; percent?: boolean; legend?: string; show_total?: boolean;
  // waterfall: the first value is a starting total drawn from 0 (default true)
  start_total?: boolean;
  source?: Source; [k: string]: unknown;
}
export interface DiagramGroup { id: string; label: string; style: "site" | "customer_system" | "cloud" | "zone" | "lane" }
export interface DiagramNode { id: string; label: string; icon: string; sku?: string | null; group?: string | null; count?: number }
export interface DiagramEdge { from: string; to: string; kind: EdgeKind; label?: string | null }
export interface DiagramSpec {
  kind: DiagramKind; title: string; language?: string; direction?: "LR" | "TB";
  groups: DiagramGroup[]; nodes: DiagramNode[]; edges: DiagramEdge[]; [k: string]: unknown;
}
export interface CustomSvgSpec { kind: "custom_svg"; title: string; svg: string; source?: Source; [k: string]: unknown }
export type FigureSpec = ChartSpec | DiagramSpec | CustomSvgSpec;

/** `data` of a block of type chart or diagram. */
export interface FigureData {
  spec: FigureSpec; caption?: string; size?: "full" | "half"; brief?: string;
  svg_asset_id?: string | null; png_asset_id?: string | null;
  checked?: { ok: boolean; notes?: string[]; at?: string } | null;
  [k: string]: unknown;
}

export const CHART_KINDS: { value: ChartKind; label: string }[] = [
  { value: "bar", label: "Bars" }, { value: "hbar", label: "Horizontal bars" }, { value: "stacked_bar", label: "Stacked bars" },
  { value: "line", label: "Line" }, { value: "area", label: "Area" }, { value: "combo", label: "Bars and line" },
  { value: "donut", label: "Donut" }, { value: "waterfall", label: "Waterfall" }, { value: "radar", label: "Radar" },
  { value: "scatter", label: "Scatter" }, { value: "heatmap", label: "Heatmap" },
  { value: "gantt", label: "Timeline (Gantt)" }, { value: "milestones", label: "Milestones" },
];
export const DIAGRAM_KINDS: { value: DiagramKind; label: string }[] = [
  { value: "architecture", label: "System architecture" }, { value: "network", label: "Network" }, { value: "process", label: "Process" },
];
export const GROUP_STYLES: { value: DiagramGroup["style"]; label: string }[] = [
  { value: "site", label: "Site" }, { value: "customer_system", label: "Customer system" }, { value: "cloud", label: "Cloud" },
  { value: "zone", label: "Zone" }, { value: "lane", label: "Lane" },
];
export type EdgeKind = "ethernet" | "poe" | "fiber" | "4g" | "wifi" | "power" | "data" | "api" | "relay";
export const EDGE_KINDS: { value: EdgeKind; label: string }[] = [
  { value: "ethernet", label: "Ethernet" }, { value: "poe", label: "PoE" }, { value: "fiber", label: "Fiber" },
  { value: "4g", label: "4G" }, { value: "wifi", label: "Wi-Fi" }, { value: "power", label: "Power" },
  { value: "data", label: "Data" }, { value: "api", label: "API" }, { value: "relay", label: "Relay" },
];

/** Icon names the renderer knows, grouped for the picker. */
export const ICON_GROUPS: { label: string; icons: string[] }[] = [
  { label: "People and places", icons: ["person", "user", "door", "gate", "wall", "box"] },
  { label: "Systems", icons: ["server", "rack", "cloud", "database", "laptop", "phone", "screen", "erp", "api", "sensor"] },
  { label: "Network", icons: ["networkSwitch", "router4g"] },
];
export const ICONS = ICON_GROUPS.flatMap((g) => g.icons);

/** "networkSwitch" -> "Network Switch", "router4g" -> "Router 4G". */
export function iconLabel(name: string): string {
  return name
    .replace(/([a-z])([A-Z0-9])/g, "$1 $2")
    .replace(/(\d)([a-z])/gi, "$1$2")
    .replace(/\b4g\b/i, "4G").replace(/\berp\b/i, "ERP").replace(/\bapi\b/i, "API")
    .replace(/^./, (c) => c.toUpperCase()).replace(/\s+/g, " ").trim();
}

const DIAGRAMS = new Set<string>(["architecture", "network", "process"]);
export const isDiagram = (s: FigureSpec): s is DiagramSpec => DIAGRAMS.has(s.kind);
export const isCustomSvg = (s: FigureSpec): s is CustomSvgSpec => s.kind === "custom_svg";
export const isChart = (s: FigureSpec): s is ChartSpec => !isDiagram(s) && !isCustomSvg(s);
/** The block type a spec belongs in. */
export const blockTypeOf = (s: FigureSpec): "chart" | "diagram" => (isDiagram(s) ? "diagram" : "chart");
/** Chart kinds the categories x series grid edits (a heatmap: categories are columns, each series a row);
 *  scatter edits points per series. */
const GRID_KINDS = new Set<string>(["bar", "stacked_bar", "line", "area", "donut", "hbar", "combo", "waterfall", "radar", "heatmap"]);
export const canStack = (k: string) => k === "bar" || k === "hbar" || k === "area";
export const usesGrid = (k: string) => GRID_KINDS.has(k);
export const usesTasks = (k: string) => k === "gantt" || k === "milestones";

export function emptyChart(language: string = "en"): ChartSpec {
  return { kind: "bar", title: "", language: lang(language), unit: null, x_label: null, y_label: null,
    categories: ["", ""], series: [{ name: "", values: [0, 0] }], source: { type: "customer", note: "" } };
}
export function emptyDiagram(language: string = "en"): DiagramSpec {
  return { kind: "architecture", title: "", language: lang(language), direction: "LR", groups: [],
    nodes: [{ id: "n1", label: "", icon: "server", sku: null, group: null, count: 1 }], edges: [] };
}
const lang = (l: string): string => l || "en";

/** A typed number: "1,5" and "1.5" both read 1.5; anything else is null (the cell keeps the old value). */
export function parseNumber(raw: string): number | null {
  const t = raw.trim().replace(/\s/g, "").replace(",", ".");
  if (!t || !/^-?\d*\.?\d+$|^-?\d+\.$/.test(t)) return null;
  const n = Number(t);
  return Number.isFinite(n) ? n : null;
}

// ---- the categories x series grid -------------------------------------------------------------------------

/** Every series has one value per category (missing ones read 0). */
export function normalizeGrid(spec: ChartSpec): ChartSpec {
  const cats = spec.categories ?? [];
  const series = (spec.series ?? []).map((s) => ({ ...s, values: cats.map((_, i) => Number(s.values?.[i] ?? 0) || 0) }));
  return { ...spec, categories: cats, series };
}
export function setCategory(spec: ChartSpec, i: number, name: string): ChartSpec {
  const s = normalizeGrid(spec);
  return { ...s, categories: s.categories!.map((c, j) => (j === i ? name : c)) };
}
export function addCategory(spec: ChartSpec): ChartSpec {
  const s = normalizeGrid(spec);
  return { ...s, categories: [...s.categories!, ""], series: s.series!.map((x) => ({ ...x, values: [...x.values, 0] })) };
}
export function removeCategory(spec: ChartSpec, i: number): ChartSpec {
  const s = normalizeGrid(spec);
  return { ...s, categories: s.categories!.filter((_, j) => j !== i), series: s.series!.map((x) => ({ ...x, values: x.values.filter((_, j) => j !== i) })) };
}
export function setSeriesName(spec: ChartSpec, j: number, name: string): ChartSpec {
  const s = normalizeGrid(spec);
  return { ...s, series: s.series!.map((x, k) => (k === j ? { ...x, name } : x)) };
}
export function addSeries(spec: ChartSpec): ChartSpec {
  const s = normalizeGrid(spec);
  return { ...s, series: [...s.series!, { name: "", values: s.categories!.map(() => 0) }] };
}
export function removeSeries(spec: ChartSpec, j: number): ChartSpec {
  const s = normalizeGrid(spec);
  return { ...s, series: s.series!.filter((_, k) => k !== j) };
}
export function setValue(spec: ChartSpec, i: number, j: number, value: number): ChartSpec {
  const s = normalizeGrid(spec);
  return { ...s, series: s.series!.map((x, k) => (k === j ? { ...x, values: x.values.map((v, m) => (m === i ? value : v)) } : x)) };
}

export function setSeries(spec: ChartSpec, j: number, patch: Partial<Series>): ChartSpec {
  return { ...spec, series: (spec.series ?? []).map((x, k) => (k === j ? { ...x, ...patch } : x)) };
}

// ---- scatter points ---------------------------------------------------------------------------------------

export function setPoint(spec: ChartSpec, j: number, i: number, patch: Partial<Point>): ChartSpec {
  return setSeries(spec, j, { points: (spec.series?.[j]?.points ?? []).map((p, k) => (k === i ? { ...p, ...patch } : p)) });
}
export function addPoint(spec: ChartSpec, j: number): ChartSpec {
  if (!(spec.series ?? []).length) spec = { ...spec, series: [{ name: "", values: [], points: [] }] };
  return setSeries(spec, j, { points: [...(spec.series![j]?.points ?? []), { x: 0, y: 0, label: null }] });
}
export function removePoint(spec: ChartSpec, j: number, i: number): ChartSpec {
  return setSeries(spec, j, { points: (spec.series?.[j]?.points ?? []).filter((_, k) => k !== i) });
}

// ---- gantt tasks and milestones ---------------------------------------------------------------------------

export function setTask(spec: ChartSpec, i: number, patch: Partial<GanttTask>): ChartSpec {
  return { ...spec, tasks: (spec.tasks ?? []).map((t, j) => (j === i ? { ...t, ...patch } : t)) };
}
export function addTask(spec: ChartSpec): ChartSpec {
  const tasks = spec.tasks ?? [];
  const end = tasks.reduce((m, t) => Math.max(m, (Number(t.start) || 0) + (Number(t.length) || 0)), 0);
  return { ...spec, tasks: [...tasks, { name: "", start: end, length: 1, group: null }] };
}
export const removeTask = (spec: ChartSpec, i: number): ChartSpec => ({ ...spec, tasks: (spec.tasks ?? []).filter((_, j) => j !== i) });
export function setMilestone(spec: ChartSpec, i: number, patch: Partial<Milestone>): ChartSpec {
  return { ...spec, milestones: (spec.milestones ?? []).map((m, j) => (j === i ? { ...m, ...patch } : m)) };
}
export function addMilestone(spec: ChartSpec): ChartSpec {
  const ms = spec.milestones ?? [];
  const end = (spec.tasks ?? []).reduce((m, t) => Math.max(m, (Number(t.start) || 0) + (Number(t.length) || 0)), 0);
  return { ...spec, milestones: [...ms, { name: "", at: end }] };
}
export const removeMilestone = (spec: ChartSpec, i: number): ChartSpec => ({ ...spec, milestones: (spec.milestones ?? []).filter((_, j) => j !== i) });

/** Changing the chart kind keeps the data the new kind uses (a gantt gets a first task, a grid a first row). */
export function setChartKind(spec: ChartSpec, kind: ChartKind): ChartSpec {
  const next: ChartSpec = { ...spec, kind };
  if (usesTasks(kind)) {
    if (kind === "gantt" && !(next.tasks ?? []).length) next.tasks = [{ name: "", start: 0, length: 1, group: null }];
    if (!next.time_unit) next.time_unit = "week";
    if (kind === "milestones" && !(next.milestones ?? []).length) next.milestones = [{ name: "", at: 0 }];
  } else if (usesGrid(kind) && !(next.categories ?? []).length) {
    next.categories = [""]; next.series = [{ name: "", values: [0] }];
  } else if (kind === "scatter" && !(next.series ?? []).some((s) => s.points?.length)) {
    next.series = [{ name: next.series?.[0]?.name ?? "", values: [], points: [{ x: 0, y: 0, label: null }] }];
  }
  if (!canStack(kind)) { delete next.stacked; delete next.percent; }
  return next;
}

// ---- diagram lists ----------------------------------------------------------------------------------------

function nextId(prefix: string, taken: string[]): string {
  let n = taken.length + 1;
  while (taken.includes(`${prefix}${n}`)) n++;
  return `${prefix}${n}`;
}
export function addNode(spec: DiagramSpec): DiagramSpec {
  const id = nextId("n", spec.nodes.map((n) => n.id));
  return { ...spec, nodes: [...spec.nodes, { id, label: "", icon: "server", sku: null, group: null, count: 1 }] };
}
export function setNode(spec: DiagramSpec, id: string, patch: Partial<DiagramNode>): DiagramSpec {
  return { ...spec, nodes: spec.nodes.map((n) => (n.id === id ? { ...n, ...patch } : n)) };
}
/** Removing a node removes its links too, so no edge points at nothing. */
export function removeNode(spec: DiagramSpec, id: string): DiagramSpec {
  return { ...spec, nodes: spec.nodes.filter((n) => n.id !== id), edges: spec.edges.filter((e) => e.from !== id && e.to !== id) };
}
export function addGroup(spec: DiagramSpec): DiagramSpec {
  const id = nextId("g", spec.groups.map((g) => g.id));
  return { ...spec, groups: [...spec.groups, { id, label: "", style: spec.kind === "process" ? "lane" : "site" }] };
}
export function setGroup(spec: DiagramSpec, id: string, patch: Partial<DiagramGroup>): DiagramSpec {
  return { ...spec, groups: spec.groups.map((g) => (g.id === id ? { ...g, ...patch } : g)) };
}
/** Removing a group leaves its nodes ungrouped. */
export function removeGroup(spec: DiagramSpec, id: string): DiagramSpec {
  return { ...spec, groups: spec.groups.filter((g) => g.id !== id), nodes: spec.nodes.map((n) => (n.group === id ? { ...n, group: null } : n)) };
}
export function addEdge(spec: DiagramSpec): DiagramSpec {
  const [a, b] = spec.nodes;
  return { ...spec, edges: [...spec.edges, { from: a?.id ?? "", to: b?.id ?? a?.id ?? "", kind: spec.kind === "process" ? "data" : "ethernet", label: null }] };
}
export function setEdge(spec: DiagramSpec, i: number, patch: Partial<DiagramEdge>): DiagramSpec {
  return { ...spec, edges: spec.edges.map((e, j) => (j === i ? { ...e, ...patch } : e)) };
}
export const removeEdge = (spec: DiagramSpec, i: number): DiagramSpec => ({ ...spec, edges: spec.edges.filter((_, j) => j !== i) });

// ---- display ----------------------------------------------------------------------------------------------

export function kindLabel(kind: string): string {
  if (kind === "custom_svg") return "Custom drawing";
  return CHART_KINDS.find((k) => k.value === kind)?.label ?? DIAGRAM_KINDS.find((k) => k.value === kind)?.label ?? kind;
}

/** The "Source:" line under a figure with customer or analysis numbers (the PDF prints the same). */
export function sourceLine(spec: FigureSpec | null | undefined): string | null {
  const s = (spec as { source?: Source } | null | undefined)?.source;
  if (!s || (s.type !== "customer" && s.type !== "analysis")) return null;
  const note = String(s.note ?? "").trim();
  return note ? `Source: ${note}` : null;
}

/** An SVG string as an <img> src: the browser never runs it as markup. */
export function svgDataUrl(svg: string): string {
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}
/** A PNG (base64, with or without its data: prefix) as an <img> src. */
export function pngDataUrl(b64: string): string {
  return b64.startsWith("data:image/") ? b64 : `data:image/png;base64,${b64}`;
}
