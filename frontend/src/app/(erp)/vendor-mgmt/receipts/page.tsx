"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useState, useEffect, useCallback, useMemo } from "react";
import { Plus, PackageCheck, Trash2, ArrowRight, Search } from "lucide-react";
import api from "@/lib/api";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, ListFrame, PageHeader, Select, Skeleton,
  StatusPill, Textarea, toast, type ColumnDef,
} from "@/components/ui";

interface GoodsReceipt {
  id: string; receipt_number: string | null; po_id: string; po_number: string | null;
  received_by_id: string; received_date: string | null; status: string;
  location_id: string | null; items_received: ReceiptItem[]; notes: string | null;
  received_at: string;
}
interface ReceiptItem { sku: string | null; po_item_id: string | null; qty_received: number; condition: string; notes: string | null; }
interface PO { id: string; number: string; vendor_id: string; status: string; approval_status?: string; items?: POItem[]; }

// Only these POs can take a goods receipt (p2p.create_receipt): approved (or not needing approval)
// and issued to the vendor, not yet fully received.
const RECEIVABLE_STATUSES = ["sent", "confirmed", "partially_received"];
const RECEIVABLE_APPROVALS = ["approved", "not_required"];
const isReceivable = (p: PO) => RECEIVABLE_STATUSES.includes(p.status) && RECEIVABLE_APPROVALS.includes(p.approval_status ?? "");
interface POItem { id: string; component_sku: string | null; description: string; quantity_ordered: number; quantity_received: number; unit: string; }
interface Location { id: string; full_path: string; }

const EMPTY_FORM = { po_id: "", received_date: "", location_id: "", notes: "" };

function fmt(d: string | null) {
  if (!d) return "-";
  return new Date(d).toLocaleDateString("en-GB", { day: "2-digit", month: "short", year: "numeric" });
}

