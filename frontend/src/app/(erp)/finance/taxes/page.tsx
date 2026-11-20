"use client";

import { useEffect, useState, useCallback, useMemo } from "react";
import { financeApi } from "@/lib/api";
import { Plus, Pencil, Trash2, CheckCircle2, XCircle, Receipt, Search } from "lucide-react";
import {
  Button, Checkbox, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame,
  PageHeader, Pill, SavedViewTabs, Select, Textarea, useSavedViews, type ColumnDef, type SavedView, type Tone,
} from "@/components/ui";

interface Tax {
  id: string;
  code: string;
  name: string;
  law_reference: string | null;
  rate: number;
  rate_base: string;
  tax_type: string;
  is_active: boolean;
  applies_to_sales: boolean;
  applies_to_purchases: boolean;
  notes: string | null;
  tax_payable_account_id: string | null;
  tax_receivable_account_id: string | null;
}

interface Account { id: string; code: string; name: string; }

const TAX_TYPES = [
  { value: "vat",                  label: "Sales tax / VAT" },
  { value: "withholding_service",  label: "Withholding: services" },
  { value: "withholding_employee", label: "Withholding: employees" },
  { value: "income",               label: "Income Tax" },
  { value: "stamp_duty",           label: "Stamp Duty" },
  { value: "other",                label: "Other" },
];

const RATE_BASES = [
  { value: "subtotal",        label: "% of invoice subtotal" },
  { value: "price",           label: "% of item price" },
  { value: "taxable_amount",  label: "Custom taxable base" },
];

const TYPE_TONE: Record<string, Tone> = {
  vat:                  "blue",
  withholding_service:  "orange",
  withholding_employee: "violet",
  income:               "blue",
  stamp_duty:           "neutral",
  other:                "neutral",
};

type Filters = { scope?: "sales" | "purchases" | "inactive" };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "sales", label: "Sales", filters: { scope: "sales" } },
  { id: "purchases", label: "Purchases", filters: { scope: "purchases" } },
  { id: "inactive", label: "Inactive", filters: { scope: "inactive" } },
];

const emptyForm = {
  code: "", name: "", law_reference: "",
  rate: "", rate_base: "subtotal", tax_type: "vat",
  applies_to_sales: true, applies_to_purchases: false,
  notes: "",
  tax_payable_account_id: "",
  tax_receivable_account_id: "",
};

