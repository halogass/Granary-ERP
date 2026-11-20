"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useState, useEffect, useCallback, useMemo } from "react";
import { localSource } from "@/lib/entitySources";
import { Plus, Package, Trash2, TrendingDown, XCircle, Search, RefreshCw, Send } from "lucide-react";
import api from "@/lib/api";
import { useCurrencySettings, currencyOptions } from "@/hooks/useCurrencySettings";
import {
  Button, Combobox, DataTable, Dialog, Field, FieldGrid, FilterChips, Input, KeyValues, ListFrame, PageHeader,
  SavedViewTabs, Select, Sheet, StatusPill, Textarea, Card, CardHeader, toast, useSavedViews,
  type ColumnDef, type SavedView,
} from "@/components/ui";
import { extractApiError } from "@/lib/apiError";
import BankAccountSelect from "@/components/finance/BankAccountSelect";
import JournalLink from "@/components/finance/JournalLink";

interface AssetCategory {
  id: string;
  name: string;
  depreciation_method: string;
  useful_life_months: number | null;
}

interface FixedAsset {
  id: string;
  number: string;
  name: string;
  category_id: string | null;
  category_name: string | null;
  status: string;
  purchase_date: string;
  purchase_price: number;
  currency: string;
  useful_life_months: number | null;
  salvage_value: number;
  depreciation_method: string | null;
  location: string | null;
  serial_number: string | null;
  notes: string | null;
}

interface DepreciationLine {
  id: string | null;
  period_label: string;
  period_date: string;
  depreciation_amount: number;
  accumulated_depreciation: number;
  book_value: number;
  db_status: string;
  db_line_id: string | null;
}

interface Disposal {
  id: string;
  disposal_date: string;
  disposal_type: string;
  proceeds: number;
  disposal_notes: string | null;
}

const STATUS_LABELS: Record<string, string> = {
  active: "Active",
  disposed: "Disposed",
  fully_depreciated: "Fully depreciated",
};

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "active", label: "Active", filters: { status: "active" } },
  { id: "fully_depreciated", label: "Fully depreciated", filters: { status: "fully_depreciated" } },
  { id: "disposed", label: "Disposed", filters: { status: "disposed" } },
];

const EMPTY_FORM = {
  name: "", description: "", category_id: "", purchase_date: "", purchase_price: "",
  currency: "USD", useful_life_months: "", salvage_value: "0",
  depreciation_method: "", location: "", serial_number: "", notes: "",
};

