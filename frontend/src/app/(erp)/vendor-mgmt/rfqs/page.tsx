"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useState, useEffect, useCallback, useMemo } from "react";
import { Plus, FileText, Trash2, UserPlus, X, CheckCircle2, Search } from "lucide-react";
import { vendorSource } from "@/lib/entitySources";
import api from "@/lib/api";
import { useCurrencySettings, currencyOptions } from "@/hooks/useCurrencySettings";
import {
  Button, Checkbox, Combobox, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, ListFrame,
  PageHeader, Pill, SavedViewTabs, Select, Skeleton, StatusPill, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface Vendor { id: string; name: string; vendor_type: string; }
interface RFQLine { id: string; component_sku: string | null; description: string; quantity: number; unit: string; responses: RFQResponse[]; }
interface RFQResponse { id: string; vendor_id: string; vendor_name: string | null; unit_price: number; currency: string; lead_time_days: number | null; is_selected: boolean; }
interface VendorInvite { id: string; vendor_id: string; vendor_name: string | null; status: string; }
interface RFQ { id: string; number: string; title: string; status: string; deadline: string | null; currency: string; line_count: number; vendor_count: number; lines?: RFQLine[]; vendor_invites?: VendorInvite[]; }

const EMPTY_FORM = { title: "", currency: "USD", deadline: "", notes: "" };

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "draft", label: "Drafts", filters: { status: "draft" } },
  { id: "sent", label: "Sent", filters: { status: "sent" } },
  { id: "evaluating", label: "Evaluating", filters: { status: "evaluating" } },
  { id: "closed", label: "Closed", filters: { status: "closed" } },
];

const INVITE_TONE: Record<string, "blue" | "red" | "neutral"> = { responded: "blue", declined: "red" };

