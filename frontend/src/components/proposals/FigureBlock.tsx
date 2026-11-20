"use client";

import { useEffect, useState } from "react";
import { AlertTriangle, CheckCircle2, Eye, RefreshCw, ScanEye, SlidersHorizontal, Sparkles } from "lucide-react";
import { Button, Input, Select } from "@/components/ui";
import { cn } from "@/lib/utils";
import { extractApiError } from "@/lib/apiError";
import FigureDataPanel from "./FigureDataPanel";
import { isCustomSvg, kindLabel, pngDataUrl, sourceLine, svgDataUrl, type FigureData, type FigureSpec } from "@/lib/proposalVisuals";

/** What /visuals/preview and /visuals/check answer (fields read leniently). */
export interface VisualResult { svg?: string; png_base64?: string; warnings?: string[]; spec?: FigureSpec; ok?: boolean; notes?: string[] }
export type VisualCall = (body: { spec?: FigureSpec; brief?: string; block_id?: string; caption?: string; size?: "full" | "half" }) => Promise<VisualResult>;

/** A chart or diagram block: the stored drawing (an <img>, never inline SVG), its caption and size, the data
 *  behind it, Preview / Redraw / Check, and "Ask Nary to change this figure". */
export default function FigureBlock({ blockId, data, editable, loadAsset, onChange, onPreview, onCheck, onAsk }: {
  blockId?: string; data: FigureData; editable: boolean; loadAsset: (id: string) => Promise<Blob>;
  onChange: (d: FigureData) => void; onPreview?: VisualCall; onCheck?: VisualCall; onAsk?: (instruction: string) => Promise<void> | void;
}) {
  const spec = data.spec;
  const [local, setLocal] = useState<{ src: string; warnings: string[] } | null>(null);
  const [stale, setStale] = useState(false);
  const [showData, setShowData] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [ask, setAsk] = useState("");
  const custom = isCustomSvg(spec);

  const edit = (d: FigureData, specChanged = false) => { if (specChanged) setStale(true); onChange(d); };
  const run = async (name: string, fn: () => Promise<void>) => {
    setBusy(name); setError("");
    try { await fn(); } catch (e) { setError(extractApiError(e, "It could not be drawn.")); } finally { setBusy(null); }
  };
  const show = (r: VisualResult) => {
    const src = r.svg ? svgDataUrl(r.svg) : r.png_base64 ? pngDataUrl(r.png_base64) : null;
    if (src) setLocal({ src, warnings: r.warnings ?? [] });
    setStale(false);
  };
  const preview = () => run("preview", async () => {
    if (!onPreview) return;
    const r = await onPreview({ spec, size: data.size ?? "full", block_id: blockId });
    show(r);
  });
  // Redraw: from the brief when there is one (Nary writes a fresh spec), else the same spec drawn again.
  const redraw = () => run("redraw", async () => {
    if (!onPreview) return;
    const size = data.size ?? "full";
    const r = await onPreview(data.brief ? { brief: data.brief, size, block_id: blockId } : { spec, size, block_id: blockId });
    show(r);
    // a fresh spec Nary wrote from the brief replaces the old one (Save keeps it)
    if (data.brief && r.spec) onChange({ ...data, spec: r.spec });
  });
  const check = () => run("check", async () => {
    if (!onCheck) return;
    const r = await onCheck({ spec, brief: data.brief, block_id: blockId, caption: data.caption });
    onChange({ ...data, checked: { ok: !!r.ok, notes: r.notes ?? r.warnings ?? [], at: new Date().toISOString() } });
  });

  const warnings = local ? local.warnings : data.checked?.notes ?? [];
  const source = sourceLine(spec);
  return (
    <figure className="space-y-2" aria-label={`${kindLabel(spec.kind)} figure`}>
      <div className="flex flex-wrap items-center gap-1.5 text-xs text-fg-muted">
        <span className="rounded-full bg-surface-2 px-2 py-0.5 font-medium text-fg">{kindLabel(spec.kind)}</span>
        {data.size === "half" && <span>Half width</span>}
        {data.checked && !local && (data.checked.ok
          ? <span className="inline-flex items-center gap-1 text-tone-blue"><CheckCircle2 className="size-3.5" aria-hidden /> Checked</span>
          : <span className="inline-flex items-center gap-1 text-tone-orange"><AlertTriangle className="size-3.5" aria-hidden /> Needs a look</span>)}
      </div>

      <div className={cn("rounded-lg border border-line bg-white p-2", data.size === "half" ? "md:max-w-[50%]" : "")}>
        {local ? <img src={local.src} alt={data.caption || spec.title || kindLabel(spec.kind)} className="mx-auto max-h-[28rem] w-full object-contain" />
          : data.svg_asset_id ? <AssetImage id={data.svg_asset_id} load={loadAsset} alt={data.caption || spec.title || kindLabel(spec.kind)} />
            : <p className="p-4 text-center text-xs text-fg-muted">Not drawn yet: Preview shows it, Save keeps it.</p>}
      </div>
      {stale && editable && <p className="text-xs text-tone-orange">Changed since it was drawn: Preview to see it; Save draws it for the PDF.</p>}

      <figcaption className="space-y-0.5 text-xs">
        {!editable && data.caption && <p className="text-fg">{data.caption}</p>}
        {source && <p className="text-fg-muted">{source}</p>}
      </figcaption>

      {!!warnings.length && (
        <ul className="space-y-0.5 rounded-lg border border-tone-orange/30 bg-tone-orange-soft p-2 text-xs text-tone-orange" aria-label="Figure warnings">
          {warnings.map((w, i) => <li key={i} className="flex gap-1.5"><AlertTriangle className="mt-0.5 size-3.5 shrink-0" aria-hidden />{w}</li>)}
        </ul>
      )}
      {error && <p className="text-xs text-tone-red">{error}</p>}

      {editable && (
        <>
          <div className="grid gap-2 sm:grid-cols-[1fr_10rem]">
            <Input aria-label="Figure caption" value={String(data.caption ?? "")} placeholder="Caption" autoComplete="off"
              onChange={(e) => edit({ ...data, caption: e.target.value })} />
            <Select aria-label="Figure size" value={data.size ?? "full"} onChange={(e) => edit({ ...data, size: e.target.value as "full" | "half" })}
              options={[{ value: "full", label: "Full width" }, { value: "half", label: "Half width" }]} />
          </div>
          <div className="flex flex-wrap gap-1.5">
            <Button size="sm" onClick={() => setShowData(!showData)} aria-expanded={showData}><SlidersHorizontal aria-hidden /> {showData ? "Hide data" : custom ? "About this drawing" : "Edit data"}</Button>
            {onPreview && !custom && <Button size="sm" loading={busy === "preview"} disabled={!!busy} onClick={preview}><Eye aria-hidden /> Preview</Button>}
            {onPreview && <Button size="sm" loading={busy === "redraw"} disabled={!!busy} onClick={redraw}><RefreshCw aria-hidden /> Redraw</Button>}
            {onCheck && <Button size="sm" loading={busy === "check"} disabled={!!busy} onClick={check}><ScanEye aria-hidden /> Check</Button>}
          </div>
          {showData && (
            <div className="rounded-lg border border-line-soft p-2">
              <FigureDataPanel spec={spec} editable={editable} onChange={(s) => edit({ ...data, spec: s }, true)} />
            </div>
          )}
          {onAsk && (
            <div className="flex flex-wrap gap-1.5">
              <Input aria-label="Ask Nary to change this figure" value={ask} onChange={(e) => setAsk(e.target.value)} autoComplete="off"
                placeholder={blockId ? "Ask Nary to change this figure: e.g. add a pilot week before rollout" : "Save first, then ask Nary to change it"}
                className="min-w-[14rem] flex-1" disabled={!blockId} />
              <Button variant="primary" disabled={!blockId || !ask.trim() || !!busy} loading={busy === "ask"}
                onClick={() => run("ask", async () => { await onAsk(ask.trim()); setAsk(""); })}>
                <Sparkles aria-hidden /> Ask Nary
              </Button>
            </div>
          )}
        </>
      )}
    </figure>
  );
}

/** A stored figure from the API (it needs the user's token): fetched as a blob, shown as an <img>. */
function AssetImage({ id, load, alt }: { id: string; load: (id: string) => Promise<Blob>; alt: string }) {
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let alive = true;
    let url: string | null = null;
    load(id).then((b) => {
      if (!alive) return;
      // an SVG served without its type would not draw in an <img>
      const blob = b.type ? b : new Blob([b], { type: "image/svg+xml" });
      url = URL.createObjectURL(blob); setSrc(url);
    }).catch(() => { if (alive) setFailed(true); });
    return () => { alive = false; if (url) URL.revokeObjectURL(url); };
  }, [id, load]);
  if (failed) return <p className="p-4 text-center text-xs text-fg-muted">The drawing could not be loaded.</p>;
  return src ? <img src={src} alt={alt} className="mx-auto max-h-[28rem] w-full object-contain" />
    : <div className="h-40 w-full animate-pulse rounded-lg bg-surface-2" aria-label="loading figure" />;
}
