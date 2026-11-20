"use client";

import { useState, useEffect, useCallback, useMemo } from "react";
import { Plus, Tag, Trash2, Pencil, Search } from "lucide-react";
import Link from "next/link";
import api, { financeApi } from "@/lib/api";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame, PageHeader,
  SavedViewTabs, Select, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface AssetCategory {
  id: string;
  name: string;
  description: string | null;
  depreciation_method: string;
  useful_life_months: number | null;
  salvage_value_pct: number;
  gl_asset_account_id: string | null;
  gl_depreciation_account_id: string | null;
  gl_accumulated_account_id: string | null;
}

interface GlAccount {
  id: string;
  code: string;
  name: string;
  category: string;
  is_active: boolean;
}

const METHOD_LABELS: Record<string, string> = {
  straight_line: "Straight-line",
  declining_balance: "Declining balance",
  none: "None (manual)",
};

type Filters = { method?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "straight_line", label: "Straight-line", filters: { method: "straight_line" } },
  { id: "declining_balance", label: "Declining balance", filters: { method: "declining_balance" } },
  { id: "none", label: "Manual", filters: { method: "none" } },
];

const EMPTY_FORM = {
  name: "",
  description: "",
  depreciation_method: "straight_line",
  useful_life_months: "",
  salvage_value_pct: "0",
  gl_asset_account_id: "",
  gl_depreciation_account_id: "",
  gl_accumulated_account_id: "",
};