export default function ReceiptsPage() {
  const [receipts, setReceipts] = useState<GoodsReceipt[]>([]);
  const [loading, setLoading] = useState(true);
  const [pos, setPos] = useState<PO[]>([]);
  const [locations, setLocations] = useState<Location[]>([]);
  const [search, setSearch] = useState("");

  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState(EMPTY_FORM);
  const [, setSelectedPO] = useState<PO | null>(null);
  const [poLoading, setPoLoading] = useState(false);
  const [receiptItems, setReceiptItems] = useState<{ po_item_id: string; sku: string; description: string; qty_ordered: number; qty_received_prev: number; qty_receiving: string }[]>([]);
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const { rows } = await fetchAllPages<GoodsReceipt>((p) => api.get("/vendor/goods-receipts", { params: p }));
      setReceipts(rows);
    } catch { setReceipts([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  useEffect(() => {
    fetchAllPages<PO>((p) => api.get("/vendor/purchase-orders", { params: p })).then(({ rows }) => setPos(rows)).catch(() => {});
    api.get("/inventory/locations").then(r => setLocations(r.data ?? [])).catch(() => {});
  }, []);

  async function loadPODetail(poId: string) {
    setPoLoading(true);
    try {
      const res = await api.get(`/vendor/purchase-orders/${poId}`);
      const po = res.data;
      setSelectedPO(po);
      setReceiptItems((po.items ?? []).map((item: POItem) => ({
        po_item_id: item.id,
        sku: item.component_sku ?? "",
        description: item.description,
        qty_ordered: item.quantity_ordered,
        qty_received_prev: item.quantity_received ?? 0,
        qty_receiving: String(Math.max(0, item.quantity_ordered - (item.quantity_received ?? 0))),
      })));
    } catch { setSelectedPO(null); } finally { setPoLoading(false); }
  }

  async function save() {
    if (!form.po_id) return toast.error("Select a purchase order");
    const items = receiptItems.filter(i => parseFloat(i.qty_receiving) > 0).map(i => ({
      sku: i.sku || null,
      po_item_id: i.po_item_id,
      qty_received: parseFloat(i.qty_receiving),
    }));
    if (items.length === 0) return toast.error("Enter quantity received for at least one item");
    setSaving(true);
    try {
      await api.post("/vendor/goods-receipts", {
        po_id: form.po_id,
        received_date: form.received_date || null,
        location_id: form.location_id || null,
        notes: form.notes || null,
        items,
      });
      toast.success("Goods receipt recorded");
      setShowModal(false);
      setForm(EMPTY_FORM);
      setSelectedPO(null);
      setReceiptItems([]);
      load();
    } catch (err: unknown) {
      const msg = (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      toast.error(msg ?? "Failed to record receipt");
    } finally { setSaving(false); }
  }

  const q = search.trim().toLowerCase();
  const rows = receipts.filter((gr) => !q ||
    (gr.receipt_number ?? "").toLowerCase().includes(q) || (gr.po_number ?? "").toLowerCase().includes(q));

  const columns = useMemo<ColumnDef<GoodsReceipt>[]>(() => [
    {
      accessorKey: "receipt_number", header: "Receipt",
      cell: (c) => <span className="font-mono text-[13px] font-medium">{c.getValue<string | null>() ?? "-"}</span>,
    },
    {
      id: "po", accessorFn: (gr) => gr.po_number ?? gr.po_id.slice(0, 8), header: "PO",
      cell: (c) => <span className="font-mono text-[13px] text-fg-muted">{c.getValue<string>()}</span>,
    },
    {
      id: "date", accessorFn: (gr) => gr.received_date ?? gr.received_at, header: "Date",
      cell: (c) => <span className="text-fg-muted">{fmt(c.getValue<string | null>())}</span>,
    },
    {
      id: "items", accessorFn: (gr) => (gr.items_received ?? []).length, header: "Items", meta: { numeric: true },
      cell: (c) => `${c.getValue<number>()} line${c.getValue<number>() !== 1 ? "s" : ""}`,
    },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="receipt" status={c.getValue<string>()} /> },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const gr = c.row.original;
        return confirmDeleteId === gr.id ? (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            <span className="text-xs font-semibold text-tone-red">Delete?</span>
            <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
          </div>
        ) : (
          <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${gr.receipt_number ?? "receipt"}`} onClick={() => setConfirmDeleteId(gr.id)} />
        );
      },
    },
  ], [confirmDeleteId]);

  return (
    <>
      <ListFrame>
        <PageHeader flush title="Goods receipts"
          summary={`${receipts.length} receipt${receipts.length !== 1 ? "s" : ""}. Record items received against purchase orders.`}
          actions={
            <Button variant="primary" onClick={() => { setForm(EMPTY_FORM); setSelectedPO(null); setReceiptItems([]); setShowModal(true); }}>
              <Plus aria-hidden /> Record receipt
            </Button>
          } />
        <FilterChips chips={[]} onToggle={() => {}}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search receipts" aria-label="Search receipts" className="w-56" />
        </FilterChips>
        <DataTable label="Goods receipts" columns={columns} data={rows} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: PackageCheck, title: "No receipts recorded yet", description: "Record a receipt when goods arrive for a purchase order." }}
          renderDetails={(gr) => (
            <div className="space-y-2">
              {(gr.items_received ?? []).length === 0
                ? <p className="text-sm text-fg-muted">No lines on this receipt.</p>
                : (
                  <ul className="space-y-1 text-sm">
                    {gr.items_received.map((it, i) => (
                      <li key={i} className="flex gap-3">
                        <span className="font-mono text-[13px] text-fg-muted">{it.sku ?? "-"}</span>
                        <span className="tabular">{it.qty_received}</span>
                        {it.condition && <span className="text-fg-muted">{it.condition}</span>}
                      </li>
                    ))}
                  </ul>
                )}
              {gr.notes && <p className="text-sm text-fg-muted">{gr.notes}</p>}
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Record goods receipt" size="lg"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={save} loading={saving}>{saving ? "Saving" : "Record receipt"}</Button>
        </>}>
        <div className="space-y-4">
          <Field label="Purchase order" required>
            <Select value={form.po_id} placeholder="Select PO"
              onChange={e => { setForm({ ...form, po_id: e.target.value }); if (e.target.value) loadPODetail(e.target.value); }}
              options={pos.filter(isReceivable).map(p => ({ value: p.id, label: `${p.number} · ${p.status}` }))} />
          </Field>
          <FieldGrid cols={2}>
            <Field label="Receipt date">
              <Input type="date" value={form.received_date} onChange={e => setForm({ ...form, received_date: e.target.value })} />
            </Field>
            <Field label="Store to location">
              <Select value={form.location_id} onChange={e => setForm({ ...form, location_id: e.target.value })}
                options={[{ value: "", label: "No location" }, ...locations.map(l => ({ value: l.id, label: l.full_path }))]} />
            </Field>
          </FieldGrid>

          {poLoading ? (
            <div className="space-y-2"><Skeleton className="h-12" /><Skeleton className="h-12" /></div>
          ) : receiptItems.length > 0 && (
            <div>
              <p className="mb-2 text-sm font-medium text-fg">Items to receive</p>
              <div className="space-y-2">
                {receiptItems.map((item, i) => (
                  <div key={item.po_item_id} className="flex items-center gap-3 rounded-lg bg-surface-2 px-4 py-3">
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-sm font-medium text-fg">{item.description}</p>
                      <p className="text-xs text-fg-muted tabular">{item.sku ? `${item.sku} · ` : ""}Ordered: {item.qty_ordered} · Prev received: {item.qty_received_prev}</p>
                    </div>
                    <div className="flex shrink-0 items-center gap-1.5">
                      <ArrowRight className="size-3.5 text-fg-subtle" aria-hidden />
                      <Input numeric type="number" min="0" max={item.qty_ordered} step="0.001" className="w-24"
                        aria-label={`Quantity receiving for ${item.description}`}
                        value={item.qty_receiving}
                        onChange={e => { const n = [...receiptItems]; n[i] = { ...n[i], qty_receiving: e.target.value }; setReceiptItems(n); }} />
                    </div>
                  </div>
                ))}
              </div>
            </div>
          )}

          <Field label="Notes">
            <Textarea value={form.notes} onChange={e => setForm({ ...form, notes: e.target.value })} rows={2} />
          </Field>
        </div>
      </Dialog>
    </>
  );
}
