"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { ArrowDown, ArrowUp, BarChart3, ImagePlus, Lock, Network, Plus, Shapes, Trash2 } from "lucide-react";
import { Button, Card, CardBody, CardHeader, IconButton, Input, Select, Textarea, controlClass } from "@/components/ui";
import { cn } from "@/lib/utils";
import { stack } from "@/lib/stackTable";
import { emptyChart, emptyDiagram, type FigureData } from "@/lib/proposalVisuals";
import FigureBlock, { type VisualCall } from "./FigureBlock";

/** The Solution Proposal editor (Nary Phase 4): sections from the template, each with its blocks. The
 *  commercial section is generated from the linked quotations and the stamp from the approval, so
 *  neither can be typed. Everything else is edited here, or by asking Nary. */

export type BlockType = "text" | "kv" | "table" | "coverage" | "image" | "chart" | "diagram" | "commercial" | "stamp";
export interface Block { id?: string; section: string; block_type: BlockType; data: Record<string, unknown> }
/** A template section. The title is English text; an older stored {en} object is still read. */
export interface Section { key: string; title: string | { en?: string }; blocks?: string[] }
/** The section's title as text (its key when the template has none). */
export const sectionTitle = (s: Section): string =>
  (typeof s.title === "string" ? s.title : s.title?.en) || s.key;
export interface Asset {
  id: string; kind: string; caption: string | null; status: string;
  /** The source file name of a picture Nary placed. */
  filename?: string | null;
}

export const COVERAGE = ["included", "optional_custom", "not_in_scope", "to_confirm"] as const;
const COVERAGE_LABEL: Record<string, string> = {
  included: "Included", optional_custom: "Optional, Custom AI", not_in_scope: "Not in scope", to_confirm: "To confirm",
};
const ADDABLE: [BlockType, string][] = [["text", "Text"], ["kv", "Key-value"], ["table", "Table"], ["coverage", "Coverage"]];
// visuals V4: figures (charts, diagrams) and pictures
type FigureChoice = "chart" | "diagram" | "image";
const FIGURES: [FigureChoice, string, React.ElementType][] = [
  ["chart", "Chart", BarChart3], ["diagram", "Diagram", Network], ["image", "Image", ImagePlus],
];

const figure = (spec: FigureData["spec"]): FigureData => ({ spec, caption: "", size: "full", svg_asset_id: null, png_asset_id: null, checked: null });

function empty(t: BlockType, language = "en"): Record<string, unknown> {
  return { text: { md: "" }, kv: { rows: [["", ""]] }, table: { columns: ["", ""], rows: [["", ""]] },
           coverage: { rows: [{ requirement: "", status: "included", notes: "" }] }, image: { asset_id: null, caption: "" },
           chart: figure(emptyChart(language)), diagram: figure(emptyDiagram(language)),
           commercial: {}, stamp: {} }[t] as Record<string, unknown>;
}

/** What the editor needs to draw and check figures and to ask Nary about one (visuals V4). */
export interface FigureActions { onPreview?: VisualCall; onCheck?: VisualCall; onAsk?: (blockId: string, instruction: string) => Promise<void> | void }