export default function AssetsPage() {
  const [assets, setAssets] = useState<FixedAsset[]>([]);
  const [categories, setCategories] = useState<AssetCategory[]>([]);
  const [loading, setLoading] = useState(true);
  const views = useSavedViews<Filters>("finance.assets", VIEWS);
  const statusFilter = views.active?.filters.status ?? "";
  const [search, setSearch] = useState("");
  const [catChip, setCatChip] = useState<string | null>(null);
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState(EMPTY_FORM);
  const { functional: functionalCurrency, active: activeCurrencies } = useCurrencySettings();
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  // Schedule drawer
  const [scheduleAsset, setScheduleAsset] = useState<FixedAsset | null>(null);
  const [schedule, setSchedule] = useState<DepreciationLine[]>([]);
  const [disposals, setDisposals] = useState<Disposal[]>([]);
  const [scheduleLoading, setScheduleLoading] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [posting, setPosting] = useState(false);

  // Disposal modal
  const [disposeAsset, setDisposeAsset] = useState<FixedAsset | null>(null);
  const [disposeForm, setDisposeForm] = useState({ disposal_date: "", disposal_type: "sold", proceeds: "0", disposal_notes: "", bank_account_id: "" });
  const [disposingSave, setDisposingSave] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const params: Record<string, string> = {};
      if (statusFilter) params.status = statusFilter;
      if (search) params.q = search;
      const [assetsRes, catsRes] = await Promise.all([
        fetchAllPages<FixedAsset>((p) => api.get("/finance/assets", { params: p }), params),
        api.get("/finance/categories"),
      ]);
      setAssets(assetsRes.rows);
      setCategories(catsRes.data ?? []);
    } catch {
      setAssets([]);
    } finally {
      setLoading(false);
    }
  }, [statusFilter, search]);

  useEffect(() => { load(); }, [load]);

  const categoryOptions = useMemo(
    () => localSource(categories.map((c) => ({ id: c.id, label: c.name }))),
    [categories],
  );

  async function createAsset() {
    if (!form.name.trim() || !form.purchase_date || !form.purchase_price) {
      return toast.error("Name, purchase date and price are required");
    }
    setSaving(true);
    try {
      await api.post("/finance/assets", {
        name: form.name.trim(),
        description: form.description || null,
        category_id: form.category_id || null,
        purchase_date: form.purchase_date,
        purchase_price: parseFloat(form.purchase_price),
        currency: form.currency,
        useful_life_months: form.useful_life_months ? parseInt(form.useful_life_months) : null,
        salvage_value: parseFloat(form.salvage_value) || 0,
        depreciation_method: form.depreciation_method || null,
        location: form.location || null,
        serial_number: form.serial_number || null,
        notes: form.notes || null,
      });
      toast.success("Asset created");
      setShowModal(false);
      setForm(EMPTY_FORM);
      load();
    } catch {
      toast.error("Failed to create asset");
    } finally {
      setSaving(false);
    }
  }

  async function deleteAsset(id: string) {
    try {
      await api.delete(`/finance/assets/${id}`);
      toast.success("Asset deleted");
      setConfirmDeleteId(null);
      load();
    } catch {
      toast.error("Failed to delete asset");
    }
  }

  async function openSchedule(asset: FixedAsset) {
    setScheduleAsset(asset);
    setScheduleLoading(true);
    try {
      const [res, detail] = await Promise.all([
        api.get(`/finance/assets/${asset.id}/depreciation-schedule`),
        api.get(`/finance/assets/${asset.id}`),
      ]);
      setSchedule(res.data.schedule ?? []);
      setDisposals(detail.data.disposals ?? []);
    } catch {
      setSchedule([]);
      setDisposals([]);
    } finally {
      setScheduleLoading(false);
    }
  }

  async function generateSchedule() {
    if (!scheduleAsset) return;
    setGenerating(true);
    try {
      const res = await api.post(`/finance/assets/${scheduleAsset.id}/generate-schedule`);
      toast.success(`Generated ${res.data.created} depreciation periods`);
      openSchedule(scheduleAsset);
    } catch {
      toast.error("Failed to generate schedule");
    } finally {
      setGenerating(false);
    }
  }

  async function postNextDepreciation() {
    if (!scheduleAsset) return;
    setPosting(true);
    try {
      const res = await api.post(`/finance/assets/${scheduleAsset.id}/post-depreciation`);
      toast.success(`Posted ${res.data.posted_period}: ${scheduleAsset.currency} ${res.data.depreciation_amount.toLocaleString()}`);
      openSchedule(scheduleAsset);
      load();
    } catch (err: unknown) {
      const msg = (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      toast.error(msg ?? "Failed to post depreciation");
    } finally {
      setPosting(false);
    }
  }

  async function submitDisposal() {
    if (!disposeAsset || !disposeForm.disposal_date) return toast.error("Disposal date is required");
    setDisposingSave(true);
    try {
      await api.post(`/finance/assets/${disposeAsset.id}/dispose`, {
        disposal_date: disposeForm.disposal_date,
        disposal_type: disposeForm.disposal_type,
        proceeds: parseFloat(disposeForm.proceeds) || 0,
        disposal_notes: disposeForm.disposal_notes || null,
        bank_account_id: disposeForm.bank_account_id || null,
      });
      toast.success("Asset disposed");
      setDisposeAsset(null);
      load();
    } catch (e) {
      toast.error(extractApiError(e, "Failed to record disposal"));
    } finally {
      setDisposingSave(false);
    }
  }

  const counts = {
    total: assets.length,
    active: assets.filter((a) => a.status === "active").length,
    fully_dep: assets.filter((a) => a.status === "fully_depreciated").length,
    disposed: assets.filter((a) => a.status === "disposed").length,
  };

  const rows = catChip ? assets.filter((a) => (a.category_id ?? "none") === catChip) : assets;
  const usedCats = useMemo(() => {
    const seen = new Map<string, string>();
    for (const a of assets) seen.set(a.category_id ?? "none", a.category_name ?? "Uncategorized");
    return Array.from(seen, ([id, label]) => ({ id, label }));
  }, [assets]);

  function startDisposal(asset: FixedAsset) {
    setDisposeAsset(asset);
    setDisposeForm({ disposal_date: "", disposal_type: "sold", proceeds: "0", disposal_notes: "", bank_account_id: "" });
  }

  const columns: ColumnDef<FixedAsset>[] = [
    { accessorKey: "number", header: "Number", meta: { width: "120px" },
      cell: (c) => <span className="font-mono text-[13px]">{c.getValue<string>()}</span> },
    { accessorKey: "name", header: "Asset", cell: (c) => <span className="font-medium">{c.getValue<string>()}</span> },
    { accessorKey: "category_name", header: "Category", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string | null>() ?? "Uncategorized"}</span> },
    { accessorKey: "location", header: "Location", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string | null>() ?? "-"}</span> },
    { accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="asset" status={c.getValue<string>()} label={STATUS_LABELS[c.getValue<string>()]} /> },
    { accessorKey: "purchase_date", header: "Purchased", meta: { numeric: true, hideOnMobile: true } },
    { accessorKey: "purchase_price", header: "Price", meta: { numeric: true },
      cell: (c) => `${c.row.original.currency} ${c.getValue<number>().toLocaleString()}` },
  ];

  return (
    <div className="animate-fade-up">
      <ListFrame>
        <PageHeader flush title="Fixed assets"
          summary={loading ? "Asset register, depreciation schedules, and disposals"
            : `${counts.total} assets, ${counts.active} active, ${counts.fully_dep} fully depreciated, ${counts.disposed} disposed`}
          actions={<Button variant="primary" onClick={() => { setForm({ ...EMPTY_FORM, currency: functionalCurrency }); setShowModal(true); }}><Plus aria-hidden /> Add asset</Button>} />
        <SavedViewTabs state={views} />
        <FilterChips
          chips={usedCats.map((c) => ({ id: c.id, label: c.label, active: catChip === c.id }))}
          onToggle={(id) => setCatChip((cur) => (cur === id ? null : id))} onClear={() => setCatChip(null)}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search assets" aria-label="Search assets" className="w-56" />
        </FilterChips>
        <DataTable label="Fixed assets" columns={columns} data={rows} getRowId={(a) => a.id} loading={loading}
          empty={{ icon: Package, title: "No assets found", description: "Pick another view, or add an asset." }}
          renderDetails={(asset) => (
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                { label: "Category", value: asset.category_name ?? "Uncategorized" },
                { label: "Purchased", value: asset.purchase_date, numeric: true },
                { label: "Price", value: `${asset.currency} ${asset.purchase_price.toLocaleString()}`, numeric: true },
                { label: "Salvage value", value: `${asset.currency} ${asset.salvage_value.toLocaleString()}`, numeric: true },
                { label: "Useful life", value: asset.useful_life_months ? `${asset.useful_life_months} months` : "Category default" },
                { label: "Method", value: asset.depreciation_method?.replace(/_/g, " ") ?? "Category default" },
                { label: "Location", value: asset.location ?? "-" },
                { label: "Serial number", value: asset.serial_number ? <span className="font-mono">{asset.serial_number}</span> : "-" },
                { label: "Notes", value: asset.notes ?? "-" },
              ]} />
              <div className="flex flex-wrap items-center gap-2">
                <Button size="sm" onClick={() => openSchedule(asset)}><TrendingDown aria-hidden /> Depreciation schedule</Button>
                {asset.status === "active" && (
                  <Button size="sm" onClick={() => startDisposal(asset)}><XCircle aria-hidden /> Record disposal</Button>
                )}
                {confirmDeleteId === asset.id ? (
                  <>
                    <span className="text-xs font-medium text-tone-red">Delete?</span>
                    <Button size="sm" variant="danger" onClick={() => deleteAsset(asset.id)}>Yes, delete</Button>
                    <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
                  </>
                ) : (
                  <Button size="sm" variant="danger" onClick={() => setConfirmDeleteId(asset.id)}><Trash2 aria-hidden /> Delete</Button>
                )}
              </div>
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Add fixed asset" size="lg"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={createAsset} loading={saving}>Create asset</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Asset name" required className="md:col-span-2">
            <Input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="e.g. Laser Printer" />
          </Field>
          <Field label="Category">
            <Combobox value={form.category_id} source={categoryOptions} placeholder="None" emptyLabel="No categories"
              onChange={(id) => setForm({ ...form, category_id: id })} />
          </Field>
          <Field label="Currency">
            <Select value={form.currency} onChange={(e) => setForm({ ...form, currency: e.target.value })}
              options={currencyOptions(activeCurrencies, form.currency)} />
          </Field>
          <Field label="Purchase date" required>
            <Input type="date" value={form.purchase_date} onChange={(e) => setForm({ ...form, purchase_date: e.target.value })} />
          </Field>
          <Field label="Purchase price" required>
            <Input type="number" numeric min="0" value={form.purchase_price} onChange={(e) => setForm({ ...form, purchase_price: e.target.value })} placeholder="0" />
          </Field>
          <Field label="Depreciation method">
            <Select value={form.depreciation_method} onChange={(e) => setForm({ ...form, depreciation_method: e.target.value })}
              placeholder="Use category default"
              options={[
                { value: "straight_line", label: "Straight-line" },
                { value: "declining_balance", label: "Declining balance" },
                { value: "none", label: "None" },
              ]} />
          </Field>
          <Field label="Useful life (months)">
            <Input type="number" numeric min="1" value={form.useful_life_months} onChange={(e) => setForm({ ...form, useful_life_months: e.target.value })} placeholder="Override category" />
          </Field>
          <Field label="Salvage value">
            <Input type="number" numeric min="0" value={form.salvage_value} onChange={(e) => setForm({ ...form, salvage_value: e.target.value })} placeholder="0" />
          </Field>
          <Field label="Location">
            <Input value={form.location} onChange={(e) => setForm({ ...form, location: e.target.value })} placeholder="e.g. Server Room, Osaka" />
          </Field>
          <Field label="Serial number" className="md:col-span-2">
            <Input value={form.serial_number} onChange={(e) => setForm({ ...form, serial_number: e.target.value })} className="font-mono" />
          </Field>
        </FieldGrid>
      </Dialog>

      <Sheet open={!!scheduleAsset} onOpenChange={(o) => { if (!o) setScheduleAsset(null); }}
        title={scheduleAsset?.name ?? "Depreciation schedule"} width="min(720px, 100vw)"
        description={scheduleAsset ? `Depreciation schedule, ${scheduleAsset.number}` : undefined}>
        {scheduleAsset && (
          <div className="space-y-4">
            <div className="flex flex-wrap items-center gap-2">
              <Button size="sm" onClick={generateSchedule} loading={generating}><RefreshCw aria-hidden /> Generate / refresh</Button>
              <Button size="sm" variant="primary" onClick={postNextDepreciation} loading={posting}><Send aria-hidden /> Post next period</Button>
            </div>
            <Card>
              <DataTable label="Depreciation schedule" maxHeight="none" data={scheduleLoading ? [] : schedule}
                loading={scheduleLoading} getRowId={(l) => l.period_label}
                empty={{ title: "No schedule generated yet", description: "Use Generate / refresh to create the depreciation timeline.", compact: true }}
                columns={[
                  { accessorKey: "period_label", header: "Period", cell: (c) => <span className="font-medium">{c.getValue<string>()}</span> },
                  { accessorKey: "depreciation_amount", header: "Depreciation", meta: { numeric: true }, cell: (c) => c.getValue<number>().toLocaleString() },
                  { accessorKey: "accumulated_depreciation", header: "Accumulated", meta: { numeric: true }, cell: (c) => c.getValue<number>().toLocaleString() },
                  { accessorKey: "book_value", header: "Book value", meta: { numeric: true }, cell: (c) => <span className="font-medium">{c.getValue<number>().toLocaleString()}</span> },
                  { accessorKey: "db_status", header: "Status", meta: { align: "center" }, enableSorting: false, cell: (c) => {
                    const line = c.row.original;
                    if (line.db_status === "posted") {
                      return (
                        <span className="inline-flex flex-col items-center gap-0.5">
                          <StatusPill family="journal" status="posted" />
                          {line.db_line_id && <JournalLink source="asset_depreciation_line" sourceId={line.db_line_id} />}
                        </span>
                      );
                    }
                    if (line.db_status === "cancelled" || line.db_status === "pending") return <StatusPill family="generic" status={line.db_status} />;
                    return <span className="text-xs text-fg-subtle">-</span>;
                  } },
                ] as ColumnDef<DepreciationLine>[]} />
            </Card>
            {disposals.length > 0 && (
              <Card>
                <CardHeader title="Disposal" />
                <div className="divide-y divide-line-soft">
                  {disposals.map((d) => (
                    <div key={d.id} className="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-2.5 text-sm">
                      <span className="font-medium capitalize text-fg">{d.disposal_type}</span>
                      <span className="text-fg-muted tabular">{d.disposal_date}</span>
                      <span className="text-fg-muted tabular">Proceeds {scheduleAsset.currency} {d.proceeds.toLocaleString()}</span>
                      <JournalLink source="asset_disposal" sourceId={d.id} />
                    </div>
                  ))}
                </div>
              </Card>
            )}
          </div>
        )}
      </Sheet>

      <Dialog open={!!disposeAsset} onOpenChange={(o) => { if (!o) setDisposeAsset(null); }} title="Record disposal"
        description={disposeAsset ? `${disposeAsset.name}, ${disposeAsset.number}` : undefined}
        footer={<>
          <Button onClick={() => setDisposeAsset(null)}>Cancel</Button>
          <Button variant="primary" onClick={submitDisposal} loading={disposingSave}>Record disposal</Button>
        </>}>
        {disposeAsset && (
          <FieldGrid cols={2}>
            <Field label="Disposal date" required>
              <Input type="date" value={disposeForm.disposal_date} onChange={(e) => setDisposeForm({ ...disposeForm, disposal_date: e.target.value })} />
            </Field>
            <Field label="Disposal type">
              <Select value={disposeForm.disposal_type} onChange={(e) => setDisposeForm({ ...disposeForm, disposal_type: e.target.value })}
                options={[{ value: "sold", label: "Sold" }, { value: "scrapped", label: "Scrapped" }, { value: "donated", label: "Donated" }]} />
            </Field>
            <Field label={`Proceeds (${disposeAsset.currency})`} className="md:col-span-2">
              <Input type="number" numeric min="0" value={disposeForm.proceeds} onChange={(e) => setDisposeForm({ ...disposeForm, proceeds: e.target.value })} placeholder="0" />
            </Field>
            {(parseFloat(disposeForm.proceeds) || 0) > 0 && (
              <Field label="Proceeds received into" className="md:col-span-2">
                <BankAccountSelect value={disposeForm.bank_account_id} currency={disposeAsset.currency}
                  onChange={(v) => setDisposeForm({ ...disposeForm, bank_account_id: v })} />
              </Field>
            )}
            <Field label="Notes" className="md:col-span-2">
              <Textarea value={disposeForm.disposal_notes} onChange={(e) => setDisposeForm({ ...disposeForm, disposal_notes: e.target.value })} rows={2} className="resize-none" />
            </Field>
          </FieldGrid>
        )}
      </Dialog>
    </div>
  );
}
