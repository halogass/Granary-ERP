"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useState, useCallback, useRef } from "react";
import { salesApi, crmApi, systemApi, attachmentApi } from "@/lib/api";
import { formatCurrency } from "@/lib/currency";
import { FileSignature, Plus, Search, Trash2, X, Paperclip } from "lucide-react";
import AttachmentPanel from "@/components/ui/AttachmentPanel";
import ApprovalPanel from "@/components/approvals/ApprovalPanel";
import {
  Button, Checkbox, Combobox, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame,
  PageHeader, SavedViewTabs, Select, Stat, StatGroup, StatusPill, Tabs, TabsContent, TabsList, TabsTrigger, Textarea,
  useSavedViews, type ColumnDef, type SavedView, statusLabel,
} from "@/components/ui";
import { cn } from "@/lib/utils";
import { orgSource } from "@/lib/entitySources";
import { useDropToInput } from "@/hooks/useFileDrop";
import { extractApiError } from "@/lib/apiError";
import ErrorToast from "@/components/ui/ErrorToast";

interface Contract {
  id: string;
  number: string;
  status: string;
  contract_type: string;
  value?: number;
  currency: string;
  start_date?: string;
  end_date?: string;
  auto_renew: boolean;
  customer_org_id?: string;
  signed_at?: string;
  created_at: string;
}

interface Org {
  id: string;
  name: string;
}

function ContractStatus({ status }: { status: string }) {
  return <StatusPill family="contract" status={status} />;
}

const fmtDate = (d?: string, month: "short" | "long" = "short") =>
  d ? new Date(d).toLocaleDateString("en-GB", { day: "numeric", month, year: "numeric" }) : "-";

type Filters = { status?: string };
const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "draft", label: "Drafts", filters: { status: "draft" } },
  { id: "sent_for_signing", label: "Sent for signing", filters: { status: "sent_for_signing" } },
  { id: "signed", label: "Signed", filters: { status: "signed" } },
];
const CHIP_STATUSES = ["expired", "terminated"];

const CONTRACT_TYPES = ["sales", "service", "msa", "nda", "partner_agreement", "vendor_agreement"];

const NEXT_STATUS: Record<string, string> = {
  draft: "sent_for_signing",
  sent_for_signing: "signed",
};