export default function ProposalEditor({ sections, blocks, assets, language, editable, loadAsset, onChange, onUpload, figures }: {
  sections: Section[]; blocks: Block[]; assets: Asset[]; language: string; editable: boolean;
  loadAsset: (id: string) => Promise<Blob>; onChange: (blocks: Block[]) => void;
  onUpload?: (file: File) => Promise<string | null>;
  figures?: FigureActions;
}) {
  const [figureMenu, setFigureMenu] = useState<string | null>(null);
  const bySection = useMemo(() => {
    const m: Record<string, { block: Block; index: number }[]> = {};
    blocks.forEach((b, index) => { (m[b.section] ||= []).push({ block: b, index }); });
    return m;
  }, [blocks]);

  const update = (index: number, data: Record<string, unknown>) =>
    onChange(blocks.map((b, i) => (i === index ? { ...b, data } : b)));
  const remove = (index: number) => onChange(blocks.filter((_, i) => i !== index));
  const move = (index: number, dir: -1 | 1) => {
    const j = index + dir;
    if (j < 0 || j >= blocks.length || blocks[j].section !== blocks[index].section) return;
    const next = [...blocks];
    [next[index], next[j]] = [next[j], next[index]];
    onChange(next);
  };
  const add = (section: string, t: BlockType, data?: Record<string, unknown>) => {
    const order = sections.map((s) => s.key);
    const at = blocks.reduce((last, b, i) => (order.indexOf(b.section) <= order.indexOf(section) ? i + 1 : last), 0);
    onChange([...blocks.slice(0, at), { section, block_type: t, data: data ?? empty(t, language) }, ...blocks.slice(at)]);
  };

  return (
    <div className="space-y-5">
      {sections.map((s, n) => (
        <Card key={s.key} aria-label={sectionTitle(s)}>
          <CardHeader title={`${n + 1}. ${sectionTitle(s)}`} />
          <CardBody className="space-y-3">
          {(bySection[s.key] || []).map(({ block, index }) => (
            <div key={block.id ?? `${s.key}-${index}`} className="group relative rounded-lg border border-line-soft p-3">
              {editable && !["commercial", "stamp"].includes(block.block_type) && (
                <div className="absolute right-2 top-2 z-[1] flex gap-1 rounded-lg bg-surface [@media(hover:hover)]:opacity-0 [@media(hover:hover)]:group-hover:opacity-100 [@media(hover:hover)]:focus-within:opacity-100 [@media(hover:hover)]:focus-visible:opacity-100 max-md:static max-md:-mt-1 max-md:mb-2 max-md:justify-end max-md:!opacity-100">
                  <IconButton size="sm" icon={ArrowUp} label="Move up" onClick={() => move(index, -1)} />
                  <IconButton size="sm" icon={ArrowDown} label="Move down" onClick={() => move(index, 1)} />
                  <IconButton size="sm" icon={Trash2} label="Remove block" onClick={() => remove(index)} />
                </div>
              )}
              {block.block_type === "chart" || block.block_type === "diagram" ? (
                <FigureBlock blockId={block.id} data={block.data as FigureData} editable={editable} loadAsset={loadAsset}
                  onChange={(d) => update(index, d)} onPreview={figures?.onPreview} onCheck={figures?.onCheck}
                  onAsk={figures?.onAsk && block.id ? (text) => figures.onAsk!(block.id!, text) : undefined} />
              ) : (
                <BlockEditor block={block} editable={editable} assets={assets} loadAsset={loadAsset}
                  onChange={(d) => update(index, d)} onUpload={onUpload} />
              )}
            </div>
          ))}
          {editable && (
            <div className="flex flex-wrap gap-1.5">
              {ADDABLE.map(([t, label]) => (
                <Button key={t} size="sm" variant="ghost" onClick={() => add(s.key, t)} aria-label={`Add ${label} to ${sectionTitle(s)}`}
                  className="border border-dashed border-line-strong text-fg-muted">
                  <Plus aria-hidden /> {label}
                </Button>
              ))}
              <Button size="sm" variant="ghost" aria-expanded={figureMenu === s.key} aria-label={`Add figure to ${sectionTitle(s)}`}
                onClick={() => setFigureMenu(figureMenu === s.key ? null : s.key)} className="border border-dashed border-line-strong text-fg-muted">
                <Shapes aria-hidden /> Add figure
              </Button>
              {figureMenu === s.key && (
                <div className="flex w-full flex-wrap gap-1.5 rounded-lg bg-surface-2 p-1.5" role="group" aria-label="Figure kinds">
                  {FIGURES.map(([t, label, Icon]) => (
                    <Button key={t} size="sm" onClick={() => {
                      setFigureMenu(null);
                      add(s.key, t);
                    }} aria-label={`Add ${label} to ${sectionTitle(s)}`}>
                      <Icon aria-hidden /> {label}
                    </Button>
                  ))}
                </div>
              )}
            </div>
          )}
          </CardBody>
        </Card>
      ))}
    </div>
  );
}

