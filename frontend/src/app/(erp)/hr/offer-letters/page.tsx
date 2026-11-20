"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { FileSignature, Plus, Search } from "lucide-react";
import { hrApi } from "@/lib/api";
import { formatCurrency } from "@/lib/currency";
import { extractApiError } from "@/lib/apiError";
import OfferLetterFormModal, { OFFER_STATUS_LABELS, type OfferLetter } from "@/components/hr/OfferLetterFormModal";
import { ApprovalStatusBadge } from "@/components/approvals/ApprovalTrail";
import {
  Button, DataTable, FilterChips, Input, KeyValues, ListFrame, PageHeader, SavedViewTabs, Stat, StatGroup,
  StatusPill, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "draft", label: "Drafts", filters: { status: "draft" } },
  { id: "waiting", label: "Awaiting answer", filters: { status: "waiting" } },
  { id: "accepted", label: "Accepted", filters: { status: "accepted" } },
];

const CHIP_STATUSES = ["declined", "expired", "cancelled"];

const fmtDate = (d?: string | null) => (d ? new Date(d).toLocaleDateString("en-GB") : "-");

export default function OfferLettersPage() {
  const router = useRouter();
  const views = useSavedViews<Filters>("hr.offer-letters", VIEWS);
  const [chipStatus, setChipStatus] = useState("");
  const [search, setSearch] = useState("");
  const [offers, setOffers] = useState<OfferLetter[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [creating, setCreating] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await hrApi.listOfferLetters();
      setOffers(res.data ?? []);
      setError("");
    } catch (e) {
      setOffers([]);
      setError(extractApiError(e, "Could not load offer letters."));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { load(); }, [load]);

  const count = (s: string) => offers.filter((o) => o.status === s).length;
  const status = chipStatus || views.active?.filters.status || "";
  const filters: Filters = status ? { status } : {};
  const q = search.trim().toLowerCase();
  const visible = offers.filter((o) => {
    if (status === "waiting" ? !["sent", "countered"].includes(o.status) : status && o.status !== status) return false;
    if (!q) return true;
    return [o.number, o.applicant_name, o.position_title, o.department].some((v) => (v ?? "").toLowerCase().includes(q));
  });

  const columns = useMemo<ColumnDef<OfferLetter>[]>(() => [
    {
      accessorKey: "number", header: "Offer",
      cell: (c) => <span className="font-mono text-[13px] text-fg-muted">{c.row.original.number}</span>,
    },
    {
      id: "candidate", accessorFn: (o) => o.applicant_name ?? "", header: "Candidate",
      cell: (c) => (
        <div className="min-w-0">
          <p className="max-w-56 truncate font-medium text-fg">{c.row.original.applicant_name ?? "-"}</p>
          <p className="max-w-56 truncate text-xs text-fg-muted">{c.row.original.position_title}</p>
        </div>
      ),
    },
    { accessorKey: "department", header: "Department", meta: { hideOnMobile: true }, cell: (c) => c.row.original.department ?? "-" },
    {
      accessorKey: "start_date", header: "Starts", meta: { hideOnMobile: true },
      cell: (c) => <span className="tabular">{fmtDate(c.row.original.start_date)}</span>,
    },
    {
      accessorKey: "salary_gross_monthly", header: "Gross / month", meta: { numeric: true },
      cell: (c) => c.row.original.salary_gross_monthly != null
        ? formatCurrency(c.row.original.salary_gross_monthly, c.row.original.salary_currency) : "-",
    },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => {
        const o = c.row.original;
        return (
          <div className="flex flex-wrap items-center gap-1.5">
            <StatusPill family="offer" status={o.status} label={OFFER_STATUS_LABELS[o.status]} />
            {o.status === "draft" && o.approval && <ApprovalStatusBadge status={o.approval.status} />}
          </div>
        );
      },
    },
  ], []);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Drafts" value={count("draft")} note="not issued yet" />
        <Stat label="Awaiting answer" value={count("sent") + count("countered")} note={count("countered") ? `${count("countered")} countered` : "sent to candidates"} alert={count("countered") > 0} />
        <Stat label="Accepted" value={count("accepted")} />
        <Stat label="Declined" value={count("declined")} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Offer letters" summary="Drafted by HR, approved by the Owner, issued to the candidate"
          actions={<Button variant="primary" onClick={() => setCreating(true)}><Plus aria-hidden /> New offer</Button>} />
        <SavedViewTabs state={views} currentFilters={filters} onSelect={() => setChipStatus("")} />
        <FilterChips
          chips={CHIP_STATUSES.map((s) => ({ id: s, label: OFFER_STATUS_LABELS[s], active: chipStatus === s }))}
          onToggle={(id) => setChipStatus((cur) => (cur === id ? "" : id))} onClear={() => setChipStatus("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search offers" aria-label="Search offers" className="w-56" />
        </FilterChips>
        {error && <p role="alert" className="border-b border-line px-4 py-2 text-[13px] text-tone-red">{error}</p>}
        <DataTable label="Offer letters" columns={columns} data={visible} getRowId={(o) => o.id} loading={loading}
          empty={{
            icon: FileSignature, title: "No offer letters here", description: "Pick another view, or draft a new offer.",
            action: <Button variant="primary" onClick={() => setCreating(true)}>New offer</Button>,
          }}
          renderDetails={(o) => (
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                { label: "Candidate", value: o.applicant_name ?? "-" },
                { label: "Email", value: o.applicant_email ?? "-" },
                { label: "Position", value: o.position_title },
                { label: "Department", value: o.department ?? "-" },
                { label: "Employment type", value: <span className="capitalize">{o.employment_type?.replace("_", " ") ?? "-"}</span> },
                { label: "Gross / month", value: o.salary_gross_monthly != null ? formatCurrency(o.salary_gross_monthly, o.salary_currency) : "-", numeric: true },
                { label: "Starts", value: fmtDate(o.start_date) },
                { label: "Respond by", value: fmtDate(o.acceptance_deadline) },
                { label: "Issued", value: o.sent_at ? fmtDate(o.sent_at) : "Not yet" },
              ]} />
              <Button size="sm" asChild><Link href={`/hr/offer-letters/${o.id}`}>Open offer</Link></Button>
            </div>
          )} />
      </ListFrame>

      {creating && (
        <OfferLetterFormModal
          onClose={() => setCreating(false)}
          onSaved={(offer) => { setCreating(false); router.push(`/hr/offer-letters/${offer.id}`); }}
        />
      )}
    </div>
  );
}