export default function RFQsPage() {
  const views = useSavedViews<Filters>("vendor.rfqs", VIEWS);
  const [search, setSearch] = useState("");
  const [rfqs, setRfqs] = useState<RFQ[]>([]);
  const [loading, setLoading] = useState(true);
  const [active, setActive] = useState<RFQ | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [, setVendors] = useState<Vendor[]>([]);

  const [showCreate, setShowCreate] = useState(false);
  const [form, setForm] = useState(EMPTY_FORM);
  const { functional: functionalCurrency, active: activeCurrencies } = useCurrencySettings();
  const [newLines, setNewLines] = useState<{ description: string; quantity: string; unit: string; component_sku: string }[]>([]);
  const [saving, setSaving] = useState(false);

  const [inviteVendorId, setInviteVendorId] = useState("");
  const [inviting, setInviting] = useState(false);

  const [showCloseModal, setShowCloseModal] = useState(false);
  const [closeVendorId, setCloseVendorId] = useState("");
  const [selectedResponses, setSelectedResponses] = useState<Set<string>>(new Set());
  const [closing, setClosing] = useState(false);

  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const { rows } = await fetchAllPages<RFQ>((p) => api.get("/vendor/rfqs", { params: p }));
      setRfqs(rows);
    } catch { setRfqs([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  useEffect(() => {
    fetchAllPages<Vendor>((p) => api.get("/vendor/vendors", { params: p })).then(({ rows }) => setVendors(rows)).catch(() => {});
  }, []);

  async function openDetail(rfq: RFQ) {
    setDetailLoading(true);
    setActive(rfq);
    try {
      const res = await api.get(`/vendor/rfqs/${rfq.id}`);
      setActive(res.data);
    } catch { } finally { setDetailLoading(false); }
  }

  function addLine() {
    setNewLines([...newLines, { description: "", quantity: "1", unit: "pcs", component_sku: "" }]);
  }

  function removeLine(i: number) {
    setNewLines(newLines.filter((_, idx) => idx !== i));
  }

  function updateLine(i: number, field: keyof (typeof newLines)[number], value: string) {
    const n = [...newLines];
    n[i] = { ...n[i], [field]: value };
    setNewLines(n);
  }

  async function createRFQ() {
    if (!form.title.trim()) return toast.error("Title is required");
    if (newLines.length === 0) return toast.error("Add at least one line item");
    setSaving(true);
    try {
      await api.post("/vendor/rfqs", {
        title: form.title.trim(),
        currency: form.currency,
        deadline: form.deadline || null,
        notes: form.notes || null,
        lines: newLines.filter(l => l.description.trim()).map(l => ({
          description: l.description.trim(),
          quantity: parseFloat(l.quantity) || 1,
          unit: l.unit || "pcs",
          component_sku: l.component_sku.trim() || null,
        })),
      });
      toast.success("RFQ created");
      setShowCreate(false);
      setForm(EMPTY_FORM);
      setNewLines([]);
      load();
    } catch { toast.error("Failed to create RFQ"); } finally { setSaving(false); }
  }

  async function inviteVendor() {
    if (!active || !inviteVendorId) return;
    setInviting(true);
    try {
      await api.post(`/vendor/rfqs/${active.id}/invite`, { vendor_id: inviteVendorId });
      toast.success("Vendor invited");
      setInviteVendorId("");
      openDetail(active);
    } catch (err: unknown) {
      const msg = (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      toast.error(msg ?? "Failed to invite vendor");
    } finally { setInviting(false); }
  }

  async function deleteRFQ(id: string) {
    try {
      await api.delete(`/vendor/rfqs/${id}`);
      toast.success("RFQ deleted");
      setConfirmDeleteId(null);
      if (active?.id === id) setActive(null);
      load();
    } catch (err: unknown) {
      const msg = (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      toast.error(msg ?? "Cannot delete this RFQ");
    }
  }

  async function closeRFQ() {
    if (!active || !closeVendorId) return toast.error("Select a vendor to award the PO to");
    setClosing(true);
    try {
      const res = await api.post(`/vendor/rfqs/${active.id}/close`, {
        create_po: true,
        vendor_id: closeVendorId,
        selected_line_responses: Array.from(selectedResponses),
        currency: active.currency,
      });
      toast.success(`RFQ closed${res.data.purchase_order_id ? ", PO created" : ""}`);
      setShowCloseModal(false);
      setSelectedResponses(new Set());
      load();
      if (active) openDetail({ ...active, id: active.id });
    } catch { toast.error("Failed to close RFQ"); } finally { setClosing(false); }
  }

  const status = views.active?.filters.status;
  const q = search.trim().toLowerCase();
  const rows = rfqs.filter((r) => (!status || r.status === status) &&
    (!q || r.title.toLowerCase().includes(q) || r.number.toLowerCase().includes(q)));

  const columns = useMemo<ColumnDef<RFQ>[]>(() => [
    {
      accessorKey: "title", header: "RFQ",
      cell: (c) => (
        <div className="min-w-0">
          <p className="truncate font-medium text-fg">{c.row.original.title}</p>
          <p className="font-mono text-xs text-fg-muted">{c.row.original.number}</p>
        </div>
      ),
    },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="rfq" status={c.getValue<string>()} /> },
    { accessorKey: "deadline", header: "Deadline", meta: { hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{c.getValue<string | null>() ?? "-"}</span> },
    { accessorKey: "line_count", header: "Lines", meta: { numeric: true, hideOnMobile: true } },
    { accessorKey: "vendor_count", header: "Vendors", meta: { numeric: true } },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const rfq = c.row.original;
        return confirmDeleteId === rfq.id ? (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            <span className="text-xs font-semibold text-tone-red">Delete?</span>
            <Button size="sm" variant="danger" onClick={() => deleteRFQ(rfq.id)}>Yes</Button>
            <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
          </div>
        ) : (
          <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${rfq.number}`} onClick={() => setConfirmDeleteId(rfq.id)} />
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId, active?.id]);

  function renderDetail(rfq: RFQ) {
    if (detailLoading || active?.id !== rfq.id) {
      return <div className="space-y-2"><Skeleton className="h-16" /><Skeleton className="h-16" /></div>;
    }
    const a = active;
    return (
      <div className="space-y-5">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <p className="text-[13px] text-fg-muted tabular">{a.number} · {a.currency}{a.deadline ? ` · Due ${a.deadline}` : ""}</p>
          {a.status === "evaluating" && (
            <Button size="sm" variant="primary" onClick={() => { setShowCloseModal(true); setCloseVendorId(""); setSelectedResponses(new Set()); }}>
              <CheckCircle2 aria-hidden /> Close and create PO
            </Button>
          )}
        </div>

        <section>
          <h3 className="mb-2 text-sm font-semibold text-fg">Line items and vendor responses</h3>
          <div className="space-y-3">
            {(a.lines ?? []).map((line) => (
              <div key={line.id} className="rounded-lg border border-line bg-surface p-3">
                <div className="mb-2 flex items-center gap-3">
                  <span className="rounded-md border border-line px-2 py-0.5 font-mono text-xs text-fg-muted">{line.component_sku ?? "-"}</span>
                  <p className="flex-1 text-sm font-medium text-fg">{line.description}</p>
                  <span className="text-xs text-fg-muted tabular">{line.quantity} {line.unit}</span>
                </div>
                {line.responses.length === 0 ? (
                  <p className="text-xs italic text-fg-subtle">No vendor responses yet</p>
                ) : (
                  <div className="flex flex-wrap gap-2">
                    {line.responses.map((r) => (
                      <div key={r.id} className={r.is_selected
                        ? "min-w-[140px] flex-1 rounded-lg border border-tone-blue/40 bg-tone-blue-soft p-3"
                        : "min-w-[140px] flex-1 rounded-lg border border-line bg-surface-2 p-3"}>
                        <p className="text-xs font-medium text-fg">{r.vendor_name}</p>
                        <p className="mt-0.5 text-base font-semibold text-fg tabular">{r.currency} {r.unit_price.toLocaleString()}</p>
                        {r.lead_time_days && <p className="text-xs text-fg-muted">{r.lead_time_days}d lead</p>}
                        {r.is_selected && <span className="inline-flex items-center gap-1 text-xs font-semibold text-tone-blue"><CheckCircle2 className="size-3" aria-hidden /> Selected</span>}
                      </div>
                    ))}
                  </div>
                )}
              </div>
            ))}
          </div>
        </section>

        <section>
          <h3 className="mb-2 text-sm font-semibold text-fg">Invite vendor</h3>
          <div className="flex gap-2">
            <div className="flex-1">
              <Combobox value={inviteVendorId} source={vendorSource} aria-label="Vendor to invite"
                placeholder="Select vendor" emptyLabel="No vendors found"
                onChange={(id) => setInviteVendorId(id)} />
            </div>
            <Button onClick={inviteVendor} loading={inviting} disabled={!inviteVendorId}>
              <UserPlus aria-hidden /> Invite
            </Button>
          </div>
          {(a.vendor_invites ?? []).length > 0 && (
            <div className="mt-3 flex flex-wrap gap-2">
              {(a.vendor_invites ?? []).map(inv => (
                <Pill key={inv.id} tone={INVITE_TONE[inv.status] ?? "neutral"}>{inv.vendor_name} · {inv.status}</Pill>
              ))}
            </div>
          )}
        </section>
      </div>
    );
  }

  return (
    <>
      <ListFrame>
        <PageHeader flush title="RFQs" summary={`${rfqs.length} request${rfqs.length !== 1 ? "s" : ""} for quotation`}
          actions={
            <Button variant="primary" onClick={() => { setForm({ ...EMPTY_FORM, currency: functionalCurrency }); setNewLines([{ description: "", quantity: "1", unit: "pcs", component_sku: "" }]); setShowCreate(true); }}>
              <Plus aria-hidden /> New RFQ
            </Button>
          } />
        <SavedViewTabs state={views} currentFilters={views.active?.filters ?? {}} />
        <FilterChips chips={[]} onToggle={() => {}}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search RFQs" aria-label="Search RFQs" className="w-56" />
        </FilterChips>
        <DataTable label="RFQs" columns={columns} data={rows} getRowId={(r) => r.id} loading={loading}
          expandedId={active?.id ?? null}
          onExpandedChange={(id) => {
            const rfq = id ? rfqs.find((r) => r.id === id) : undefined;
            if (rfq) openDetail(rfq); else setActive(null);
          }}
          empty={{ icon: FileText, title: "No RFQs yet", description: "Create an RFQ to collect vendor prices." }}
          renderDetails={renderDetail} />
      </ListFrame>

      <Dialog open={showCreate} onOpenChange={setShowCreate} title="New request for quotation" size="lg"
        footer={<>
          <Button onClick={() => setShowCreate(false)}>Cancel</Button>
          <Button variant="primary" onClick={createRFQ} loading={saving}>{saving ? "Creating" : "Create RFQ"}</Button>
        </>}>
        <div className="space-y-4">
          <Field label="Title" required>
            <Input value={form.title} onChange={e => setForm({ ...form, title: e.target.value })} placeholder="e.g. Office furniture Q2 2026" />
          </Field>
          <FieldGrid cols={2}>
            <Field label="Currency">
              <Select value={form.currency} onChange={e => setForm({ ...form, currency: e.target.value })}
                options={currencyOptions(activeCurrencies, form.currency)} />
            </Field>
            <Field label="Response deadline">
              <Input type="date" value={form.deadline} onChange={e => setForm({ ...form, deadline: e.target.value })} />
            </Field>
          </FieldGrid>
          <div>
            <div className="mb-2 flex items-center justify-between">
              <span className="text-sm font-medium text-fg">Line items <span className="text-brand-ink">*</span></span>
              <Button size="sm" variant="ghost" onClick={addLine}><Plus aria-hidden /> Add line</Button>
            </div>
            <div className="space-y-2">
              {newLines.map((line, i) => (
                <div key={i} className="grid grid-cols-[96px_1fr_72px_64px_32px] items-center gap-2 max-md:grid-cols-[1fr_1fr_auto] max-md:rounded-lg max-md:border max-md:border-line max-md:p-3">
                  <Input value={line.component_sku} onChange={e => updateLine(i, "component_sku", e.target.value)}
                    className="font-mono max-md:col-span-3" placeholder="SKU" aria-label={`Line ${i + 1} SKU`} />
                  <Input value={line.description} onChange={e => updateLine(i, "description", e.target.value)}
                    placeholder="Description" aria-label={`Line ${i + 1} description`} className="max-md:col-span-3" />
                  <div>
                    <span className="mb-1 block text-xs text-fg-muted md:hidden" aria-hidden>Qty</span>
                    <Input numeric type="number" min="0.001" value={line.quantity} onChange={e => updateLine(i, "quantity", e.target.value)}
                      placeholder="Qty" aria-label={`Line ${i + 1} quantity`} />
                  </div>
                  <div>
                    <span className="mb-1 block text-xs text-fg-muted md:hidden" aria-hidden>Unit</span>
                    <Input value={line.unit} onChange={e => updateLine(i, "unit", e.target.value)} placeholder="Unit" aria-label={`Line ${i + 1} unit`} />
                  </div>
                  <IconButton size="sm" variant="ghost" icon={X} label={`Remove line ${i + 1}`} onClick={() => removeLine(i)} className="max-md:self-end" />
                </div>
              ))}
            </div>
          </div>
        </div>
      </Dialog>

      <Dialog open={showCloseModal && !!active} onOpenChange={setShowCloseModal} title="Close RFQ and create PO"
        description="Select the winning vendor and mark which price responses to use."
        footer={<>
          <Button onClick={() => setShowCloseModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={closeRFQ} loading={closing} disabled={!closeVendorId}>{closing ? "Processing" : "Create PO"}</Button>
        </>}>
        {active && (
          <div className="space-y-4">
            <Field label="Award to" required>
              <Select value={closeVendorId} onChange={e => setCloseVendorId(e.target.value)} placeholder="Select vendor"
                options={(active.vendor_invites ?? []).filter(v => v.status === "responded").map(v => ({ value: v.vendor_id, label: v.vendor_name ?? v.vendor_id }))} />
            </Field>
            <div>
              <p className="mb-2 text-sm font-medium text-fg">Select responses for PO lines</p>
              {(active.lines ?? []).map(line => {
                const vendorResps = line.responses.filter(r => r.vendor_id === closeVendorId);
                if (!vendorResps.length) return null;
                return (
                  <div key={line.id} className="mb-3 rounded-lg bg-surface-2 p-3">
                    <p className="mb-2 text-xs font-medium text-fg">{line.description} ({line.quantity} {line.unit})</p>
                    {vendorResps.map(resp => (
                      <Checkbox key={resp.id} checked={selectedResponses.has(resp.id)}
                        onChange={e => { const s = new Set(selectedResponses); e.target.checked ? s.add(resp.id) : s.delete(resp.id); setSelectedResponses(s); }}
                        label={<span className="tabular">{active.currency} {resp.unit_price.toLocaleString()} / {line.unit}{resp.lead_time_days ? `, ${resp.lead_time_days}d lead` : ""}</span>} />
                    ))}
                  </div>
                );
              })}
            </div>
          </div>
        )}
      </Dialog>
    </>
  );
}