function BlockEditor({ block, editable, assets, loadAsset, onChange, onUpload }: {
  block: Block; editable: boolean; assets: Asset[]; loadAsset: (id: string) => Promise<Blob>;
  onChange: (d: Record<string, unknown>) => void;
  onUpload?: (file: File) => Promise<string | null>;
}) {
  const d = block.data as Record<string, unknown>;
  const [preview, setPreview] = useState(false);
  if (block.block_type === "commercial") {
    return <p className="inline-flex items-center gap-1.5 text-xs text-fg-muted"><Lock className="size-3.5" aria-hidden /> Commercial summary: generated from the linked quotation(s) when the PDF is made. It can&apos;t be typed, so the numbers always match.</p>;
  }
  if (block.block_type === "stamp") {
    return <p className="inline-flex items-center gap-1.5 text-xs text-fg-muted"><Lock className="size-3.5" aria-hidden /> Approval stamp and QR: added once the proposal is approved.</p>;
  }
  if (block.block_type === "text") {
    const md = String(d.md ?? "");
    return (
      <div className="space-y-1.5">
        {editable && <Button size="sm" variant="ghost" className="text-fg-muted" onClick={() => setPreview(!preview)}>{preview ? "Edit" : "Preview"}</Button>}
        {editable && !preview ? (
          <Textarea aria-label="Text (Markdown)" value={md} onChange={(e) => onChange({ ...d, md: e.target.value })} rows={Math.min(18, Math.max(4, md.split("\n").length + 1))}
            className="font-mono" />
        ) : (
          <div className="prose prose-sm max-w-none text-fg dark:prose-invert"><ReactMarkdown remarkPlugins={[remarkGfm]}>{md || "*(empty)*"}</ReactMarkdown></div>
        )}
      </div>
    );
  }
  if (block.block_type === "kv") {
    const rows = (d.rows as string[][]) || [];
    return <Grid rows={rows} columns={["Label", "Value"]} editable={editable} onChange={(r) => onChange({ ...d, rows: r })} />;
  }
  if (block.block_type === "table") {
    const cols = (d.columns as string[]) || [];
    const rows = (d.rows as string[][]) || [];
    return (
      <div className="space-y-1.5">
        {editable && (
          <div className="flex flex-wrap items-center gap-1.5">
            {cols.map((c, i) => (
              <input key={i} aria-label={`Column ${i + 1}`} value={c} placeholder={`Column ${i + 1}`} autoComplete="off"
                onChange={(e) => onChange({ ...d, columns: cols.map((x, j) => (j === i ? e.target.value : x)) })}
                className={cn(cell, "min-w-[8rem] flex-1 font-semibold")} />
            ))}
            <IconButton size="sm" icon={Plus} label="Add column" onClick={() => onChange({ ...d, columns: [...cols, ""], rows: rows.map((r) => [...r, ""]) })} />
          </div>
        )}
        <Grid rows={rows} columns={cols} editable={editable} onChange={(r) => onChange({ ...d, rows: r })} />
      </div>
    );
  }
  if (block.block_type === "coverage") {
    const rows = (d.rows as { requirement: string; status: string; notes: string }[]) || [];
    const set = (i: number, k: string, v: string) => onChange({ ...d, rows: rows.map((r, j) => (j === i ? { ...r, [k]: v } : r)) });
    return (
      <table className={cn("w-full text-xs", stack.table)}>
        <thead className={stack.thead}><tr className="text-left text-fg-muted"><th className="font-medium">Requirement</th><th className="w-44 font-medium">Status</th><th className="font-medium">Notes</th><th /></tr></thead>
        <tbody className={stack.tbody}>
          {rows.map((r, i) => (
            <tr key={i} className={cn("border-t border-line-soft align-top", stack.tr)}>
              <td data-label="Requirement" className={cn("py-1 pr-1", editable ? stack.field : stack.title)}>{editable ? <input aria-label={`Requirement ${i + 1}`} value={r.requirement} autoComplete="off" onChange={(e) => set(i, "requirement", e.target.value)} className={cn(cell, "w-full")} /> : r.requirement}</td>
              <td data-label="Status" className={cn("py-1 pr-1", editable ? stack.field : stack.td)}>{editable ? (
                <Select aria-label={`Status ${i + 1}`} value={r.status} onChange={(e) => set(i, "status", e.target.value)} className="h-11 max-md:text-base md:h-7 md:text-xs"
                  options={COVERAGE.map((c) => ({ value: c, label: COVERAGE_LABEL[c] }))} />) : COVERAGE_LABEL[r.status] ?? r.status}</td>
              <td data-label="Notes" className={cn("py-1 pr-1", editable ? stack.field : stack.td)}>{editable ? <input aria-label={`Notes ${i + 1}`} value={r.notes} autoComplete="off" onChange={(e) => set(i, "notes", e.target.value)} className={cn(cell, "w-full")} /> : r.notes}</td>
              <td className={editable ? stack.actions : stack.hide}>{editable && <IconButton size="sm" icon={Trash2} label={`Remove requirement ${i + 1}`} onClick={() => onChange({ ...d, rows: rows.filter((_, j) => j !== i) })} />}</td>
            </tr>
          ))}
          {editable && <tr className="max-md:block"><td colSpan={4} className="max-md:block"><Button size="sm" variant="ghost" className="text-fg-muted" onClick={() => onChange({ ...d, rows: [...rows, { requirement: "", status: "included", notes: "" }] })}><Plus aria-hidden /> Requirement</Button></td></tr>}
        </tbody>
      </table>
    );
  }
  return <ImageBlock d={d} editable={editable} assets={assets} loadAsset={loadAsset} onChange={onChange} onUpload={onUpload} />;
}