export default function AssetCategoriesPage() {
  const [categories, setCategories] = useState<AssetCategory[]>([]);
  const [accounts, setAccounts] = useState<GlAccount[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [editing, setEditing] = useState<AssetCategory | null>(null);
  const [form, setForm] = useState(EMPTY_FORM);
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const views = useSavedViews<Filters>("finance.asset-categories", VIEWS);
  const [chips, setChips] = useState<Record<string, boolean>>({});
  const [q, setQ] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await api.get("/finance/categories");
      setCategories(res.data ?? []);
    } catch {
      setCategories([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { load(); }, [load]);

  // The chart of accounts for the three GL pickers (posting depreciation and disposals needs them).
  useEffect(() => {
    financeApi.listAccounts({ active_only: true })
      .then((r) => setAccounts(r.data ?? []))
      .catch(() => setAccounts([]));
  }, []);

  const accountLabel = (id: string | null) => {
    if (!id) return "Not set";
    const a = accounts.find((x) => x.id === id);
    return a ? `${a.code} - ${a.name}` : <span className="font-mono text-xs">{id}</span>;
  };
  /** Options for one picker: the accounts of the expected category first, then the rest of the chart. */
  const accountOptions = useCallback((category: string) => {
    const label = (a: GlAccount) => ({ value: a.id, label: `${a.code} - ${a.name}` });
    return [
      ...accounts.filter((a) => a.category === category).map(label),
      ...accounts.filter((a) => a.category !== category).map(label),
    ];
  }, [accounts]);

  function openCreate() {
    setEditing(null);
    setForm(EMPTY_FORM);
    setShowModal(true);
  }

  function openEdit(cat: AssetCategory) {
    setEditing(cat);
    setForm({
      name: cat.name,
      description: cat.description ?? "",
      depreciation_method: cat.depreciation_method,
      useful_life_months: cat.useful_life_months?.toString() ?? "",
      salvage_value_pct: cat.salvage_value_pct.toString(),
      gl_asset_account_id: cat.gl_asset_account_id ?? "",
      gl_depreciation_account_id: cat.gl_depreciation_account_id ?? "",
      gl_accumulated_account_id: cat.gl_accumulated_account_id ?? "",
    });
    setShowModal(true);
  }

  async function save() {
    if (!form.name.trim()) return toast.error("Name is required");
    setSaving(true);
    try {
      const payload = {
        name: form.name.trim(),
        description: form.description || null,
        depreciation_method: form.depreciation_method,
        useful_life_months: form.useful_life_months ? parseInt(form.useful_life_months) : null,
        salvage_value_pct: parseFloat(form.salvage_value_pct) || 0,
        gl_asset_account_id: form.gl_asset_account_id || null,
        gl_depreciation_account_id: form.gl_depreciation_account_id || null,
        gl_accumulated_account_id: form.gl_accumulated_account_id || null,
      };
      if (editing) {
        await api.patch(`/finance/categories/${editing.id}`, payload);
        toast.success("Category updated");
      } else {
        await api.post("/finance/categories", payload);
        toast.success("Category created");
      }
      setShowModal(false);
      load();
    } catch {
      toast.error("Failed to save category");
    } finally {
      setSaving(false);
    }
  }

  async function deleteCategory(id: string) {
    try {
      await api.delete(`/finance/categories/${id}`);
      toast.success("Category deleted");
      setConfirmDeleteId(null);
      load();
    } catch {
      toast.error("Failed to delete category");
    }
  }

  const method = views.active?.filters.method;
  const rows = useMemo(() => {
    const term = q.trim().toLowerCase();
    return categories
      .filter((c) => !method || c.depreciation_method === method)
      .filter((c) => !chips.no_life || !c.useful_life_months)
      .filter((c) => !chips.salvage || c.salvage_value_pct > 0)
      .filter((c) => !term || c.name.toLowerCase().includes(term) || (c.description ?? "").toLowerCase().includes(term));
  }, [categories, method, chips, q]);

  const columns: ColumnDef<AssetCategory>[] = [
    { accessorKey: "name", header: "Category", cell: (c) => {
      const cat = c.row.original;
      return (
        <div className="min-w-0">
          <p className="truncate font-medium text-fg">{cat.name}</p>
          {cat.description && <p className="truncate text-xs text-fg-muted">{cat.description}</p>}
        </div>
      );
    } },
    { accessorKey: "depreciation_method", header: "Method",
      cell: (c) => <span className="text-fg-muted">{METHOD_LABELS[c.getValue<string>()] ?? c.getValue<string>()}</span> },
    { accessorKey: "useful_life_months", header: "Useful life", meta: { numeric: true },
      cell: (c) => (c.getValue<number | null>() ? `${c.getValue<number>()} mo` : "-") },
    { accessorKey: "salvage_value_pct", header: "Salvage", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => `${c.getValue<number>()}%` },
    { id: "actions", header: () => <span className="sr-only">Actions</span>, enableSorting: false, meta: { align: "right", width: "160px" },
      cell: (c) => {
        const cat = c.row.original;
        return confirmDeleteId === cat.id ? (
          <div className="flex items-center justify-end gap-1.5">
            <span className="text-xs font-medium text-tone-red">Delete?</span>
            <Button size="sm" variant="danger" onClick={() => deleteCategory(cat.id)}>Yes</Button>
            <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
          </div>
        ) : (
          <div className="flex items-center justify-end gap-0.5">
            <IconButton size="sm" label="Edit category" icon={Pencil} onClick={() => openEdit(cat)} />
            <IconButton size="sm" label="Delete category" icon={Trash2} onClick={() => setConfirmDeleteId(cat.id)} />
          </div>
        );
      } },
  ];

  return (
    <div className="animate-fade-up">
      <ListFrame>
        <PageHeader flush title="Asset categories"
          summary={loading ? "Depreciation methods and GL accounts per asset type" : `${categories.length} categories`}
          actions={<Button variant="primary" onClick={openCreate}><Plus aria-hidden /> New category</Button>} />
        <SavedViewTabs state={views} />
        <FilterChips
          chips={[
            { id: "no_life", label: "No useful life set", active: !!chips.no_life },
            { id: "salvage", label: "With salvage value", active: !!chips.salvage },
          ]}
          onToggle={(id) => setChips((c) => ({ ...c, [id]: !c[id] }))} onClear={() => setChips({})}>
          <Input type="search" autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other" leading={Search} value={q} onChange={(e) => setQ(e.target.value)}
            placeholder="Search categories" aria-label="Search categories" className="w-56" />
        </FilterChips>
        <DataTable label="Asset categories" columns={columns} data={rows} getRowId={(c) => c.id} loading={loading}
          empty={{ icon: Tag, title: "No categories yet", description: "Create one to get started." }}
          renderDetails={(cat) => (
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                { label: "Method", value: METHOD_LABELS[cat.depreciation_method] ?? cat.depreciation_method },
                { label: "Useful life", value: cat.useful_life_months ? `${cat.useful_life_months} months` : "-", numeric: true },
                { label: "Salvage value", value: `${cat.salvage_value_pct}%`, numeric: true },
                { label: "Asset account", value: accountLabel(cat.gl_asset_account_id) },
                { label: "Depreciation expense account", value: accountLabel(cat.gl_depreciation_account_id) },
                { label: "Accumulated depreciation account", value: accountLabel(cat.gl_accumulated_account_id) },
              ]} />
              <Button size="sm" onClick={() => openEdit(cat)}><Pencil aria-hidden /> Edit category</Button>
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title={editing ? "Edit category" : "New asset category"}
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={save} loading={saving}>{editing ? "Save changes" : "Create category"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Category name" required className="md:col-span-2">
            <Input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="e.g. Vehicles, IT Equipment, Machinery" />
          </Field>
          <Field label="Description" className="md:col-span-2">
            <Input value={form.description} onChange={(e) => setForm({ ...form, description: e.target.value })} />
          </Field>
          <Field label="Depreciation method">
            <Select value={form.depreciation_method} onChange={(e) => setForm({ ...form, depreciation_method: e.target.value })}
              options={[
                { value: "straight_line", label: "Straight-line" },
                { value: "declining_balance", label: "Declining balance (DDB)" },
                { value: "none", label: "None (manual)" },
              ]} />
          </Field>
          <Field label="Useful life (months)">
            <Input type="number" numeric min="1" value={form.useful_life_months} onChange={(e) => setForm({ ...form, useful_life_months: e.target.value })} placeholder="e.g. 60" />
          </Field>
          <Field label="Salvage value %" className="md:col-span-2">
            <Input type="number" numeric min="0" max="100" step="0.01" value={form.salvage_value_pct}
              onChange={(e) => setForm({ ...form, salvage_value_pct: e.target.value })} placeholder="0" />
          </Field>
          <Field label="Asset account" className="md:col-span-2"
            hint="The balance sheet account holding the assets' cost. Needed to dispose of an asset.">
            <Select value={form.gl_asset_account_id} placeholder="Not set" options={accountOptions("assets")}
              onChange={(e) => setForm({ ...form, gl_asset_account_id: e.target.value })} />
          </Field>
          <Field label="Depreciation expense account"
            hint="Debited each period. Needed to post depreciation.">
            <Select value={form.gl_depreciation_account_id} placeholder="Not set" options={accountOptions("expenses")}
              onChange={(e) => setForm({ ...form, gl_depreciation_account_id: e.target.value })} />
          </Field>
          <Field label="Accumulated depreciation account"
            hint="Credited each period (a contra-asset). Needed to post depreciation and disposals.">
            <Select value={form.gl_accumulated_account_id} placeholder="Not set" options={accountOptions("assets")}
              onChange={(e) => setForm({ ...form, gl_accumulated_account_id: e.target.value })} />
          </Field>
          <p className="text-xs text-fg-muted md:col-span-2">
            The gain or loss on a disposal goes to the account mapped to &ldquo;Gain / loss on asset disposal&rdquo; in
            the <Link href="/finance/accounts" className="text-brand-ink underline">Chart of Accounts</Link> account mapping.
          </p>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
