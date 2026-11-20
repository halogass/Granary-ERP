"use client";

import { useEffect, useMemo, useState, useCallback } from "react";
import { financeApi } from "@/lib/api";
import { Plus, Pencil, CheckCircle2, XCircle, BookOpen, Trash2, Link2, Search } from "lucide-react";
import {
  Button, IconButton, Input, Textarea, Select, Checkbox, Field, FieldGrid, Card, CardHeader, CardFooter,
  KeyValues, Pill, PageHeader, ListFrame, SavedViewTabs, useSavedViews, FilterChips, DataTable, Dialog,
  Tabs, TabsList, TabsTrigger, TabsContent, EmptyState, Skeleton, toast,
  type ColumnDef, type SavedView, type Tone,
} from "@/components/ui";
import { extractApiError } from "@/lib/apiError";

interface Account {
  id: string;
  code: string;
  name: string;
  category: string;
  account_type: string | null;
  normal_balance: string;
  currency: string;
  is_active: boolean;
  allow_direct_posting: boolean;
  description: string | null;
}

const CATEGORIES = ["assets", "liabilities", "equity", "revenue", "expenses"];
const CAT_TONES: Record<string, Tone> = {
  assets: "blue",
  liabilities: "orange",
  equity: "violet",
  revenue: "blue",
  expenses: "neutral",
};
const CAT_LABELS: Record<string, string> = {
  assets: "Assets", liabilities: "Liabilities",
  equity: "Equity", revenue: "Revenue", expenses: "Expenses",
};

const ACCOUNT_TYPES = [
  "cash","bank","receivable","payable","tax","revenue","cogs","expense","equity","retained_earnings","other",
];

const emptyForm = {
  code: "", name: "", category: "assets",
  account_type: "", description: "", currency: "USD",
  allow_direct_posting: true,
};

interface GlRole {
  role: string;
  label: string;
  account_id: string | null;
  account_code: string | null;
  account_name: string | null;
}

type Filters = { category?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  ...CATEGORIES.map((c) => ({ id: c, label: CAT_LABELS[c], filters: { category: c } })),
];

