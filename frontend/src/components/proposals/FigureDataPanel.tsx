"use client";

import { useEffect, useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import { Button, Checkbox, IconButton, Select, controlClass } from "@/components/ui";
import { cn } from "@/lib/utils";
import { stack } from "@/lib/stackTable";
import {
  CHART_KINDS, DIAGRAM_KINDS, EDGE_KINDS, GROUP_STYLES, ICON_GROUPS, iconLabel, isChart, isCustomSvg, isDiagram,
  parseNumber, usesGrid, usesTasks, canStack, setSeries, setPoint, addPoint, removePoint,
  addCategory, addEdge, addGroup, addMilestone, addNode, addSeries, addTask, normalizeGrid, removeCategory, removeEdge,
  removeGroup, removeMilestone, removeNode, removeSeries, removeTask, setCategory, setChartKind, setEdge, setGroup,
  setMilestone, setNode, setSeriesName, setTask, setValue,
  type ChartKind, type ChartSpec, type DiagramSpec, type FigureSpec, type Source,
} from "@/lib/proposalVisuals";

/** The data behind a figure, editable: a chart as a categories x series grid (or a task list for a timeline),
 *  a diagram as lists of groups, nodes and links. Every edit produces a new spec; the server guards it. */
export default function FigureDataPanel({ spec, editable, onChange }: {
  spec: FigureSpec; editable: boolean; onChange: (s: FigureSpec) => void;
}) {
  if (isCustomSvg(spec)) {
    return (
      <p className="rounded-lg border border-line-soft bg-surface-2 p-2 text-xs text-fg-muted" aria-label="Custom drawing">
        Custom drawing made by Nary or the MCP client. It has no data grid: ask Nary to change this figure.
      </p>
    );
  }
  if (isDiagram(spec)) return <DiagramPanel spec={spec} editable={editable} onChange={onChange} />;
  if (isChart(spec)) return <ChartPanel spec={spec} editable={editable} onChange={onChange} />;
  return null;
}

const cell = cn(controlClass, "h-11 px-2 max-md:text-base md:h-7 md:text-xs");
const sel = "h-11 max-md:text-base md:h-7 md:text-xs";
const opt = (s: string | null | undefined) => (s && s.trim() ? s : null);

/** A number input that keeps what is typed ("1." or "") until it reads as a number. */
function NumberCell({ value, label, disabled, onChange }: { value: number; label: string; disabled?: boolean; onChange: (n: number) => void }) {
  const [text, setText] = useState(String(value));
  useEffect(() => { if (parseNumber(text) !== value) setText(String(value)); }, [value]); // eslint-disable-line react-hooks/exhaustive-deps
  return (
    <input aria-label={label} inputMode="decimal" autoComplete="off" value={text} disabled={disabled}
      onChange={(e) => { setText(e.target.value); const n = parseNumber(e.target.value); if (n !== null) onChange(n); }}
      onBlur={() => { if (parseNumber(text) === null) setText(String(value)); }}
      className={cn(cell, "w-full text-right tabular-nums")} />
  );
}

function TextCell({ value, label, placeholder, disabled, onChange, className }: {
  value: string; label: string; placeholder?: string; disabled?: boolean; onChange: (v: string) => void; className?: string;
}) {
  return <input aria-label={label} value={value} placeholder={placeholder} autoComplete="off" disabled={disabled}
    onChange={(e) => onChange(e.target.value)} className={cn(cell, "w-full", className)} />;
}

function ChartPanel({ spec, editable, onChange }: { spec: ChartSpec; editable: boolean; onChange: (s: FigureSpec) => void }) {
  const ro = !editable;
  return (
    <div className="space-y-3 text-xs">
      <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
        <label className="space-y-1"><span className="text-fg-muted">Chart</span>
          <Select aria-label="Chart kind" value={spec.kind} disabled={ro} className={sel}
            onChange={(e) => onChange(setChartKind(spec, e.target.value as ChartKind))} options={CHART_KINDS} /></label>
        <label className="space-y-1"><span className="text-fg-muted">Title</span>
          <TextCell label="Chart title" value={spec.title ?? ""} disabled={ro} onChange={(v) => onChange({ ...spec, title: v })} /></label>
        {usesTasks(spec.kind) ? (
          <label className="space-y-1"><span className="text-fg-muted">Time unit</span>
            <Select aria-label="Time unit" value={spec.time_unit ?? "week"} disabled={ro} className={sel}
              onChange={(e) => onChange({ ...spec, time_unit: e.target.value as ChartSpec["time_unit"] })}
              options={[{ value: "day", label: "Days" }, { value: "week", label: "Weeks" }, { value: "month", label: "Months" }]} /></label>
        ) : (
          <>
            <label className="space-y-1"><span className="text-fg-muted">Unit</span>
              <TextCell label="Unit" placeholder="e.g. users, hours" value={spec.unit ?? ""} disabled={ro} onChange={(v) => onChange({ ...spec, unit: opt(v) })} /></label>
            <label className="space-y-1"><span className="text-fg-muted">Axis labels</span>
              <div className="flex gap-1">
                <TextCell label="X axis label" placeholder="X axis" value={spec.x_label ?? ""} disabled={ro} onChange={(v) => onChange({ ...spec, x_label: opt(v) })} />
                <TextCell label="Y axis label" placeholder="Y axis" value={spec.y_label ?? ""} disabled={ro} onChange={(v) => onChange({ ...spec, y_label: opt(v) })} />
              </div></label>
          </>
        )}
      </div>

      {usesGrid(spec.kind) && <SeriesGrid spec={spec} editable={editable} onChange={onChange} />}
      {spec.kind === "gantt" && <TaskList spec={spec} editable={editable} onChange={onChange} />}
      {usesTasks(spec.kind) && <MilestoneList spec={spec} editable={editable} onChange={onChange} />}
      {canStack(spec.kind) && (
        <div className="flex flex-wrap gap-4">
          <Checkbox label="Stack the series" checked={!!spec.stacked} disabled={ro} onChange={(e) => onChange({ ...spec, stacked: e.target.checked, percent: e.target.checked ? spec.percent : false })} />
          <Checkbox label="As percent of each category" checked={!!spec.percent} disabled={ro || !spec.stacked} onChange={(e) => onChange({ ...spec, percent: e.target.checked })} />
        </div>
      )}
      {spec.kind === "combo" && <ComboSeries spec={spec} editable={editable} onChange={onChange} />}
      {spec.kind === "scatter" && <ScatterPoints spec={spec} editable={editable} onChange={onChange} />}
      {!!spec.annotations?.length && <Annotations spec={spec} editable={editable} onChange={onChange} />}
      <SourceEditor source={spec.source} editable={editable} onChange={(source) => onChange({ ...spec, source })} />
    </div>
  );
}

/** Categories down, series across: one row per category, one number per series. Phones: one card per category. */
export function SeriesGrid({ spec: raw, editable, onChange }: { spec: ChartSpec; editable: boolean; onChange: (s: FigureSpec) => void }) {
  const spec = normalizeGrid(raw);
  const cats = spec.categories!;
  const series = spec.series!;
  const ro = !editable;
  return (
    <div className="space-y-1.5" aria-label="Chart data">
      {spec.kind === "heatmap" && <p className="text-xs text-fg-muted">Heatmap: each line here is a column of the drawing, each series a row.</p>}
      <table className={cn("w-full", stack.table)}>
        <thead className={stack.thead}>
          <tr className="text-left text-fg-muted">
            <th className="pb-1 pr-1 font-medium">{spec.kind === "heatmap" ? "Column" : "Category"}</th>
            {series.map((s, j) => (
              <th key={j} className="pb-1 pr-1 font-medium">
                <div className="flex items-center gap-1">
                  <TextCell label={`Series ${j + 1} name`} placeholder={`Series ${j + 1}`} value={s.name} disabled={ro}
                    onChange={(v) => onChange(setSeriesName(spec, j, v))} className="font-semibold" />
                  {editable && series.length > 1 && <IconButton size="sm" icon={Trash2} label={`Remove series ${j + 1}`} onClick={() => onChange(removeSeries(spec, j))} />}
                </div>
              </th>
            ))}
            <th />
          </tr>
        </thead>
        <tbody className={stack.tbody}>
          {cats.map((c, i) => (
            <tr key={i} className={cn("border-t border-line-soft align-top", stack.tr)}>
              <td data-label="Category" className={cn("py-1 pr-1", stack.field)}>
                <TextCell label={`Category ${i + 1}`} value={c} disabled={ro} onChange={(v) => onChange(setCategory(spec, i, v))} />
              </td>
              {series.map((s, j) => (
                <td key={j} data-label={s.name || `Series ${j + 1}`} className={cn("py-1 pr-1", stack.field)}>
                  <NumberCell label={`${c || `Category ${i + 1}`}, ${s.name || `series ${j + 1}`}`} value={s.values[i]} disabled={ro}
                    onChange={(n) => onChange(setValue(spec, i, j, n))} />
                </td>
              ))}
              <td className={editable ? stack.actions : stack.hide}>
                {editable && <IconButton size="sm" icon={Trash2} label={`Remove category ${i + 1}`} onClick={() => onChange(removeCategory(spec, i))} />}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {editable && (
        <div className="flex flex-wrap gap-1.5">
          <Button size="sm" variant="ghost" className="text-fg-muted" onClick={() => onChange(addCategory(spec))}><Plus aria-hidden /> Category</Button>
          <Button size="sm" variant="ghost" className="text-fg-muted" onClick={() => onChange(addSeries(spec))}><Plus aria-hidden /> Series</Button>
          {/* phones: series names sit in the hidden header, so they are edited here */}
          <div className="w-full space-y-1 md:hidden">
            {series.map((s, j) => (
              <div key={j} className="flex items-center gap-1">
                <TextCell label={`Series ${j + 1} name (phone)`} placeholder={`Series ${j + 1} name`} value={s.name} onChange={(v) => onChange(setSeriesName(spec, j, v))} />
                {series.length > 1 && <IconButton size="sm" icon={Trash2} label={`Remove series ${j + 1} (phone)`} onClick={() => onChange(removeSeries(spec, j))} />}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function TaskList({ spec, editable, onChange }: { spec: ChartSpec; editable: boolean; onChange: (s: FigureSpec) => void }) {
  const tasks = spec.tasks ?? [];
  const ro = !editable;
  return (
    <div className="space-y-1.5" aria-label="Timeline tasks">
      <table className={cn("w-full", stack.table)}>
        <thead className={stack.thead}><tr className="text-left text-fg-muted">
          <th className="font-medium">Task</th><th className="w-24 font-medium">Starts at</th><th className="w-24 font-medium">Length</th><th className="font-medium">Phase</th><th />
        </tr></thead>
        <tbody className={stack.tbody}>
          {tasks.map((t, i) => (
            <tr key={i} className={cn("border-t border-line-soft align-top", stack.tr)}>
              <td data-label="Task" className={cn("py-1 pr-1", stack.field)}><TextCell label={`Task ${i + 1}`} value={t.name} disabled={ro} onChange={(v) => onChange(setTask(spec, i, { name: v }))} /></td>
              <td data-label={`Starts at (${spec.time_unit ?? "week"})`} className={cn("py-1 pr-1", stack.field)}>
                <NumberCell label={`Task ${i + 1} start`} value={Number(t.start) || 0} disabled={ro} onChange={(n) => onChange(setTask(spec, i, { start: n }))} /></td>
              <td data-label={`Length (${spec.time_unit ?? "week"}s)`} className={cn("py-1 pr-1", stack.field)}>
                <NumberCell label={`Task ${i + 1} length`} value={Number(t.length) || 0} disabled={ro} onChange={(n) => onChange(setTask(spec, i, { length: n }))} /></td>
              <td data-label="Phase" className={cn("py-1 pr-1", stack.field)}><TextCell label={`Task ${i + 1} phase`} placeholder="optional" value={t.group ?? ""} disabled={ro}
                onChange={(v) => onChange(setTask(spec, i, { group: opt(v) }))} /></td>
              <td className={editable ? stack.actions : stack.hide}>{editable && <IconButton size="sm" icon={Trash2} label={`Remove task ${i + 1}`} onClick={() => onChange(removeTask(spec, i))} />}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {editable && <Button size="sm" variant="ghost" className="text-fg-muted" onClick={() => onChange(addTask(spec))}><Plus aria-hidden /> Task</Button>}
    </div>
  );
}

function MilestoneList({ spec, editable, onChange }: { spec: ChartSpec; editable: boolean; onChange: (s: FigureSpec) => void }) {
  const ms = spec.milestones ?? [];
  const ro = !editable;
  if (!ms.length && !editable) return null;
  return (
    <div className="space-y-1.5" aria-label="Milestones">
      {ms.map((m, i) => (
        <div key={i} className="flex items-center gap-1">
          <TextCell label={`Milestone ${i + 1}`} placeholder="Milestone" value={m.name} disabled={ro} onChange={(v) => onChange(setMilestone(spec, i, { name: v }))} />
          <div className="w-24 shrink-0"><NumberCell label={`Milestone ${i + 1} at`} value={Number(m.at) || 0} disabled={ro} onChange={(n) => onChange(setMilestone(spec, i, { at: n }))} /></div>
          {editable && <IconButton size="sm" icon={Trash2} label={`Remove milestone ${i + 1}`} onClick={() => onChange(removeMilestone(spec, i))} />}
        </div>
      ))}
      {editable && <Button size="sm" variant="ghost" className="text-fg-muted" onClick={() => onChange(addMilestone(spec))}><Plus aria-hidden /> Milestone</Button>}
    </div>
  );
}

/** Combo: each series is drawn as bars or a line, on the left or right axis. */
function ComboSeries({ spec, editable, onChange }: { spec: ChartSpec; editable: boolean; onChange: (s: FigureSpec) => void }) {
  return (
    <div className="space-y-1" aria-label="Series drawing">
      {(spec.series ?? []).map((s, j) => (
        <div key={j} className="flex flex-wrap items-center gap-1">
          <span className="min-w-[8rem] flex-1 font-medium">{s.name || `Series ${j + 1}`}</span>
          <div className="w-32"><Select aria-label={`Series ${j + 1} drawn as`} value={s.type ?? "bar"} disabled={!editable} className={sel}
            onChange={(e) => onChange(setSeries(spec, j, { type: e.target.value as "bar" | "line" }))}
            options={[{ value: "bar", label: "Bars" }, { value: "line", label: "Line" }]} /></div>
          <div className="w-36"><Select aria-label={`Series ${j + 1} axis`} value={s.axis ?? "left"} disabled={!editable} className={sel}
            onChange={(e) => onChange(setSeries(spec, j, { axis: e.target.value as "left" | "right" }))}
            options={[{ value: "left", label: "Left axis" }, { value: "right", label: "Right axis" }]} /></div>
        </div>
      ))}
    </div>
  );
}

/** Scatter: points (x, y, optional label) per series. */
function ScatterPoints({ spec, editable, onChange }: { spec: ChartSpec; editable: boolean; onChange: (s: FigureSpec) => void }) {
  const series = spec.series ?? [];
  const ro = !editable;
  return (
    <div className="space-y-3" aria-label="Points">
      {series.map((s, j) => (
        <div key={j} className="space-y-1.5">
          <div className="flex items-center gap-1">
            <TextCell label={`Series ${j + 1} name`} placeholder={`Series ${j + 1}`} value={s.name} disabled={ro} onChange={(v) => onChange(setSeriesName(spec, j, v))} className="font-semibold" />
            {editable && series.length > 1 && <IconButton size="sm" icon={Trash2} label={`Remove series ${j + 1}`} onClick={() => onChange(removeSeries(spec, j))} />}
          </div>
          <table className={cn("w-full", stack.table)}>
            <thead className={stack.thead}><tr className="text-left text-fg-muted"><th className="w-28 font-medium">X</th><th className="w-28 font-medium">Y</th><th className="font-medium">Label</th><th /></tr></thead>
            <tbody className={stack.tbody}>
              {(s.points ?? []).map((p, i) => (
                <tr key={i} className={cn("border-t border-line-soft align-top", stack.tr)}>
                  <td data-label="X" className={cn("py-1 pr-1", stack.field)}><NumberCell label={`Series ${j + 1} point ${i + 1} x`} value={p.x} disabled={ro} onChange={(n) => onChange(setPoint(spec, j, i, { x: n }))} /></td>
                  <td data-label="Y" className={cn("py-1 pr-1", stack.field)}><NumberCell label={`Series ${j + 1} point ${i + 1} y`} value={p.y} disabled={ro} onChange={(n) => onChange(setPoint(spec, j, i, { y: n }))} /></td>
                  <td data-label="Label" className={cn("py-1 pr-1", stack.field)}><TextCell label={`Series ${j + 1} point ${i + 1} label`} placeholder="optional" value={p.label ?? ""} disabled={ro} onChange={(v) => onChange(setPoint(spec, j, i, { label: opt(v) }))} /></td>
                  <td className={editable ? stack.actions : stack.hide}>{editable && <IconButton size="sm" icon={Trash2} label={`Remove series ${j + 1} point ${i + 1}`} onClick={() => onChange(removePoint(spec, j, i))} />}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {editable && <Button size="sm" variant="ghost" className="text-fg-muted" onClick={() => onChange(addPoint(spec, j))}><Plus aria-hidden /> Point</Button>}
        </div>
      ))}
      {editable && <Button size="sm" variant="ghost" className="text-fg-muted" onClick={() => onChange({ ...spec, series: [...series, { name: "", values: [], points: [{ x: 0, y: 0, label: null }] }] })}><Plus aria-hidden /> Series</Button>}
    </div>
  );
}

function Annotations({ spec, editable, onChange }: { spec: ChartSpec; editable: boolean; onChange: (s: FigureSpec) => void }) {
  const list = spec.annotations ?? [];
  const describe = (a: Record<string, unknown>) =>
    a.type === "ref_line" ? `Reference line at ${a.axis ?? "y"} = ${a.value}${a.label ? `: ${a.label}` : ""}`
      : a.type === "highlight" ? `Highlight ${a.series ?? ""} #${Number(a.index ?? 0) + 1}${a.label ? `: ${a.label}` : ""}`
        : a.type === "note" ? `Note: ${a.text ?? ""}` : String(a.type ?? "annotation");
  return (
    <div className="space-y-1" aria-label="Annotations">
      <p className="text-fg-muted">Annotations</p>
      {list.map((a, i) => (
        <div key={i} className="flex items-center gap-1">
          <span className="flex-1">{describe(a)}</span>
          {editable && <IconButton size="sm" icon={Trash2} label={`Remove annotation ${i + 1}`} onClick={() => onChange({ ...spec, annotations: list.filter((_, j) => j !== i) })} />}
        </div>
      ))}
    </div>
  );
}

/** Where the numbers come from: ERP figures are filled by the server; customer and analysis numbers need a note. */
function SourceEditor({ source, editable, onChange }: { source?: Source; editable: boolean; onChange: (s: Source) => void }) {
  const type = source?.type ?? "customer";
  if (type === "erp") return <p className="text-fg-muted">Numbers from the ERP ({(source as { ref: string }).ref}): filled in when it is drawn.</p>;
  const note = (source as { note?: string } | undefined)?.note ?? "";
  return (
    <div className="grid gap-2 sm:grid-cols-[12rem_1fr]">
      <label className="space-y-1"><span className="text-fg-muted">Numbers from</span>
        <Select aria-label="Numbers from" value={type} disabled={!editable} className={sel}
          onChange={(e) => onChange(e.target.value === "analysis"
            ? { type: "analysis", note, from: (source as { from?: string[] })?.from ?? [] } : { type: "customer", note })}
          options={[{ value: "customer", label: "The customer" }, { value: "analysis", label: "Nary's analysis" }]} /></label>
      <label className="space-y-1"><span className="text-fg-muted">Source note (printed under the figure)</span>
        <TextCell label="Source note" placeholder="e.g. customer's site survey, 12 Oct" value={note} disabled={!editable}
          onChange={(v) => onChange({ ...(source ?? { type: "customer" }), note: v } as Source)} /></label>
    </div>
  );
}

function DiagramPanel({ spec, editable, onChange }: { spec: DiagramSpec; editable: boolean; onChange: (s: FigureSpec) => void }) {
  const ro = !editable;
  const nodeOptions = spec.nodes.map((n) => ({ value: n.id, label: n.label || n.id }));
  return (
    <div className="space-y-3 text-xs">
      <div className="grid gap-2 sm:grid-cols-3">
        <label className="space-y-1"><span className="text-fg-muted">Diagram</span>
          <Select aria-label="Diagram kind" value={spec.kind} disabled={ro} className={sel}
            onChange={(e) => onChange({ ...spec, kind: e.target.value as DiagramSpec["kind"] })} options={DIAGRAM_KINDS} /></label>
        <label className="space-y-1"><span className="text-fg-muted">Title</span>
          <TextCell label="Diagram title" value={spec.title ?? ""} disabled={ro} onChange={(v) => onChange({ ...spec, title: v })} /></label>
        <label className="space-y-1"><span className="text-fg-muted">Direction</span>
          <Select aria-label="Direction" value={spec.direction ?? "LR"} disabled={ro} className={sel}
            onChange={(e) => onChange({ ...spec, direction: e.target.value as "LR" | "TB" })}
            options={[{ value: "LR", label: "Left to right" }, { value: "TB", label: "Top to bottom" }]} /></label>
      </div>

      <fieldset className="space-y-1.5" aria-label="Groups">
        <legend className="mb-1 font-medium text-fg">{spec.kind === "process" ? "Lanes and groups" : "Groups (sites, systems, zones)"}</legend>
        {spec.groups.map((g, i) => (
          <div key={g.id} className="flex flex-wrap items-center gap-1 max-md:rounded-lg max-md:border max-md:border-line max-md:p-2">
            <div className="min-w-[10rem] flex-1"><TextCell label={`Group ${i + 1} name`} placeholder="Name" value={g.label} disabled={ro} onChange={(v) => onChange(setGroup(spec, g.id, { label: v }))} /></div>
            <div className="w-40 max-md:flex-1"><Select aria-label={`Group ${i + 1} kind`} value={g.style} disabled={ro} className={sel}
              onChange={(e) => onChange(setGroup(spec, g.id, { style: e.target.value as DiagramSpec["groups"][number]["style"] }))} options={GROUP_STYLES} /></div>
            {editable && <IconButton size="sm" icon={Trash2} label={`Remove group ${i + 1}`} onClick={() => onChange(removeGroup(spec, g.id))} />}
          </div>
        ))}
        {editable && <Button size="sm" variant="ghost" className="text-fg-muted" onClick={() => onChange(addGroup(spec))}><Plus aria-hidden /> Group</Button>}
      </fieldset>

      <fieldset className="space-y-1.5" aria-label="Items">
        <legend className="mb-1 font-medium text-fg">{spec.kind === "process" ? "Steps" : "Items"}</legend>
        <table className={cn("w-full", stack.table)}>
          <thead className={stack.thead}><tr className="text-left text-fg-muted">
            <th className="font-medium">Label</th><th className="w-44 font-medium">Icon</th><th className="w-32 font-medium">SKU</th>
            <th className="w-36 font-medium">Group</th><th className="w-16 font-medium">Count</th><th />
          </tr></thead>
          <tbody className={stack.tbody}>
            {spec.nodes.map((n, i) => (
              <tr key={n.id} className={cn("border-t border-line-soft align-top", stack.tr)}>
                <td data-label="Label" className={cn("py-1 pr-1", stack.field)}><TextCell label={`Item ${i + 1} label`} value={n.label} disabled={ro} onChange={(v) => onChange(setNode(spec, n.id, { label: v }))} /></td>
                <td data-label="Icon" className={cn("py-1 pr-1", stack.field)}>
                  <Select aria-label={`Item ${i + 1} icon`} value={n.icon} disabled={ro} className={sel} onChange={(e) => onChange(setNode(spec, n.id, { icon: e.target.value }))}>
                    {!ICON_GROUPS.some((g) => g.icons.includes(n.icon)) && <option value={n.icon}>{n.icon}</option>}
                    {ICON_GROUPS.map((g) => (
                      <optgroup key={g.label} label={g.label}>{g.icons.map((ic) => <option key={ic} value={ic}>{iconLabel(ic)}</option>)}</optgroup>
                    ))}
                  </Select>
                </td>
                <td data-label="SKU" className={cn("py-1 pr-1", stack.field)}>
                  <TextCell label={`Item ${i + 1} SKU`} placeholder="optional" value={n.sku ?? ""} disabled={ro}
                    onChange={(v) => onChange(setNode(spec, n.id, { sku: opt(v) }))}
                    className="font-mono" /></td>
                <td data-label="Group" className={cn("py-1 pr-1", stack.field)}>
                  <Select aria-label={`Item ${i + 1} group`} value={n.group ?? ""} disabled={ro} className={sel} placeholder="No group"
                    onChange={(e) => onChange(setNode(spec, n.id, { group: e.target.value || null }))}
                    options={spec.groups.map((g) => ({ value: g.id, label: g.label || g.id }))} /></td>
                <td data-label="Count" className={cn("py-1 pr-1", stack.field)}>
                  <NumberCell label={`Item ${i + 1} count`} value={n.count ?? 1} disabled={ro} onChange={(c) => onChange(setNode(spec, n.id, { count: Math.max(1, Math.round(c)) }))} /></td>
                <td className={editable ? stack.actions : stack.hide}>{editable && <IconButton size="sm" icon={Trash2} label={`Remove item ${i + 1}`} onClick={() => onChange(removeNode(spec, n.id))} />}</td>
              </tr>
            ))}
          </tbody>
        </table>
        {editable && <Button size="sm" variant="ghost" className="text-fg-muted" onClick={() => onChange(addNode(spec))}><Plus aria-hidden /> {spec.kind === "process" ? "Step" : "Item"}</Button>}
      </fieldset>

      <fieldset className="space-y-1.5" aria-label="Links">
        <legend className="mb-1 font-medium text-fg">Links</legend>
        <table className={cn("w-full", stack.table)}>
          <thead className={stack.thead}><tr className="text-left text-fg-muted">
            <th className="font-medium">From</th><th className="font-medium">To</th><th className="w-32 font-medium">Kind</th><th className="font-medium">Label</th><th />
          </tr></thead>
          <tbody className={stack.tbody}>
            {spec.edges.map((e, i) => (
              <tr key={i} className={cn("border-t border-line-soft align-top", stack.tr)}>
                <td data-label="From" className={cn("py-1 pr-1", stack.field)}><Select aria-label={`Link ${i + 1} from`} value={e.from} disabled={ro} className={sel}
                  onChange={(ev) => onChange(setEdge(spec, i, { from: ev.target.value }))} options={nodeOptions} /></td>
                <td data-label="To" className={cn("py-1 pr-1", stack.field)}><Select aria-label={`Link ${i + 1} to`} value={e.to} disabled={ro} className={sel}
                  onChange={(ev) => onChange(setEdge(spec, i, { to: ev.target.value }))} options={nodeOptions} /></td>
                <td data-label="Kind" className={cn("py-1 pr-1", stack.field)}><Select aria-label={`Link ${i + 1} kind`} value={e.kind} disabled={ro} className={sel}
                  onChange={(ev) => onChange(setEdge(spec, i, { kind: ev.target.value as DiagramSpec["edges"][number]["kind"] }))} options={EDGE_KINDS} /></td>
                <td data-label="Label" className={cn("py-1 pr-1", stack.field)}><TextCell label={`Link ${i + 1} label`} placeholder="optional" value={e.label ?? ""} disabled={ro}
                  onChange={(v) => onChange(setEdge(spec, i, { label: opt(v) }))} /></td>
                <td className={editable ? stack.actions : stack.hide}>{editable && <IconButton size="sm" icon={Trash2} label={`Remove link ${i + 1}`} onClick={() => onChange(removeEdge(spec, i))} />}</td>
              </tr>
            ))}
          </tbody>
        </table>
        {editable && spec.nodes.length > 0 && <Button size="sm" variant="ghost" className="text-fg-muted" onClick={() => onChange(addEdge(spec))}><Plus aria-hidden /> Link</Button>}
      </fieldset>
    </div>
  );
}