/** A compact cell input for the block tables. */
const cell = cn(controlClass, "h-11 px-2 max-md:text-base md:h-7 md:text-xs");

function Grid({ rows, columns, editable, onChange }: { rows: string[][]; columns: string[]; editable: boolean; onChange: (r: string[][]) => void }) {
  const width = Math.max(columns.length, ...rows.map((r) => r.length), 1);
  // Phones: one card per row when the row would not fit (an editable row, or more than two columns).
  const st = editable || width > 2 ? stack : { table: "", thead: "", tbody: "", tr: "", td: "", field: "", actions: "" };
  return (
    <table className={cn("w-full text-xs", st.table)}>
      {columns.some(Boolean) && !editable && <thead className={st.thead}><tr>{columns.map((c, i) => <th key={i} className="text-left font-medium text-fg-muted">{c}</th>)}</tr></thead>}
      <tbody className={st.tbody}>
        {rows.map((r, i) => (
          <tr key={i} className={cn("border-t border-line-soft", st.tr)}>
            {Array.from({ length: width }).map((_, j) => (
              <td key={j} data-label={columns[j] || `Column ${j + 1}`} className={cn("py-1 pr-1", editable ? st.field : st.td)}>{editable ? (
                <input aria-label={`Row ${i + 1} column ${j + 1}`} value={r[j] ?? ""} autoComplete="off"
                  onChange={(e) => onChange(rows.map((row, k) => (k === i ? Array.from({ length: width }, (_, m) => (m === j ? e.target.value : row[m] ?? "")) : row)))}
                  className={cn(cell, "w-full")} />) : r[j]}</td>
            ))}
            {editable && <td className={st.actions}><IconButton size="sm" icon={Trash2} label={`Remove row ${i + 1}`} onClick={() => onChange(rows.filter((_, k) => k !== i))} /></td>}
          </tr>
        ))}
        {editable && <tr className="max-md:block"><td colSpan={width + 1} className="max-md:block"><Button size="sm" variant="ghost" className="text-fg-muted" onClick={() => onChange([...rows, Array(width).fill("")])}><Plus aria-hidden /> Row</Button></td></tr>}
      </tbody>
    </table>
  );
}