/** Which ledger account each automatic posting uses (invoice to receivables, payment to bank, ...). */
function AccountMapping({ accounts }: { accounts: Account[] }) {
  const [roles, setRoles] = useState<GlRole[]>([]);
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(true);
  const [denied, setDenied] = useState(false);
  const [saving, setSaving] = useState(false);
  const [newCategory, setNewCategory] = useState("");

  const apply = (list: GlRole[]) => {
    setRoles(list);
    setDraft(Object.fromEntries(list.map(r => [r.role, r.account_id ?? ""])));
  };

  useEffect(() => {
    financeApi.getGlRoles()
      .then(res => apply(res.data ?? []))
      .catch(e => {
        if ((e as { response?: { status?: number } })?.response?.status === 403) setDenied(true);
        else toast.error(extractApiError(e, "Failed to load account mapping"));
      })
      .finally(() => setLoading(false));
  }, []);

  const original = Object.fromEntries(roles.map(r => [r.role, r.account_id ?? ""]));
  const changed = Object.keys(draft).filter(role => (original[role] ?? "") !== draft[role]);

  function addCategory() {
    const cat = newCategory.trim().toLowerCase().replace(/\s+/g, "_");
    if (!cat) return;
    const role = `expense.${cat}`;
    if (role in draft) { toast.error(`${role} is already listed`); return; }
    setRoles(rs => [...rs, { role, label: `Expense category: ${cat}`, account_id: null, account_code: null, account_name: null }]);
    setDraft(d => ({ ...d, [role]: "" }));
    setNewCategory("");
  }

  async function save() {
    if (changed.length === 0) return;
    setSaving(true);
    try {
      const res = await financeApi.updateGlRoles(Object.fromEntries(changed.map(role => [role, draft[role] || null])));
      apply(res.data ?? []);
      toast.success("Account mapping saved");
    } catch (e) {
      toast.error(extractApiError(e, "Failed to save account mapping"));
    } finally { setSaving(false); }
  }

  if (loading) {
    return (
      <Card className="space-y-3 p-4">
        {[0, 1, 2, 3].map(i => <Skeleton key={i} className="h-8 w-full" />)}
      </Card>
    );
  }
  if (denied) {
    return (
      <Card>
        <EmptyState icon={Link2} title="You don't have permission to view the account mapping" />
      </Card>
    );
  }

  const options = accounts.filter(a => a.is_active).sort((a, b) => a.code.localeCompare(b.code));
  return (
    <Card>
      <CardHeader title="Account mapping"
        description="Accounts the automatic journal postings use. Unmapped roles block posting for that document."
        actions={
          <Button variant="primary" onClick={save} loading={saving} disabled={changed.length === 0}>
            {changed.length ? `Save (${changed.length})` : "Save"}
          </Button>
        } />
      <div className="divide-y divide-line-soft">
        {roles.map(r => {
          const isChanged = changed.includes(r.role);
          return (
            <div key={r.role} className="flex flex-col gap-2 px-4 py-2.5 sm:flex-row sm:items-center sm:gap-4">
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-medium text-fg">{r.label}</p>
                <p className="truncate font-mono text-xs text-fg-subtle">{r.role}</p>
              </div>
              <div className="w-full sm:w-72">
                <Select aria-label={`Account for ${r.label}`} value={draft[r.role] ?? ""}
                  onChange={e => setDraft(d => ({ ...d, [r.role]: e.target.value }))}
                  className={`${isChanged ? "border-brand/50" : ""} ${!draft[r.role] ? "text-fg-subtle" : ""}`}>
                  <option value="">Not mapped</option>
                  {/* Keep a mapped account visible even if it was deactivated since. */}
                  {r.account_id && !options.some(a => a.id === r.account_id) && (
                    <option value={r.account_id}>{r.account_code ?? "?"} - {r.account_name ?? "?"}</option>
                  )}
                  {options.map(a => <option key={a.id} value={a.id}>{a.code} - {a.name}</option>)}
                </Select>
              </div>
            </div>
          );
        })}
      </div>
      <CardFooter className="justify-start">
        <span className="font-mono text-xs text-fg-muted">expense.</span>
        <Input value={newCategory} onChange={e => setNewCategory(e.target.value)}
          onKeyDown={e => { if (e.key === "Enter") addCategory(); }}
          aria-label="New expense category"
          placeholder="expense category, e.g. travel" className="w-56" />
        <Button size="sm" onClick={addCategory} disabled={!newCategory.trim()}>
          <Plus aria-hidden /> Add expense category mapping
        </Button>
      </CardFooter>
    </Card>
  );
}

