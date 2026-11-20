"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useParams } from "next/navigation";
import { Eye, FileSignature, GitBranch, Loader2, RotateCcw, Save, Send, Sparkles, XCircle } from "lucide-react";
import { salesDocumentsApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { settleJob } from "@/lib/longWork";
import { usePermissions } from "@/hooks/usePermissions";
import ApprovalPanel from "@/components/approvals/ApprovalPanel";
import { useSendDocument } from "@/components/approvals/useSendDocument";
import ProposalEditor, { type Asset, type Block, type FigureActions, type Section } from "@/components/proposals/ProposalEditor";
import CostSheet, { type Sheet } from "@/components/proposals/CostSheet";
import ConfirmPanel from "@/components/proposals/ConfirmPanel";
import {
  Button, Card, CardBody, CardHeader, DataTable, Dialog, Field, Input, PageHeader, Skeleton, StatusPill, Textarea, toast,
  type ColumnDef,
} from "@/components/ui";

interface Quote { id: string; number: string; status: string; currency: string; total: string | null; label: string | null }
interface Doc {
  id: string; full_number: string | null; number: string; title: string; subtitle: string | null; language: string; status: string;
  revision: number; deal_id: string | null; cover: Record<string, unknown>; blocks: Block[]; assets: Asset[]; template: Section[];
  quotations: Quote[];
}
const STATUS: Record<string, string> = { draft: "Draft", review: "In review", approved: "Approved", sent: "Sent", superseded: "Superseded", rejected: "Rejected" };
interface Rejection { reason?: string; by?: string; at?: string }

export default function ProposalPage() {
  const { id } = useParams<{ id: string }>();
  const { can } = usePermissions();
  const [doc, setDoc] = useState<Doc | null>(null);
  const [blocks, setBlocks] = useState<Block[]>([]);
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [ask, setAsk] = useState("");
  const [sheet, setSheet] = useState<Sheet | null>(null);
  const [revising, setRevising] = useState(false);
  const [reason, setReason] = useState("");
  // fix-longapply: long Nary work that outlasts the request (it finishes on its own; the page waits for it)
  const [working, setWorking] = useState(false);
  const [rejecting, setRejecting] = useState(false);
  const [rejectReason, setRejectReason] = useState("");
  const sender = useSendDocument({ entityType: "sales_document", label: "Proposal", onChanged: () => load(),
    onMessage: (k, m) => (k === "success" ? toast.success(m) : k === "info" ? toast.info(m) : toast.error(m)) });

  const loadAsset = useCallback(async (aid: string) => (await salesDocumentsApi.asset(id, aid)).data as Blob, [id]);
  const load = useCallback(async () => {
    const res = await salesDocumentsApi.get(id);
    setDoc(res.data); setBlocks(res.data.blocks); setDirty(false);
    if (can("sales:view_costs")) {
      try { setSheet((await salesDocumentsApi.costSheet(id)).data.sheet); } catch { setSheet(null); }
    }
  }, [id, can]);
  useEffect(() => { load().catch((e) => toast.error(extractApiError(e))); }, [load]);

  const act = async (name: string, fn: () => Promise<unknown>, done?: string) => {
    setBusy(name);
    try { await fn(); if (done) toast.success(done); await load(); } catch (e) { toast.error(extractApiError(e)); } finally { setBusy(null); setWorking(false); }
  };
  const onWorking = () => { setWorking(true); toast.info("Nary is working on it: this can take a few minutes. The proposal refreshes when it is done."); };

  const quoteColumns = useMemo<ColumnDef<Quote>[]>(() => [
    { accessorKey: "number", header: "Quotation",
      cell: (c) => <Link href={`/sales/quotations/${c.row.original.id}`} className="font-mono text-[13px] font-medium text-brand-ink hover:underline">{c.row.original.number}</Link> },
    { accessorKey: "label", header: "Label", cell: (c) => c.getValue<string>() || "-" },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="quotation" status={c.getValue<string>()} /> },
    { accessorKey: "total", header: "Total", meta: { numeric: true },
      cell: (c) => `${c.row.original.currency} ${c.row.original.total ?? "-"}` },
  ], []);

  // visuals V4: draw, check, and ask Nary about one figure (through the same "Ask Nary to change" flow)
  const figures: FigureActions = {
    onPreview: async (body) => (await salesDocumentsApi.visualPreview(id, body)).data,
    onCheck: async (body) => settleJob(id, await salesDocumentsApi.visualCheck(id, body), onWorking),
    onAsk: async (blockId, instruction) => {
      if (dirty) { toast.error("Save your changes first, then ask Nary."); return; }
      const b = blocks.find((x) => x.id === blockId);
      const caption = String((b?.data as { caption?: string } | undefined)?.caption || "").trim();
      await act("ask", async () => settleJob(id, await salesDocumentsApi.ask(id, `Change only the figure${caption ? ` "${caption}"` : ""} (block ${blockId}): ${instruction}`), onWorking),
        "Nary changed the figure: review it.");
    },
  };

  if (!doc) return <div className="space-y-3"><Skeleton className="h-8 w-72" /><Skeleton className="h-40 w-full" /></div>;
  const editable = ["draft", "review"].includes(doc.status) && can("sales:create_quotation");
  const rejection = doc.status === "rejected" ? (doc.cover?.rejection as Rejection | undefined) : undefined;

  return (
    <div className="mx-auto max-w-5xl space-y-4">
      <PageHeader title={doc.title} back={{ href: "/sales/proposals", label: "Proposals" }}
        status={<StatusPill family="proposal" status={doc.status} label={STATUS[doc.status]} />}
        summary={<><span className="font-mono">{doc.full_number || doc.number}</span>{doc.subtitle ? `. ${doc.subtitle}` : ""}</>}
        className="sticky top-0 z-10 -mx-1 bg-canvas px-1 py-2"
        actions={<>
          {!["superseded"].includes(doc.status) && can("sales:create_quotation") && (
            <Button disabled={!!busy} onClick={() => { setReason(""); setRevising(true); }}><GitBranch aria-hidden /> Revise</Button>
          )}
          {editable && (
            <Button disabled={!!busy} onClick={() => { setRejectReason(""); setRejecting(true); }}><XCircle aria-hidden /> Reject</Button>
          )}
          {doc.status === "rejected" && can("sales:create_quotation") && (
            <Button disabled={!!busy} loading={busy === "reopen"}
              onClick={() => act("reopen", () => salesDocumentsApi.reopen(id), "Reopened as a draft.")}><RotateCcw aria-hidden /> Reopen</Button>
          )}
          {["approved", "sent"].includes(doc.status) && (
            <Button onClick={() => sender.start(id)}><Send aria-hidden /> Send with its quotation(s)</Button>
          )}
          <Button disabled={!!busy || dirty} loading={busy === "preview"} onClick={() => act("preview", async () => {
            const res = await salesDocumentsApi.preview(id);
            window.open(URL.createObjectURL(new Blob([res.data], { type: "application/pdf" })), "_blank");
          })}><Eye aria-hidden /> Preview PDF</Button>
          {editable && (
            <Button variant="primary" disabled={!dirty || !!busy} loading={busy === "save"}
              onClick={() => act("save", () => salesDocumentsApi.saveBlocks(id, blocks), "Saved.")}><Save aria-hidden /> Save</Button>
          )}
        </>} />

      {rejection && (
        <Card>
          <CardBody className="p-3 text-sm" role="note" aria-label="Why it was rejected">
            <p className="font-medium text-tone-red">Rejected{rejection.by ? ` by ${rejection.by}` : ""}{rejection.at ? ` on ${new Date(rejection.at).toLocaleDateString("en-GB")}` : ""}</p>
            {rejection.reason && <p className="mt-1 whitespace-pre-wrap text-fg">{rejection.reason}</p>}
            <p className="mt-1 text-xs text-fg-muted">Read-only. Reopen it if this was a mistake, or make a new proposal for the deal.</p>
          </CardBody>
        </Card>
      )}
      {working && (
        <p className="inline-flex items-center gap-1.5 text-sm text-fg-muted" role="status">
          <Loader2 className="size-4 animate-spin" aria-hidden /> Nary is working on it. This can take a few minutes; the proposal refreshes when it is done.
        </p>
      )}
      {editable && (
        <Card>
          <CardBody className="flex flex-wrap items-end gap-2 p-3">
            <Field label="Ask Nary to change" className="min-w-[16rem] flex-1">
              <Input value={ask} onChange={(e) => setAsk(e.target.value)} autoComplete="off"
                placeholder="e.g. make the implementation plan 8 weeks and add a pilot phase" />
            </Field>
            <Button variant="primary" disabled={!ask.trim() || !!busy || dirty}
              onClick={() => act("ask", async () => { await settleJob(id, await salesDocumentsApi.ask(id, ask.trim()), onWorking); setAsk(""); }, "Nary changed it: review the sections.")}>
              <Sparkles aria-hidden /> {busy === "ask" ? "Working..." : "Ask"}
            </Button>
          </CardBody>
        </Card>
      )}

      <Card>
        <CardHeader title={<span className="inline-flex items-center gap-1.5"><FileSignature className="size-4 text-fg-muted" aria-hidden /> Quotations in this proposal</span>} />
        {doc.quotations.length === 0
          ? <CardBody><p className="text-sm text-fg-muted">None yet: confirm the proposal and Nary drafts it from your final prices, or link one.</p></CardBody>
          : <DataTable label="Quotations in this proposal" columns={quoteColumns} data={doc.quotations} getRowId={(q) => q.id} maxHeight="none" />}
      </Card>

      <ProposalEditor sections={doc.template} blocks={blocks} assets={doc.assets} language={doc.language} editable={editable}
        loadAsset={loadAsset}
        onChange={(b) => { setBlocks(b); setDirty(true); }}
        onUpload={async (file) => { const f = new FormData(); f.append("file", file); const r = await salesDocumentsApi.upload(id, f); await load(); return r.data.id; }}
        figures={figures} />
      {sheet && <CostSheet sheet={sheet} busy={busy === "reestimate"}
        onReestimate={() => act("reestimate", async () => { setSheet((await settleJob<{ sheet: Sheet }>(id, await salesDocumentsApi.reestimate(id), onWorking)).sheet); }, "Cost sheet re-estimated")} />}
      {editable && doc.quotations.length === 0 && (
        <ConfirmPanel currency={doc.cover?.currency as string || "USD"} suggested={sheet?.summary.likely.price_at_target ?? null}
          floor={sheet?.summary.likely.price_at_floor ?? null}
          onConfirm={(lines) => act("confirm", () => salesDocumentsApi.confirm(id, lines.map((l) => (l.kind === "catalog"
            ? { product_id: l.product_id, quantity: l.quantity, name: l.name } : { name: l.name, quantity: l.quantity, unit_price: l.unit_price }))),
            "Confirmed. Nary drafted the quotation for your approval (Nary, Approvals).")} />
      )}
      <ApprovalPanel entityType="sales_document" entityId={id} />
      {sender.modal}

      <Dialog open={revising} onOpenChange={setRevising} title="Revise the proposal" description="A new revision opens; the reason is printed on the cover."
        footer={<>
          <Button onClick={() => setRevising(false)}>Cancel</Button>
          <Button variant="primary" disabled={!reason.trim()} onClick={() => {
            setRevising(false);
            act("revise", () => salesDocumentsApi.revise(id, reason.trim()), "A new revision was opened.");
          }}>Open revision</Button>
        </>}>
        <Field label="Why is it revised?"><Textarea value={reason} onChange={(e) => setReason(e.target.value)} autoFocus /></Field>
      </Dialog>

      <Dialog open={rejecting} onOpenChange={setRejecting} title="Reject the proposal"
        description="It becomes read-only and Nary stops working on it; a new proposal can then be made for the deal. You can reopen it."
        footer={<>
          <Button onClick={() => setRejecting(false)}>Cancel</Button>
          <Button variant="danger" disabled={!rejectReason.trim()} onClick={() => {
            setRejecting(false);
            act("reject", () => salesDocumentsApi.reject(id, rejectReason.trim()), "The proposal was rejected.");
          }}>Reject</Button>
        </>}>
        <Field label="Why is it rejected?"><Textarea value={rejectReason} onChange={(e) => setRejectReason(e.target.value)} autoFocus /></Field>
      </Dialog>
    </div>
  );
}
