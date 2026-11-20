"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { AlertTriangle, BarChart3, Check, ExternalLink, FilePenLine, ImagePlus, Loader2 } from "lucide-react";
import { naryApi, salesDocumentsApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { isStillWorking, LongWorkError, settleCard } from "@/lib/longWork";
import { kindLabel, pngDataUrl, sourceLine, svgDataUrl, type FigureSpec } from "@/lib/proposalVisuals";
import type { NaryActionProposal, ProposalMeta, ResolveProposal } from "./types";

/** visuals V4: the cards for Nary's actions proposal.visual (add, change or remove a figure) and proposal.edit
 *  ("Ask Nary to change" a proposal). The figure is an <img> (a PNG data URL or the stored preview asset);
 *  model SVG is never put in the page. Confirm runs the action as the person, like every Nary action card. */
export const PROPOSAL_CARD_ACTIONS = ["proposal.visual", "proposal.edit"] as const;
export const isProposalCardAction = (a: unknown) => typeof a === "string" && (PROPOSAL_CARD_ACTIONS as readonly string[]).includes(a);

type Bag = Record<string, unknown>;
const str = (v: unknown) => (typeof v === "string" && v.trim() ? v.trim() : null);
const list = (v: unknown) => (Array.isArray(v) ? v.map((x) => (typeof x === "string" ? x : JSON.stringify(x))).filter(Boolean) : []);

/** The fields the card reads: from the card itself first, then from its payload. */
export function readVisualCard(p: Bag) {
  const pay = (p.payload && typeof p.payload === "object" ? p.payload : {}) as Bag;
  const get = (...keys: string[]) => { for (const k of keys) { if (p[k] != null) return p[k]; if (pay[k] != null) return pay[k]; } return undefined; };
  const spec = (get("spec") ?? null) as FigureSpec | null;
  const png = str(get("preview_png", "preview_png_base64", "png_base64", "preview_data_url", "image"));
  const svg = str(get("preview_svg"));
  // an older card's "mode": only an operation name counts
  const legacyOp = str(get("mode"));
  const op = str(get("op", "operation")) ?? (legacyOp && ["add", "change", "remove"].includes(legacyOp) ? legacyOp : null)
    ?? (p.action === "proposal.edit" ? "edit" : "add");
  return {
    edit: p.action === "proposal.edit",
    op: op as "add" | "change" | "remove" | "edit" | string,
    documentId: str(get("document_id")),
    documentLabel: str(get("document_label", "document_number", "proposal_number", "document_title", "proposal_title")),
    imageSrc: png ? pngDataUrl(png) : svg ? svgDataUrl(svg) : null,
    assetId: str(get("preview_png_asset_id", "png_asset_id", "preview_asset_id")),
    caption: str(get("caption")),
    kind: str(get("figure_kind", "kind_label")) ?? (spec?.kind ? kindLabel(spec.kind) : str(pay.kind) ? kindLabel(String(pay.kind)) : null),
    location: str(get("location", "where", "section_title", "section")),
    instruction: str(get("instruction", "text")),
    warnings: list(get("warnings")),
    diff: list(get("diff", "changes")),
    source: sourceLine(spec) ?? (str(get("source", "source_label")) ? `From: ${str(get("source", "source_label"))}` : null),
  };
}

const TITLE: Record<string, string> = {
  add: "Add a figure", change: "Change a figure", remove: "Remove a figure", edit: "Change the proposal",
  add_image: "Add a picture",
};

export default function ProposalVisualCard({ proposal, onResolved }: { proposal: NaryActionProposal & ProposalMeta; onResolved?: ResolveProposal }) {
  const c = readVisualCard(proposal as unknown as Bag);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<null | { label: string; warn: boolean }>(proposal.status === "applied" ? { label: proposal.summary, warn: false } : null);
  const [cancelled, setCancelled] = useState(proposal.status === "discarded");
  const [error, setError] = useState("");
  const [working, setWorking] = useState(false); // fix-longapply
  const href = c.documentId ? `/sales/proposals/${c.documentId}` : null;

  const confirm = async () => {
    setBusy(true); setError("");
    try {
      const res = await naryApi.runAction({ action: proposal.action, payload: proposal.payload, message: proposal.message ?? null });
      // fix-longapply: still running after the wait: it finishes on its own, the card is polled
      const data = await settleCard(res, () => setWorking(true));
      if (data.status === "failed") throw new LongWorkError(String((data as { error?: string | null }).error || "It could not be done."));
      const polled = (data as { result?: { label?: string } | null }).result?.label;
      const label = (isStillWorking(res) ? polled : res.data.label) || proposal.summary;
      setDone({ label, warn: !!res.data.sent_error });
      onResolved?.("applied", { id: res.data.id, label });
    } catch (e) {
      setError(extractApiError(e, "It could not be done."));
    } finally { setBusy(false); setWorking(false); }
  };

  const open = href && (
    <Link href={href} className="inline-flex items-center gap-1 text-xs text-fg-muted hover:underline">
      Open {c.documentLabel || "the proposal"} <ExternalLink size={12} aria-hidden />
    </Link>
  );
  if (cancelled) return <div className="mt-2 text-xs text-fg-subtle italic">Not done.</div>;
  if (done) {
    return (
      <div className={`mt-2 rounded-lg border p-3 text-sm not-prose ${done.warn ? "border-tone-orange/30 bg-tone-orange-soft" : "border-tone-blue/30 bg-tone-blue-soft"}`}>
        <p className={`flex items-center gap-1.5 font-medium ${done.warn ? "text-tone-orange" : "text-tone-blue"}`}>
          {done.warn ? <AlertTriangle size={15} aria-hidden /> : <Check size={15} aria-hidden />} {done.label}
        </p>
        {open && <div className="mt-1.5">{open}</div>}
      </div>
    );
  }
  const Icon = c.edit ? FilePenLine : c.op === "add_image" ? ImagePlus : BarChart3;
  return (
    <div className="mt-2 rounded-lg border border-line bg-surface p-3 not-prose" aria-label={TITLE[c.op] ?? "Proposal figure"}>
      <p className="mb-1 flex items-center gap-1.5 text-xs font-semibold text-fg">
        <Icon size={14} aria-hidden /> {TITLE[c.op] ?? "Proposal figure"}{c.kind && !c.edit && c.op !== "add_image" ? `: ${c.kind}` : ""}
      </p>
      <p className="text-xs text-fg">{proposal.summary}</p>
      {(c.location || c.documentLabel) && (
        <p className="mt-1 text-[11px] text-fg-muted">
          {c.documentLabel && <>In {c.documentLabel}</>}{c.documentLabel && c.location ? ", " : ""}{c.location && <>where: {c.location}</>}
        </p>
      )}
      {c.instruction && c.edit && <p className="mt-1.5 rounded-lg bg-surface-2 p-2 text-xs text-fg whitespace-pre-wrap">{c.instruction}</p>}

      {c.op !== "remove" && (c.imageSrc || (c.assetId && c.documentId)) && (
        <figure className="mt-2 space-y-1">
          <div className="rounded-lg border border-line bg-white p-1.5">
            {c.imageSrc ? <img src={c.imageSrc} alt={c.caption || "Figure preview"} className="mx-auto max-h-72 w-full object-contain" />
              : <AssetPreview documentId={c.documentId!} assetId={c.assetId!} alt={c.caption || "Figure preview"} />}
          </div>
          {(c.caption || c.source) && (
            <figcaption className="text-[11px]">
              {c.caption && <span className="text-fg">{c.caption}</span>}
              {c.source && <span className="block text-fg-muted">{c.source}</span>}
            </figcaption>
          )}
        </figure>
      )}
      {c.op === "remove" && c.caption && <p className="mt-1.5 text-xs text-fg-muted">Figure: {c.caption}</p>}

      {!!(c.diff.length || proposal.details?.length) && (
        <ul className="mt-1.5 space-y-0.5 text-[11px] text-fg-muted whitespace-pre-wrap" aria-label="What changes">
          {(c.diff.length ? c.diff : proposal.details!).map((d, i) => <li key={i}>{d}</li>)}
        </ul>
      )}
      {!!c.warnings.length && (
        <ul className="mt-2 space-y-0.5 rounded-lg border border-tone-orange/30 bg-tone-orange-soft p-2 text-[11px] text-tone-orange" aria-label="Warnings">
          {c.warnings.map((w, i) => <li key={i} className="flex gap-1.5"><AlertTriangle size={12} className="mt-0.5 shrink-0" aria-hidden />{w}</li>)}
        </ul>
      )}
      {working && <p className="mt-1.5 flex items-center gap-1.5 text-[11px] text-fg-muted" role="status"><Loader2 size={12} className="animate-spin" aria-hidden /> Working... this can take a few minutes.</p>}
      {error && <p className="mt-1.5 text-[11px] text-tone-red">{error}</p>}
      <div className="mt-2.5 flex items-center gap-2">
        <button onClick={confirm} disabled={busy}
          className="inline-flex min-h-11 flex-1 items-center justify-center gap-1.5 rounded-lg bg-brand px-3 py-1.5 text-xs font-medium text-brand-on hover:bg-brand-hover disabled:opacity-50 md:min-h-0">
          {busy ? <Loader2 size={13} className="animate-spin" aria-hidden /> : <Check size={13} aria-hidden />} Confirm
        </button>
        <button onClick={() => { setCancelled(true); onResolved?.("discarded"); }} disabled={busy}
          className="min-h-11 rounded-lg border border-line px-3 py-1.5 text-xs text-fg-muted hover:bg-surface-2 md:min-h-0">Cancel</button>
      </div>
      {open && <div className="mt-2">{open}</div>}
    </div>
  );
}

/** The stored preview PNG (it needs the user's token, so it is fetched as a blob). */
function AssetPreview({ documentId, assetId, alt }: { documentId: string; assetId: string; alt: string }) {
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let alive = true;
    let url: string | null = null;
    salesDocumentsApi.asset(documentId, assetId)
      .then((r) => { if (alive) { url = URL.createObjectURL(r.data as Blob); setSrc(url); } })
      .catch(() => { if (alive) setFailed(true); });
    return () => { alive = false; if (url) URL.revokeObjectURL(url); };
  }, [documentId, assetId]);
  if (failed) return <p className="p-3 text-center text-[11px] text-fg-muted">The preview could not be loaded.</p>;
  return src ? <img src={src} alt={alt} className="mx-auto max-h-72 w-full object-contain" />
    : <div className="h-32 w-full animate-pulse rounded-lg bg-surface-2" aria-label="loading figure" />;
}