export default function AccountsPage() {
  const [tab, setTab] = useState<"accounts" | "mapping">("accounts");
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [loading, setLoading] = useState(true);
  const views = useSavedViews<Filters>("finance.accounts", VIEWS);
  const [chips, setChips] = useState<Record<string, boolean>>({});
  const [q, setQ] = useState("");
  const [modal, setModal] = useState<{ open: boolean; editing: Account | null }>({ open: false, editing: null });
  const [form, setForm] = useState(emptyForm);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await financeApi.listAccounts();
      setAccounts(res.data ?? []);
    } catch { setAccounts([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  function openCreate() {
    setForm(emptyForm);
    setError("");
    setModal({ open: true, editing: null });
  }

  function openEdit(a: Account) {
    setForm({
      code: a.code, name: a.name, category: a.category,
      account_type: a.account_type ?? "",
      description: a.description ?? "",
      currency: a.currency,
      allow_direct_posting: a.allow_direct_posting,
    });
    setError("");
    setModal({ open: true, editing: a });
  }

  async function save() {
    setSaving(true); setError("");
    try {
      if (modal.editing) {
        await financeApi.updateAccount(modal.editing.id, {
          name: form.name, account_type: form.account_type || null,
          description: form.description || null,
          currency: form.currency,
          allow_direct_posting: form.allow_direct_posting,
        });
      } else {
        await financeApi.createAccount({
          code: form.code, name: form.name, category: form.category,
          account_type: form.account_type || null,
          description: form.description || null,
          currency: form.currency,
          allow_direct_posting: form.allow_direct_posting,
        });
      }
      setModal({ open: false, editing: null });
      load();
    } catch (e: unknown) {
      const msg = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      setError(msg ?? "Failed to save account");
    } finally { setSaving(false); }
  }

  async function toggleActive(a: Account) {
    await financeApi.updateAccount(a.id, { is_active: !a.is_active });
    load();
  }

  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  async function deleteAccount(id: string) {
    try {
      await financeApi.deleteAccount(id);
      setConfirmDeleteId(null);
      setAccounts((prev) => prev.filter((a) => a.id !== id));
    } catch { /* keep */ }
  }

  const filterCat = views.active?.filters.category;
  const rows = useMemo(() => {
    const term = q.trim().toLowerCase();
    return accounts
      .filter(a => !filterCat || a.category === filterCat)
      .filter(a => !chips.active || a.is_active)
      .filter(a => !chips.inactive || !a.is_active)
      .filter(a => !chips.direct || a.allow_direct_posting)
      .filter(a => !term || a.code.toLowerCase().includes(term) || a.name.toLowerCase().includes(term))
      .map((a, i) => ({ a, i }))
      .sort((x, y) => (CATEGORIES.indexOf(x.a.category) - CATEGORIES.indexOf(y.a.category)) || x.i - y.i)
      .map(x => x.a);
  }, [accounts, filterCat, chips, q]);

  const counts = useMemo(() => {
    const c: Record<string, number> = { all: accounts.length };
    for (const cat of CATEGORIES) c[cat] = accounts.filter(a => a.category === cat).length;
    return c;
  }, [accounts]);

  const columns = useMemo<ColumnDef<Account>[]>(() => [
    { accessorKey: "code", header: "Code", meta: { width: "96px" },
      cell: (c) => <span className="font-mono text-[13px] font-medium">{c.getValue<string>()}</span> },
    { accessorKey: "name", header: "Name", cell: (c) => {
      const a = c.row.original;
      return (
        <div className="min-w-0">
          <p className={`truncate font-medium ${a.is_active ? "text-fg" : "text-fg-subtle"}`}>{a.name}</p>
          {a.description && <p className="truncate text-xs text-fg-muted">{a.description}</p>}
        </div>
      );
    } },
    { accessorKey: "category", header: "Category",
      cell: (c) => <Pill tone={CAT_TONES[c.getValue<string>()] ?? "neutral"}>{CAT_LABELS[c.getValue<string>()] ?? c.getValue<string>()}</Pill> },
    { accessorKey: "account_type", header: "Type", meta: { hideOnMobile: true },
      cell: (c) => c.getValue<string | null>()
        ? <span className="font-mono text-xs text-fg-muted">{c.getValue<string>()}</span>
        : <span className="text-fg-subtle">-</span> },
    { accessorKey: "currency", header: "Currency", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-xs text-fg-muted">{c.getValue<string>()}</span> },
    { accessorKey: "normal_balance", header: "Normal", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-xs font-medium capitalize text-fg-muted">{c.getValue<string>()}</span> },
    { id: "actions", header: () => <span className="sr-only">Actions</span>, enableSorting: false, meta: { align: "right", width: "132px" },
      cell: (c) => {
        const a = c.row.original;
        return (
          <div className="flex items-center justify-end gap-0.5">
            <IconButton size="sm" label={a.is_active ? "Deactivate" : "Activate"} onClick={() => toggleActive(a)}
              icon={a.is_active ? CheckCircle2 : XCircle} className={a.is_active ? "text-tone-blue" : "text-fg-subtle"} />
            <IconButton size="sm" label="Edit account" icon={Pencil} onClick={() => openEdit(a)} />
            {confirmDeleteId === a.id ? (
              <span className="flex items-center gap-1">
                <Button size="sm" variant="danger" onClick={() => deleteAccount(a.id)}>Delete</Button>
                <IconButton size="sm" label="Cancel delete" icon={XCircle} onClick={() => setConfirmDeleteId(null)} />
              </span>
            ) : (
              <IconButton size="sm" label="Delete account" icon={Trash2} onClick={() => setConfirmDeleteId(a.id)} />
            )}
          </div>
        );
      } },
  // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId]);

  const set = (k: keyof typeof emptyForm) => (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>) =>
    setForm(f => ({ ...f, [k]: e.target.value }));

  return (
    <div className="animate-fade-up">
      <Tabs value={tab} onValueChange={(v) => setTab(v as "accounts" | "mapping")}>
        <PageHeader title="Chart of accounts"
          summary={loading ? undefined : `${accounts.length} accounts, ${accounts.filter(a => a.is_active).length} active`}
          actions={<Button variant="primary" onClick={openCreate}><Plus aria-hidden /> Add account</Button>}>
          <TabsList className="mt-3">
            <TabsTrigger value="accounts"><BookOpen className="size-3.5" aria-hidden /> Accounts</TabsTrigger>
            <TabsTrigger value="mapping"><Link2 className="size-3.5" aria-hidden /> Account mapping</TabsTrigger>
          </TabsList>
        </PageHeader>

        <TabsContent value="accounts" className="pt-0">
          <ListFrame>
            <SavedViewTabs state={views} counts={counts} />
            <FilterChips
              chips={[
                { id: "active", label: "Active", active: !!chips.active },
                { id: "inactive", label: "Inactive", active: !!chips.inactive },
                { id: "direct", label: "Direct posting", active: !!chips.direct },
              ]}
              onToggle={(id) => setChips(c => ({ ...c, [id]: !c[id] }))} onClear={() => setChips({})}>
              <Input type="search" autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other" leading={Search} value={q} onChange={e => setQ(e.target.value)}
                placeholder="Search code or name" aria-label="Search accounts" className="w-56" />
            </FilterChips>
            <DataTable label="Accounts" columns={columns} data={rows} getRowId={(a) => a.id} loading={loading}
              empty={{ icon: BookOpen, title: "No accounts found", description: "Pick another view, or add an account." }}
              renderDetails={(a) => (
                <div className="flex flex-wrap items-start gap-6">
                  <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                    { label: "Code", value: <span className="font-mono">{a.code}</span> },
                    { label: "Category", value: CAT_LABELS[a.category] ?? a.category },
                    { label: "Type", value: a.account_type ?? "-" },
                    { label: "Normal balance", value: <span className="capitalize">{a.normal_balance}</span> },
                    { label: "Currency", value: a.currency },
                    { label: "Direct posting", value: a.allow_direct_posting ? "Allowed" : "Not allowed" },
                    { label: "Status", value: a.is_active ? "Active" : "Inactive" },
                    { label: "Description", value: a.description || "-" },
                  ]} />
                  <Button size="sm" onClick={() => openEdit(a)}><Pencil aria-hidden /> Edit account</Button>
                </div>
              )} />
          </ListFrame>
        </TabsContent>

        <TabsContent value="mapping">
          {loading ? null : <AccountMapping accounts={accounts} />}
        </TabsContent>
      </Tabs>

      <Dialog open={modal.open} onOpenChange={(o) => { if (!o) setModal({ open: false, editing: null }); }}
        title={modal.editing ? "Edit account" : "New account"} size="md"
        footer={<>
          <Button onClick={() => setModal({ open: false, editing: null })}>Cancel</Button>
          <Button variant="primary" onClick={save} loading={saving} disabled={!form.name || (!modal.editing && !form.code)}>
            {modal.editing ? "Update" : "Create"}
          </Button>
        </>}>
        <FieldGrid cols={2}>
          {!modal.editing && (<>
            <Field label="Account code" required>
              <Input value={form.code} onChange={set("code")} placeholder="e.g. 1300" className="font-mono" />
            </Field>
            <Field label="Category" required>
              <Select value={form.category} onChange={set("category")}
                options={CATEGORIES.map(c => ({ value: c, label: CAT_LABELS[c] }))} />
            </Field>
          </>)}
          <Field label="Account name" required className="md:col-span-2">
            <Input value={form.name} onChange={set("name")} placeholder="e.g. Accounts Receivable" />
          </Field>
          <Field label="Account type">
            <Select value={form.account_type} onChange={set("account_type")} placeholder="None"
              options={ACCOUNT_TYPES.map(t => ({ value: t, label: t }))} />
          </Field>
          <Field label="Currency">
            <Input value={form.currency} onChange={e => setForm(f => ({ ...f, currency: e.target.value.toUpperCase() }))}
              maxLength={3} className="font-mono uppercase" />
          </Field>
          <Field label="Description" className="md:col-span-2">
            <Textarea value={form.description} onChange={set("description")} rows={2} className="resize-none" />
          </Field>
          <Checkbox className="md:col-span-2" label="Allow direct posting" checked={form.allow_direct_posting}
            onChange={e => setForm(f => ({ ...f, allow_direct_posting: e.target.checked }))} />
        </FieldGrid>
        {error && <p className="mt-3 text-sm text-tone-red">{error}</p>}
      </Dialog>
    </div>
  );
}