function ImageBlock({ d, editable, assets, loadAsset, onChange, onUpload }: {
  d: Record<string, unknown>; editable: boolean; assets: Asset[]; loadAsset: (id: string) => Promise<Blob>;
  onChange: (d: Record<string, unknown>) => void;
  onUpload?: (file: File) => Promise<string | null>;
}) {
  const fileRef = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const id = d.asset_id as string | null;
  const pickable = assets.filter((a) => a.status === "ready" && !["figure_svg", "figure_png", "figure_preview"].includes(a.kind));
  const asset = id ? assets.find((a) => a.id === id) : undefined;
  const run = async (fn: () => Promise<string | null>) => {
    setBusy(true);
    try { const newId = await fn(); if (newId) onChange({ ...d, asset_id: newId }); } finally { setBusy(false); }
  };
  return (
    <div className="space-y-2">
      {(asset?.kind === "nary_image" || d.size === "half") && (
        <div className="flex flex-wrap items-center gap-1.5 text-xs text-fg-muted">
          {asset?.kind === "nary_image" && <span className="rounded-full bg-surface-2 px-2 py-0.5 font-medium text-fg">Picture placed by Nary{asset.filename ? `: ${asset.filename}` : ""}</span>}
          {d.size === "half" && <span>Half width</span>}
        </div>
      )}
      {id ? <AuthImage id={id} load={loadAsset} alt={String(d.caption || "picture")} />
        : <p className="text-xs text-fg-muted">No image yet.</p>}
      {editable && (
        <>
          <div className="grid gap-2 sm:grid-cols-[1fr_10rem]">
            <Input aria-label="Caption" value={String(d.caption ?? "")} placeholder="Caption" autoComplete="off" onChange={(e) => onChange({ ...d, caption: e.target.value })} />
            <Select aria-label="Image size" value={d.size === "half" ? "half" : "full"} onChange={(e) => onChange({ ...d, size: e.target.value })}
              options={[{ value: "full", label: "Full width" }, { value: "half", label: "Half width" }]} />
          </div>
          <div className="flex flex-wrap gap-1.5 items-center">
            {pickable.length > 0 && (
              <Select aria-label="Pick an image" value={id ?? ""} onChange={(e) => onChange({ ...d, asset_id: e.target.value || null })}
                placeholder="Pick an image..." className="w-56" options={pickable.map((a) => ({ value: a.id, label: a.caption || a.kind }))} />
            )}
            {onUpload && (
              <>
                <input ref={fileRef} type="file" accept="image/png,image/jpeg,image/webp" className="hidden" aria-label="Upload an image"
                  onChange={(e) => { const f = e.target.files?.[0]; if (f) run(() => onUpload(f)); e.target.value = ""; }} />
                <Button size="sm" disabled={busy} onClick={() => fileRef.current?.click()}><ImagePlus aria-hidden /> Upload</Button>
              </>
            )}
          </div>
        </>
      )}
    </div>
  );
}

/** An image from the API (it needs the user's token, so it is fetched as a blob). */
function AuthImage({ id, load, alt }: { id: string; load: (id: string) => Promise<Blob>; alt: string }) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    let url: string | null = null;
    load(id).then((b) => { if (alive) { url = URL.createObjectURL(b); setSrc(url); } }).catch(() => setSrc(null));
    return () => { alive = false; if (url) URL.revokeObjectURL(url); };
  }, [id, load]);
  return src ? <img src={src} alt={alt} className="max-h-72 rounded-lg border border-line" /> : <div className="h-24 w-40 animate-pulse rounded-lg bg-surface-2" aria-label="loading image" />;
}