export default function ContractsPage() {
  const [actionError, setActionError] = useState("");
  const [contracts, setContracts] = useState<Contract[]>([]);
  const [orgs, setOrgs] = useState<Org[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [pendingFiles, setPendingFiles] = useState<File[]>([]);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const { dragActive: docDrag, dropHandlers: docDrop } = useDropToInput(fileInputRef);
  const [showSignModal, setShowSignModal] = useState<Contract | null>(null);
  const views = useSavedViews<Filters>("sales.contracts", VIEWS);
  const [chipStatus, setChipStatus] = useState("");
  const [search, setSearch] = useState("");
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [functionalCurrency, setFunctionalCurrency] = useState("USD");
  const [form, setForm] = useState({
    customer_org_id: "", contract_type: "sales", currency: "USD",
    value: "", start_date: "", end_date: "", auto_renew: false, notes: "",
  });
  const [signForm, setSignForm] = useState({
    signed_by_customer_name: "", signed_by_customer_title: "", signed_by_company_name: "", document_url: "",
  });

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [cRes, oRes] = await Promise.all([
        fetchAllPages<Contract>(salesApi.listContracts),
        fetchAllPages<Org>(crmApi.listOrgs),
      ]);
      setContracts(cRes.rows);
      setOrgs(oRes.rows);
    } catch { setContracts([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  useEffect(() => {
    systemApi.getCurrencySettings()
      .then((r) => setFunctionalCurrency(r.data?.functional ?? "USD"))
      .catch(() => {});
  }, []);

  async function create() {
    if (!form.contract_type) return;
    setSaving(true);
    try {
      const res = await salesApi.createContract({
        customer_org_id: form.customer_org_id || undefined,
        contract_type: form.contract_type,
        currency: form.currency,
        value: form.value ? parseFloat(form.value) : undefined,
        start_date: form.start_date || undefined,
        end_date: form.end_date || undefined,
        auto_renew: form.auto_renew,
        notes: form.notes || undefined,
      });
      // Upload any documents picked in the modal against the freshly-created contract.
      const newId = res?.data?.id;
      if (newId && pendingFiles.length) {
        for (const file of pendingFiles) {
          try { await attachmentApi.upload("contract", newId, file); }
          catch { /* one failed upload shouldn't block the others / the create */ }
        }
      }
      setShowModal(false);
      setForm({ customer_org_id: "", contract_type: "sales", currency: "USD", value: "", start_date: "", end_date: "", auto_renew: false, notes: "" });
      setPendingFiles([]);
      await load();
    } finally { setSaving(false); }
  }

  async function deleteContract(id: string) {
    setDeleting(true);
    try {
      await salesApi.deleteContract(id);
      setConfirmDeleteId(null);
      setContracts((prev) => prev.filter((c) => c.id !== id));
    } catch { /* keep */ } finally { setDeleting(false); }
  }

  async function advanceStatus(contract: Contract) {
    const nextStatus = NEXT_STATUS[contract.status];
    if (!nextStatus) return;
    if (nextStatus === "signed") {
      setShowSignModal(contract);
      return;
    }
    setActionError("");
    try {
      await salesApi.updateContractStatus(contract.id, { status: nextStatus });
    } catch (e) {
      setActionError(extractApiError(e, "Could not update the contract."));
    }
    window.dispatchEvent(new Event("approvals:changed"));
    await load();
  }

  async function markSigned() {
    if (!showSignModal) return;
    setSaving(true);
    try {
      await salesApi.updateContractStatus(showSignModal.id, {
        status: "signed",
        ...signForm,
      });
      setShowSignModal(null);
      setSignForm({ signed_by_customer_name: "", signed_by_customer_title: "", signed_by_company_name: "", document_url: "" });
      await load();
    } catch (e) {
      setActionError(extractApiError(e, "Could not mark the contract signed."));
      setShowSignModal(null);
    } finally { setSaving(false); }
  }

  const orgMap = Object.fromEntries(orgs.map((o) => [o.id, o.name]));
  const filterStatus = chipStatus || views.active?.filters.status || "all";
  const q = search.trim().toLowerCase();
  const filtered = (filterStatus === "all" ? contracts : contracts.filter((c) => c.status === filterStatus))
    .filter((c) => !q || c.number.toLowerCase().includes(q) || (c.customer_org_id ? orgMap[c.customer_org_id] ?? "" : "").toLowerCase().includes(q));

  const signed = contracts.filter((c) => c.status === "signed").length;
  const pending = contracts.filter((c) => ["draft", "sent_for_signing"].includes(c.status)).length;
  const totalValue = contracts.filter((c) => c.status === "signed").reduce((s, c) => s + (c.value ?? 0), 0);

  const now = new Date();

  const columns: ColumnDef<Contract>[] = [
    {
      accessorKey: "number", header: "Contract",
      cell: (c) => {
        const k = c.row.original;
        const isExpiringSoon = k.end_date && k.status === "signed" &&
          (new Date(k.end_date).getTime() - now.getTime()) < 30 * 86400000 && new Date(k.end_date) > now;
        return (
          <div className="min-w-0">
            <p className="font-mono text-[13px] font-medium text-fg">{k.number}</p>
            <p className="text-xs text-fg-muted">
              {k.start_date && fmtDate(k.start_date)}
              {k.end_date && ` to ${fmtDate(k.end_date)}`}
              {isExpiringSoon && <span className="ml-1 font-semibold text-tone-orange">expiring soon</span>}
              {k.auto_renew && <span className="ml-1 text-tone-blue">· auto-renew</span>}
            </p>
          </div>
        );
      },
    },
    {
      id: "customer", accessorFn: (k) => (k.customer_org_id ? orgMap[k.customer_org_id] ?? "-" : "-"), header: "Customer",
      cell: (c) => <span className="block max-w-48 truncate">{c.getValue<string>()}</span>,
    },
    {
      accessorKey: "contract_type", header: "Type", meta: { hideOnMobile: true },
      cell: (c) => <span className="capitalize text-fg-muted">{c.getValue<string>().replace("_", " ")}</span>,
    },
    {
      id: "value", accessorFn: (k) => k.value ?? 0, header: "Value", meta: { numeric: true },
      cell: (c) => <span className="font-medium">{c.row.original.value ? `${c.row.original.value.toLocaleString()} ${c.row.original.currency}` : "-"}</span>,
    },
    { accessorKey: "status", header: "Status", cell: (c) => <ContractStatus status={c.getValue<string>()} /> },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const k = c.row.original;
        const nextBtn = NEXT_STATUS[k.status];
        const canDelete = ["draft", "terminated"].includes(k.status);
        if (confirmDeleteId === k.id) {
          return (
            <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
              <span className="text-xs font-semibold text-tone-red">Delete?</span>
              <Button size="sm" variant="danger" loading={deleting} onClick={() => deleteContract(k.id)}>Yes</Button>
              <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
            </div>
          );
        }
        return (
          <div className="flex items-center justify-end gap-1 whitespace-nowrap">
            {nextBtn && (
              <Button size="sm" variant="ghost" className="text-brand-ink" onClick={() => advanceStatus(k)}>
                {nextBtn === "sent_for_signing" ? "Send" : "Mark signed"}
              </Button>
            )}
            {canDelete && <IconButton size="sm" icon={Trash2} label={`Delete ${k.number}`} onClick={() => setConfirmDeleteId(k.id)} />}
          </div>
        );
      },
    },
  ];

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={contracts.length} />
        <Stat label="Signed" value={signed} />
        <Stat label="In progress" value={pending} note="draft or sent for signing" />
        <Stat label="Total value" value={formatCurrency(totalValue, functionalCurrency)} note="signed contracts" />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Contracts" summary="Sales agreements, MSAs, NDAs and service contracts"
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> New contract</Button>} />
        <SavedViewTabs state={views} currentFilters={filterStatus === "all" ? {} : { status: filterStatus }} onSelect={() => setChipStatus("")} />
        <FilterChips chips={CHIP_STATUSES.map((s) => ({ id: s, label: statusLabel(s), active: chipStatus === s }))}
          onToggle={(id) => setChipStatus((cur) => (cur === id ? "" : id))} onClear={() => setChipStatus("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search contracts" aria-label="Search contracts" className="w-56" />
        </FilterChips>
        <DataTable label="Contracts" columns={columns} data={filtered} getRowId={(k) => k.id} loading={loading}
          empty={{ icon: FileSignature, title: "No contracts", description: "Create your first contract to get started." }}
          renderDetails={(k) => (
            <Tabs defaultValue="details">
              <TabsList>
                <TabsTrigger value="details">Details</TabsTrigger>
                <TabsTrigger value="files"><Paperclip className="size-3.5" aria-hidden /> Files</TabsTrigger>
              </TabsList>
              <TabsContent value="details" className="pt-3">
                {/* Sending for signing goes through the approval gate: its state, trail and actions live here. */}
                <ApprovalPanel entityType="sales_contract" entityId={k.id} className="mb-3" onChange={load} />
                <KeyValues cols={3} items={[
                  { label: "Type", value: <span className="capitalize">{k.contract_type.replace("_", " ")}</span> },
                  { label: "Value", value: k.value ? `${k.value.toLocaleString()} ${k.currency}` : "-", numeric: true },
                  { label: "Start date", value: fmtDate(k.start_date, "long") },
                  { label: "End date", value: fmtDate(k.end_date, "long") },
                  { label: "Signed at", value: fmtDate(k.signed_at, "long") },
                  { label: "Auto renew", value: k.auto_renew ? "Yes" : "No" },
                ]} />
              </TabsContent>
              <TabsContent value="files" className="pt-3">
                <AttachmentPanel entityType="contract" entityId={k.id} />
              </TabsContent>
            </Tabs>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={(o) => { if (!o) { setShowModal(false); setPendingFiles([]); } }} title="New contract"
        footer={<>
          <Button onClick={() => { setShowModal(false); setPendingFiles([]); }}>Cancel</Button>
          <Button variant="primary" loading={saving} onClick={create}>{saving ? "Creating..." : "Create contract"}</Button>
        </>}>
        <div className="space-y-4">
          <Field label="Customer">
            <Combobox value={form.customer_org_id} source={orgSource}
              placeholder="Select organisation" emptyLabel="No organizations found"
              onChange={(id) => setForm({ ...form, customer_org_id: id })} />
          </Field>
          <FieldGrid cols={2}>
            <Field label="Contract type" required>
              <Select value={form.contract_type} onChange={(e) => setForm({ ...form, contract_type: e.target.value })}
                options={CONTRACT_TYPES.map((t) => ({ value: t, label: t.replace("_", " ") }))} />
            </Field>
            <Field label="Currency">
              <Select value={form.currency} onChange={(e) => setForm({ ...form, currency: e.target.value })}
                options={["USD", "EUR", "GBP", "JPY", "SGD"].map((c) => ({ value: c, label: c }))} />
            </Field>
            <Field label="Contract value" className="md:col-span-2">
              <Input type="number" numeric value={form.value} onChange={(e) => setForm({ ...form, value: e.target.value })} placeholder="0.00" />
            </Field>
            <Field label="Start date">
              <Input type="date" value={form.start_date} onChange={(e) => setForm({ ...form, start_date: e.target.value })} />
            </Field>
            <Field label="End date">
              <Input type="date" value={form.end_date} onChange={(e) => setForm({ ...form, end_date: e.target.value })} />
            </Field>
          </FieldGrid>
          <Checkbox label="Auto-renew on expiry" checked={form.auto_renew} onChange={(e) => setForm({ ...form, auto_renew: e.target.checked })} />
          <Field label="Notes">
            <Textarea value={form.notes} onChange={(e) => setForm({ ...form, notes: e.target.value })} rows={2} />
          </Field>
          <Field label="Documents" hint="PDF, signed contract or supporting files (optional).">
            <div
              role="button" tabIndex={0}
              onClick={() => fileInputRef.current?.click()}
              onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); fileInputRef.current?.click(); } }}
              {...docDrop}
              className={cn("flex cursor-pointer items-center gap-2 rounded-lg border-2 border-dashed px-4 py-3 transition-colors",
                docDrag ? "border-brand bg-brand/5" : "border-line-strong hover:bg-surface-2")}>
              <Paperclip className="size-4 shrink-0 text-fg-subtle" aria-hidden />
              <span className="text-xs text-fg-muted">{docDrag ? "Drop files here" : "Drag and drop or click to attach PDF/documents"}</span>
              <input
                ref={fileInputRef}
                type="file"
                multiple
                className="hidden"
                onChange={(e) => {
                  const files = Array.from(e.target.files ?? []);
                  if (files.length) setPendingFiles((prev) => [...prev, ...files]);
                  if (fileInputRef.current) fileInputRef.current.value = "";
                }}
              />
            </div>
          </Field>
          {pendingFiles.length > 0 && (
            <div className="space-y-1.5">
              {pendingFiles.map((f, i) => (
                <div key={i} className="flex items-center gap-2 rounded-lg bg-surface-2 px-3 py-1 text-xs">
                  <Paperclip className="size-3 shrink-0 text-fg-subtle" aria-hidden />
                  <span className="min-w-0 flex-1 truncate font-medium text-fg">{f.name}</span>
                  <span className="shrink-0 tabular text-fg-muted">{(f.size / 1024).toFixed(0)} KB</span>
                  <IconButton size="sm" icon={X} label={`Remove ${f.name}`} onClick={() => setPendingFiles((prev) => prev.filter((_, idx) => idx !== i))} />
                </div>
              ))}
            </div>
          )}
        </div>
      </Dialog>

      <Dialog open={!!showSignModal} onOpenChange={(o) => { if (!o) setShowSignModal(null); }} title="Mark as signed"
        description={showSignModal?.number} size="sm"
        footer={<>
          <Button onClick={() => setShowSignModal(null)}>Cancel</Button>
          <Button variant="primary" loading={saving} onClick={markSigned}>{saving ? "Saving..." : "Confirm signed"}</Button>
        </>}>
        <div className="space-y-4">
          <Field label="Customer signatory name">
            <Input value={signForm.signed_by_customer_name} onChange={(e) => setSignForm({ ...signForm, signed_by_customer_name: e.target.value })} placeholder="e.g. John Smith" />
          </Field>
          <Field label="Customer signatory title">
            <Input value={signForm.signed_by_customer_title} onChange={(e) => setSignForm({ ...signForm, signed_by_customer_title: e.target.value })} placeholder="e.g. Owner" />
          </Field>
          <Field label="Our signatory name">
            <Input value={signForm.signed_by_company_name} onChange={(e) => setSignForm({ ...signForm, signed_by_company_name: e.target.value })} placeholder="e.g. Robert Smith" />
          </Field>
          <Field label="Document URL" hint="Optional.">
            <Input value={signForm.document_url} onChange={(e) => setSignForm({ ...signForm, document_url: e.target.value })} placeholder="https://..." />
          </Field>
        </div>
      </Dialog>
      <ErrorToast message={actionError} onClose={() => setActionError("")} />
    </div>
  );
}