export default function TaxesPage() {
  const [taxes, setTaxes] = useState<Tax[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [loading, setLoading] = useState(true);
  const [modal, setModal] = useState<{ open: boolean; editing: Tax | null }>({ open: false, editing: null });
  const [form, setForm] = useState(emptyForm);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const views = useSavedViews<Filters>("finance.taxes", VIEWS);
  const [typeChip, setTypeChip] = useState<string | null>(null);
  const [q, setQ] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [tRes, aRes] = await Promise.all([
        financeApi.listTaxes(),
        financeApi.listAccounts({ category: "liabilities" }),
      ]);
      setTaxes(tRes.data ?? []);
      setAccounts(aRes.data ?? []);
    } catch { setTaxes([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  function openCreate() {
    setForm(emptyForm); setError("");
    setModal({ open: true, editing: null });
  }

  function openEdit(t: Tax) {
    setForm({
      code: t.code, name: t.name,
      law_reference: t.law_reference ?? "",
      rate: String(t.rate), rate_base: t.rate_base,
      tax_type: t.tax_type,
      applies_to_sales: t.applies_to_sales,
      applies_to_purchases: t.applies_to_purchases,
      notes: t.notes ?? "",
      tax_payable_account_id: t.tax_payable_account_id ?? "",
      tax_receivable_account_id: t.tax_receivable_account_id ?? "",
    });
    setError("");
    setModal({ open: true, editing: t });
  }

  async function save() {
    setSaving(true); setError("");
    try {
      const payload = {
        code: form.code, name: form.name,
        law_reference: form.law_reference || null,
        rate: parseFloat(form.rate) || 0,
        rate_base: form.rate_base, tax_type: form.tax_type,
        applies_to_sales: form.applies_to_sales,
        applies_to_purchases: form.applies_to_purchases,
        notes: form.notes || null,
        tax_payable_account_id: form.tax_payable_account_id || null,
        tax_receivable_account_id: form.tax_receivable_account_id || null,
      };
      if (modal.editing) {
        await financeApi.updateTax(modal.editing.id, payload);
      } else {
        await financeApi.createTax(payload);
      }
      setModal({ open: false, editing: null });
      load();
    } catch (e: unknown) {
      const msg = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      setError(msg ?? "Failed to save tax");
    } finally { setSaving(false); }
  }

  async function deleteTax(t: Tax) {
    if (!confirm(`Delete tax "${t.code}"? This cannot be undone.`)) return;
    await financeApi.deleteTax(t.id);
    load();
  }

  async function toggleActive(t: Tax) {
    await financeApi.updateTax(t.id, { is_active: !t.is_active });
    load();
  }

  const scope = views.active?.filters.scope;
  const rows = useMemo(() => {
    const term = q.trim().toLowerCase();
    return taxes
      .filter((t) => scope === "sales" ? t.applies_to_sales : scope === "purchases" ? t.applies_to_purchases : scope === "inactive" ? !t.is_active : true)
      .filter((t) => !typeChip || t.tax_type === typeChip)
      .filter((t) => !term || t.code.toLowerCase().includes(term) || t.name.toLowerCase().includes(term) || (t.law_reference ?? "").toLowerCase().includes(term));
  }, [taxes, scope, typeChip, q]);

  const typeLabel = (v: string) => TAX_TYPES.find((x) => x.value === v)?.label ?? v;
  const baseLabel = (v: string) => RATE_BASES.find((b) => b.value === v)?.label ?? v;
  const accountLabel = (id: string | null) => {
    if (!id) return "None";
    const a = accounts.find((x) => x.id === id);
    return a ? `${a.code} - ${a.name}` : id;
  };
  const yesNo = (v: boolean, label: string) => v
    ? <CheckCircle2 className="mx-auto size-4 text-tone-blue" aria-label={`${label}: yes`} />
    : <XCircle className="mx-auto size-4 text-fg-subtle" aria-label={`${label}: no`} />;

  const columns: ColumnDef<Tax>[] = [
    { accessorKey: "code", header: "Code", meta: { width: "96px" },
      cell: (c) => <span className={`font-mono text-[13px] font-medium ${c.row.original.is_active ? "" : "text-fg-subtle"}`}>{c.getValue<string>()}</span> },
    { accessorKey: "name", header: "Name / law", cell: (c) => {
      const t = c.row.original;
      return (
        <div className="min-w-0">
          <p className={`truncate font-medium ${t.is_active ? "text-fg" : "text-fg-subtle"}`}>{t.name}</p>
          {t.law_reference && <p className="truncate text-xs text-fg-muted">{t.law_reference}</p>}
        </div>
      );
    } },
    { accessorKey: "tax_type", header: "Type", meta: { hideOnMobile: true },
      cell: (c) => <Pill tone={TYPE_TONE[c.getValue<string>()] ?? "neutral"}>{typeLabel(c.getValue<string>())}</Pill> },
    { accessorKey: "rate", header: "Rate", meta: { numeric: true }, cell: (c) => <span className="font-medium">{c.getValue<number>()}%</span> },
    { accessorKey: "applies_to_sales", header: "Sales", meta: { align: "center", hideOnMobile: true }, cell: (c) => yesNo(c.getValue<boolean>(), "Sales") },
    { accessorKey: "applies_to_purchases", header: "Purchase", meta: { align: "center", hideOnMobile: true }, cell: (c) => yesNo(c.getValue<boolean>(), "Purchases") },
    { id: "actions", header: () => <span className="sr-only">Actions</span>, enableSorting: false, meta: { align: "right", width: "120px" },
      cell: (c) => {
        const t = c.row.original;
        return (
          <div className="flex items-center justify-end gap-0.5">
            <IconButton size="sm" label={t.is_active ? "Deactivate" : "Activate"} icon={t.is_active ? CheckCircle2 : XCircle}
              className={t.is_active ? "text-tone-blue" : "text-fg-subtle"} onClick={() => toggleActive(t)} />
            <IconButton size="sm" label="Edit tax" icon={Pencil} onClick={() => openEdit(t)} />
            <IconButton size="sm" label="Delete tax" icon={Trash2} onClick={() => deleteTax(t)} />
          </div>
        );
      } },
  ];

  const usedTypes = TAX_TYPES.filter((tt) => taxes.some((t) => t.tax_type === tt.value));
  const accountOptions = accounts.map((a) => ({ value: a.id, label: `${a.code} - ${a.name}` }));

  return (
    <div className="animate-fade-up">
      <ListFrame>
        <PageHeader flush title="Tax configuration"
          summary={loading ? "Law reference, rate, and calculation base per tax" : `${taxes.length} taxes, ${taxes.filter((t) => t.is_active).length} active`}
          actions={<Button variant="primary" onClick={openCreate}><Plus aria-hidden /> Add tax</Button>} />
        <SavedViewTabs state={views} />
        <FilterChips
          chips={usedTypes.map((tt) => ({ id: tt.value, label: tt.label, active: typeChip === tt.value }))}
          onToggle={(id) => setTypeChip((cur) => (cur === id ? null : id))} onClear={() => setTypeChip(null)}>
          <Input type="search" autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other" leading={Search} value={q} onChange={(e) => setQ(e.target.value)}
            placeholder="Search taxes" aria-label="Search taxes" className="w-56" />
        </FilterChips>
        <DataTable label="Taxes" columns={columns} data={rows} getRowId={(t) => t.id} loading={loading}
          empty={{ icon: Receipt, title: "No taxes configured", description: "Pick another view, or add a tax." }}
          renderDetails={(t) => (
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                { label: "Type", value: typeLabel(t.tax_type) },
                { label: "Rate", value: `${t.rate}%`, numeric: true },
                { label: "Rate base", value: baseLabel(t.rate_base) },
                { label: "Law reference", value: t.law_reference ?? "-" },
                { label: "Payable account", value: accountLabel(t.tax_payable_account_id) },
                { label: "Receivable account", value: accountLabel(t.tax_receivable_account_id) },
                { label: "Applies to", value: [t.applies_to_sales && "Sales", t.applies_to_purchases && "Purchases"].filter(Boolean).join(", ") || "None" },
                { label: "Status", value: t.is_active ? "Active" : "Inactive" },
                { label: "Notes", value: t.notes ?? "-" },
              ]} />
              <Button size="sm" onClick={() => openEdit(t)}><Pencil aria-hidden /> Edit tax</Button>
            </div>
          )} />
      </ListFrame>

      <Dialog open={modal.open} onOpenChange={(o) => { if (!o) setModal({ open: false, editing: null }); }}
        title={modal.editing ? "Edit tax" : "New tax"} size="lg"
        footer={<>
          <Button onClick={() => setModal({ open: false, editing: null })}>Cancel</Button>
          <Button variant="primary" onClick={save} loading={saving} disabled={!form.code || !form.name}>
            {modal.editing ? "Update" : "Create"}
          </Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Tax code" required>
            <Input value={form.code} onChange={e => setForm(f => ({ ...f, code: e.target.value.toUpperCase() }))}
              placeholder="VAT10" disabled={!!modal.editing} className="font-mono uppercase" />
          </Field>
          <Field label="Tax type" required>
            <Select value={form.tax_type} onChange={e => setForm(f => ({ ...f, tax_type: e.target.value }))} options={TAX_TYPES} />
          </Field>
          <Field label="Tax name" required className="md:col-span-2">
            <Input value={form.name} onChange={e => setForm(f => ({ ...f, name: e.target.value }))} placeholder="Value Added Tax" />
          </Field>
          <Field label="Law reference" className="md:col-span-2">
            <Input value={form.law_reference} onChange={e => setForm(f => ({ ...f, law_reference: e.target.value }))}
              placeholder="e.g. Tax Act, section 12" />
          </Field>
          <Field label="Rate (%)">
            <Input type="number" numeric step="0.01" value={form.rate} onChange={e => setForm(f => ({ ...f, rate: e.target.value }))} placeholder="11" />
          </Field>
          <Field label="Rate base">
            <Select value={form.rate_base} onChange={e => setForm(f => ({ ...f, rate_base: e.target.value }))} options={RATE_BASES} />
          </Field>
          <div className="flex flex-wrap gap-6 md:col-span-2">
            <Checkbox label="Applies to sales" checked={form.applies_to_sales}
              onChange={e => setForm(f => ({ ...f, applies_to_sales: e.target.checked }))} />
            <Checkbox label="Applies to purchases" checked={form.applies_to_purchases}
              onChange={e => setForm(f => ({ ...f, applies_to_purchases: e.target.checked }))} />
          </div>
          <Field label="Tax payable account">
            <Select value={form.tax_payable_account_id} placeholder="None" options={accountOptions}
              onChange={e => setForm(f => ({ ...f, tax_payable_account_id: e.target.value }))} />
          </Field>
          <Field label="Tax receivable account">
            <Select value={form.tax_receivable_account_id} placeholder="None" options={accountOptions}
              onChange={e => setForm(f => ({ ...f, tax_receivable_account_id: e.target.value }))} />
          </Field>
          <Field label="Notes" className="md:col-span-2">
            <Textarea value={form.notes} onChange={e => setForm(f => ({ ...f, notes: e.target.value }))}
              rows={2} placeholder="Additional notes or implementation details" className="resize-none" />
          </Field>
        </FieldGrid>
        {error && <p className="mt-3 text-sm text-tone-red">{error}</p>}
      </Dialog>
    </div>
  );
}
