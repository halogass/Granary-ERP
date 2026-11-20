"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useCallback, useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { FileText, Plus, Search } from "lucide-react";
import { salesDocumentsApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { usePermissions } from "@/hooks/usePermissions";
import {
  Button, DataTable, Field, FilterChips, Input, KeyValues, ListFrame, PageHeader, SavedViewTabs, Select,
  StatusPill, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface Doc { id: string; full_number: string | null; number: string; title: string; status: string; language: string; deal_id: string | null; origin: string | null; created_at: string }
const STATUS: Record<string, string> = { draft: "Draft", review: "In review", approved: "Approved", sent: "Sent", superseded: "Superseded", rejected: "Rejected" };

type Filters = { status?: string };
const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "draft", label: "Drafts", filters: { status: "draft" } },
  { id: "review", label: "In review", filters: { status: "review" } },
  { id: "approved", label: "Approved", filters: { status: "approved" } },
  { id: "sent", label: "Sent", filters: { status: "sent" } },
  { id: "rejected", label: "Rejected", filters: { status: "rejected" } },
];

const fmtDate = (d?: string | null) => (d ? new Date(d).toLocaleDateString("en-GB") : "-");

export default function ProposalsPage() {
  const router = useRouter();
  const { can } = usePermissions();
  const views = useSavedViews<Filters>("sales.proposals", VIEWS);
  const [rows, setRows] = useState<Doc[] | null>(null);
  const [title, setTitle] = useState("");
  const [language, setLanguage] = useState("en");
  const [notice, setNotice] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [byNary, setByNary] = useState(false);
  const dealId = typeof window !== "undefined" ? new URLSearchParams(window.location.search).get("deal_id") : null;

  const load = useCallback(async () => {
    try { setRows((await fetchAllPages<Doc>(salesDocumentsApi.list, dealId ? { deal_id: dealId } : {})).rows); } catch (e) { setNotice(extractApiError(e)); }
  }, [dealId]);
  useEffect(() => { load(); }, [load]);

  const create = async () => {
    try {
      const res = await salesDocumentsApi.create({ title: title.trim(), language, deal_id: dealId || undefined });
      router.push(`/sales/proposals/${res.data.id}`);
    } catch (e) { setNotice(extractApiError(e)); }
  };

  const status = views.active?.filters.status;
  const q = search.trim().toLowerCase();
  const visible = (rows ?? []).filter((d) =>
    (!status || d.status === status) && (!byNary || d.origin === "nary") &&
    (!q || d.title.toLowerCase().includes(q) || (d.full_number || d.number).toLowerCase().includes(q)));

  const columns = useMemo<ColumnDef<Doc>[]>(() => [
    { id: "number", accessorFn: (d) => d.full_number || d.number, header: "Number", meta: { width: "14rem" },
      cell: (c) => <span className="font-mono text-[13px] text-fg-muted">{c.getValue<string>()}</span> },
    { accessorKey: "title", header: "Title", cell: (c) => <span className="font-medium text-fg">{c.getValue<string>()}</span> },
    { accessorKey: "origin", header: "By", meta: { hideOnMobile: true },
      cell: (c) => (c.getValue<string>() === "nary" ? <span className="text-xs text-fg-muted">by Nary</span> : null) },
    { accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="proposal" status={c.getValue<string>()} label={STATUS[c.getValue<string>()]} /> },
    { accessorKey: "created_at", header: "Created", meta: { numeric: true, hideOnMobile: true }, cell: (c) => fmtDate(c.getValue<string>()) },
  ], []);

  return (
    <div className="space-y-4">
      {can("sales:create_quotation") && (
        <div className="flex flex-wrap items-end gap-3 rounded-lg border border-line bg-surface p-4">
          <Field label={`New proposal${dealId ? " for this deal" : ""}`} className="min-w-[16rem] flex-1">
            <Input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="Title, e.g. Office Furniture Rollout" autoComplete="off" />
          </Field>
          <Field label="Language">
            <Select value={language} onChange={(e) => setLanguage(e.target.value)}
              options={[{ value: "en", label: "English" }]} />
          </Field>
          <Button variant="primary" disabled={!title.trim()} onClick={create}><Plus aria-hidden /> Create</Button>
        </div>
      )}
      {notice && <p className="text-sm text-tone-red" role="alert">{notice}</p>}

      <ListFrame>
        <PageHeader flush title="Solution proposals"
          summary={`${visible.length} proposal${visible.length !== 1 ? "s" : ""}. Custom, tender and complex deals: a proposal with its quotation(s), approved and sent together.`} />
        <SavedViewTabs state={views} currentFilters={status ? { status } : {}} />
        <FilterChips chips={[{ id: "nary", label: "Drafted by Nary", active: byNary }]}
          onToggle={() => setByNary((v) => !v)} onClear={() => setByNary(false)}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search proposals" aria-label="Search proposals" className="w-56" />
        </FilterChips>
        <DataTable label="Proposals" columns={columns} data={visible} getRowId={(d) => d.id} loading={rows === null}
          empty={{ icon: FileText, title: "No proposals yet", description: "Nary drafts them from requirement discovery, or create one above." }}
          renderDetails={(d) => (
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                { label: "Number", value: <span className="font-mono">{d.full_number || d.number}</span> },
                { label: "Language", value: d.language === "en" || !d.language ? "English" : d.language },
                { label: "Created", value: fmtDate(d.created_at) },
                { label: "Origin", value: d.origin === "nary" ? "Nary" : "Manual" },
                { label: "Deal", value: d.deal_id ? <Link className="text-brand-ink hover:underline" href={`/crm/deals/${d.deal_id}`}>Open deal</Link> : "-" },
              ]} />
              <Button size="sm" asChild><Link href={`/sales/proposals/${d.id}`}>Open proposal</Link></Button>
            </div>
          )} />
      </ListFrame>
    </div>
  );
}
