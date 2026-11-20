"use client";

import { useState, useEffect, useMemo } from "react";
import { useSession } from "next-auth/react";
import { FileQuestion } from "lucide-react";
import {
  Button, DataTable, Dialog, Field, FilterChips, Input, ListFrame, PageHeader, StatusPill, Textarea, toast, type ColumnDef,
} from "@/components/ui";

interface RFQLine {
  id: string;
  item_name: string;
  description: string | null;
  quantity: number;
  unit: string | null;
}

interface RFQInvite {
  id: string;
  rfq_id: string;
  rfq_number: string;
  title: string;
  buyer_name: string;
  deadline: string | null;
  currency: string;
  status: string;
  notes: string | null;
  created_at: string;
  lines: RFQLine[];
}

// Invite status from models/p2p.py PurchaseRFQVendorInvite
const STATUS_LABEL: Record<string, string> = {
  invited: "Awaiting quote", responded: "Responded", declined: "Declined", no_response: "No response", awarded: "Awarded",
};

interface QuoteLine {
  rfq_line_id: string;
  unit_price: string;
  lead_time_days: string;
}

export default function VendorRFQsPage() {
  const { data: session } = useSession();
  const [rfqs, setRfqs] = useState<RFQInvite[]>([]);
  const [loading, setLoading] = useState(true);
  const [statusFilter, setStatusFilter] = useState("all");
  const [selected, setSelected] = useState<RFQInvite | null>(null);
  const [quoting, setQuoting] = useState(false);
  const [quoteLines, setQuoteLines] = useState<QuoteLine[]>([]);
  const [quoteNotes, setQuoteNotes] = useState("");
  const [submitting, setSubmitting] = useState(false);

  const token = session?.access_token;

  useEffect(() => {
    if (!token) return;
    fetch("/api/v1/vendor/portal/rfqs", { headers: { Authorization: `Bearer ${token}` } })
      .then(r => r.json())
      .then(setRfqs)
      .catch(() => toast.error("Failed to load RFQs"))
      .finally(() => setLoading(false));
  }, [token]);

  function openDetail(rfq: RFQInvite) {
    setSelected(rfq);
    setQuoting(false);
    setQuoteLines(rfq.lines.map(l => ({ rfq_line_id: l.id, unit_price: "", lead_time_days: "" })));
    setQuoteNotes("");
  }

  async function submitQuote(e: React.FormEvent) {
    e.preventDefault();
    if (!token || !selected) return;
    setSubmitting(true);
    try {
      const res = await fetch(`/api/v1/vendor/portal/rfqs/${selected.rfq_id}/respond`, {
        method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
        body: JSON.stringify({
          line_responses: quoteLines.map(l => ({
            rfq_line_id: l.rfq_line_id,
            unit_price: parseFloat(l.unit_price),
            lead_time_days: parseInt(l.lead_time_days) || null,
          })),
          response_notes: quoteNotes,
        }),
      });
      if (!res.ok) throw new Error();
      setRfqs(prev => prev.map(r => r.id === selected.id ? { ...r, status: "responded" } : r));
      setSelected(null);
      toast.success("Quote submitted successfully");
    } catch { toast.error("Failed to submit quote"); }
    finally { setSubmitting(false); }
  }

  function fmt(d: string) { return new Date(d).toLocaleDateString("en-GB", { day: "2-digit", month: "short", year: "numeric" }); }

  const filtered = statusFilter === "all" ? rfqs : rfqs.filter(r => r.status === statusFilter);

  const columns = useMemo<ColumnDef<RFQInvite>[]>(() => [
    { accessorKey: "rfq_number", header: "RFQ", cell: (c) => <span className="font-mono text-[13px]">{c.getValue<string>()}</span> },
    { accessorKey: "title", header: "Title", cell: (c) => <span className="font-medium">{c.getValue<string>()}</span> },
    { id: "items", header: "Items", meta: { numeric: true, hideOnMobile: true }, accessorFn: (r) => r.lines?.length ?? 0,
      cell: (c) => `${c.getValue<number>()} item${c.getValue<number>() !== 1 ? "s" : ""}` },
    { accessorKey: "deadline", header: "Deadline", meta: { numeric: true, hideOnMobile: true },
      cell: ({ row }) => {
        const rfq = row.original;
        if (!rfq.deadline) return <span className="text-fg-subtle">-</span>;
        const late = new Date(rfq.deadline) < new Date() && rfq.status === "invited";
        return <span className={late ? "font-semibold text-tone-red" : undefined}>{fmt(rfq.deadline)}{late ? " (expired)" : ""}</span>;
      } },
    { accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="rfq_invite" status={c.getValue<string>()} label={STATUS_LABEL[c.getValue<string>()]} /> },
    { id: "action", header: "", enableSorting: false,
      cell: ({ row }) => (
        <Button size="sm" variant={row.original.status === "invited" ? "primary" : "secondary"} onClick={() => openDetail(row.original)}>
          {row.original.status === "invited" ? "Quote now" : "View"}
        </Button>
      ) },
  // eslint-disable-next-line react-hooks/exhaustive-deps
  ], []);

  return (
    <div className="space-y-4">
      <ListFrame>
        <PageHeader flush title="Requests for quotation" summary="RFQs where you have been invited to submit a quote." />
        <FilterChips
          chips={["invited", "responded", "declined"].map((s) => ({ id: s, label: STATUS_LABEL[s], active: statusFilter === s }))}
          onToggle={(id) => setStatusFilter((cur) => (cur === id ? "all" : id))}
          onClear={() => setStatusFilter("all")} />
        <DataTable label="Requests for quotation" columns={columns} data={filtered} getRowId={(r) => r.id} loading={loading}
          onRowClick={openDetail}
          empty={{ icon: FileQuestion, title: "No RFQs found", description: "You will be notified when you are invited to quote on a purchase." }} />
      </ListFrame>

      <Dialog open={!!selected} onOpenChange={(o) => { if (!o) setSelected(null); }} size="lg" dismissable={!submitting}
        title={selected?.title ?? "RFQ"}
        description={selected ? <><span className="font-mono">{selected.rfq_number}</span> · From: {selected.buyer_name}</> : undefined}
        footer={selected && (selected.status === "invited" ? <>
          <Button onClick={() => setSelected(null)}>Cancel</Button>
          <Button type="submit" form="rfq-quote-form" variant="primary" loading={submitting}>{submitting ? "Submitting..." : "Submit quote"}</Button>
        </> : <Button onClick={() => setSelected(null)}>Close</Button>)}>
        {selected && (
          <div className="space-y-5">
            <section>
              <h3 className="mb-2 text-sm font-semibold text-fg">Line items</h3>
              <div className="overflow-x-auto rounded-lg border border-line">
                <table className="w-full text-sm">
                  <thead className="bg-surface-2 text-xs text-fg-muted">
                    <tr>
                      <th scope="col" className="px-3 py-2 text-left font-medium">Item</th>
                      <th scope="col" className="px-3 py-2 text-right font-medium">Qty</th>
                      <th scope="col" className="px-3 py-2 text-left font-medium">Unit</th>
                    </tr>
                  </thead>
                  <tbody>
                    {selected.lines?.map(line => (
                      <tr key={line.id} className="border-t border-line-soft">
                        <td className="px-3 py-2">
                          <p className="font-medium text-fg">{line.item_name}</p>
                          {line.description && <p className="text-xs text-fg-muted">{line.description}</p>}
                        </td>
                        <td className="px-3 py-2 text-right tabular">{line.quantity}</td>
                        <td className="px-3 py-2 text-xs text-fg-muted">{line.unit || "pcs"}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </section>

            {selected.notes && (
              <div className="rounded-lg border border-tone-blue/30 bg-tone-blue-soft p-3 text-sm text-tone-blue">{selected.notes}</div>
            )}

            {selected.status === "invited" ? (
              <form id="rfq-quote-form" onSubmit={submitQuote} className="space-y-4">
                <h3 className="text-sm font-semibold text-fg">Your quote</h3>
                <ul className="space-y-3">
                  {selected.lines?.map((line, li) => (
                    <li key={line.id} className="rounded-lg border border-line p-3">
                      <p className="mb-3 text-sm font-medium text-fg">{line.item_name} <span className="font-normal text-fg-muted tabular">x{line.quantity}</span></p>
                      <div className="grid grid-cols-2 gap-3">
                        <Field label={`Unit price (${selected.currency})`} required>
                          <Input required type="number" min="0" step="0.01" numeric
                            value={quoteLines[li]?.unit_price ?? ""}
                            onChange={e => setQuoteLines(prev => prev.map((l, i) => i === li ? { ...l, unit_price: e.target.value } : l))} />
                        </Field>
                        <Field label="Lead time (days)">
                          <Input type="number" min="0" numeric
                            value={quoteLines[li]?.lead_time_days ?? ""}
                            onChange={e => setQuoteLines(prev => prev.map((l, i) => i === li ? { ...l, lead_time_days: e.target.value } : l))} />
                        </Field>
                      </div>
                    </li>
                  ))}
                </ul>
                <Field label="Additional notes">
                  <Textarea rows={2} value={quoteNotes} onChange={e => setQuoteNotes(e.target.value)} placeholder="Warranty, delivery terms, remarks..." />
                </Field>
              </form>
            ) : (
              <div className="py-2 text-center">
                <StatusPill family="rfq_invite" status={selected.status} label={STATUS_LABEL[selected.status] ?? selected.status} />
                <p className="mt-2 text-[13px] text-fg-muted">
                  {selected.status === "responded" ? "Your quote has been submitted and is under review."
                    : selected.status === "awarded" ? "Congratulations, your quote was selected." : "This RFQ has been closed."}
                </p>
              </div>
            )}
          </div>
        )}
      </Dialog>
    </div>
  );
}
