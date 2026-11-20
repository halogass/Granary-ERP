"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback, useRef } from "react";
import { financeApi, systemApi } from "@/lib/api";
import { Plus, Send, RotateCcw, Clock, CheckCircle2, Trash2, RefreshCw, X } from "lucide-react";
import { formatCurrency, CURRENCY_META, getCurrencySymbol } from "@/lib/currency";
import { extractApiError } from "@/lib/apiError";
import { stack } from "@/lib/stackTable";
import { cn } from "@/lib/utils";
import {
  Button, DataTable, Dialog, Field, FieldGrid, IconButton, Input, ListFrame, PageHeader, SavedViewTabs, Select,
  StatusPill, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface JournalEntry {
  id: string;
  entry_number: string;
  entry_date: string;
  narration: string;
  reference: string | null;
  status: "draft" | "posted" | "reversed";
  source: string | null;
  currency: string;
  exchange_rate?: number;
  total_debit: number;
  posted_at: string | null;
  reversed_at?: string | null;
  reversal_of_id?: string | null;
}

// A posted entry that has been reversed stays "posted" in the ledger (its reversal offsets
// it); legacy data may still carry status "reversed".
const isReversed = (je: JournalEntry) => je.status === "reversed" || !!je.reversed_at;
const displayStatus = (je: JournalEntry) => (isReversed(je) ? "reversed" : je.status);

interface JournalLine {
  id: string;
  account_id: string;
  account_code: string | null;
  account_name: string | null;
  description: string | null;
  debit: number;
  credit: number;
  currency: string;
}

interface Account {
  id: string;
  code: string;
  name: string;
  category: string;
}

interface JournalDetail extends JournalEntry {
  lines: JournalLine[];
}

type Filters = { status?: "draft" | "posted" | "reversed" };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All entries", filters: {} },
  { id: "draft", label: "Drafts", filters: { status: "draft" } },
  { id: "posted", label: "Posted", filters: { status: "posted" } },
  { id: "reversed", label: "Reversed", filters: { status: "reversed" } },
];

const fmtDay = (iso: string) =>
  new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });

export default function JournalPage() {
  const [entries, setEntries] = useState<JournalEntry[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [loading, setLoading] = useState(true);
  const views = useSavedViews<Filters>("finance.journal", VIEWS);
  const statusFilter = views.active?.filters.status ?? "all";
  const [expanded, setExpanded] = useState<string | null>(null);
  const [detail, setDetail] = useState<JournalDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [creating, setCreating] = useState(false);
  const [actionLoading, setActionLoading] = useState<string | null>(null);
  const [activeCurrencies, setActiveCurrencies] = useState<string[]>(["USD", "EUR"]);
  const [functionalCurrency, setFunctionalCurrency] = useState("USD");
  const [fetchingRate, setFetchingRate] = useState(false);

  // Create form state
  const [form, setForm] = useState({
    entry_date: new Date().toISOString().slice(0, 10),
    narration: "",
    reference: "",
    currency: "USD",
    exchange_rate: "",
  });
  const [lines, setLines] = useState([
    { account_id: "", description: "", debit: "", credit: "" },
    { account_id: "", description: "", debit: "", credit: "" },
  ]);
  const [createError, setCreateError] = useState("");
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const params: Record<string, unknown> = {};
      if (statusFilter !== "all") params.status = statusFilter;
      const [jRes, aRes, currRes] = await Promise.all([
        fetchAllPages<JournalEntry>(financeApi.listJournal, params),
        financeApi.listAccounts(),
        systemApi.getCurrencySettings().catch(() => null),
      ]);
      setEntries(jRes.rows);
      setAccounts(aRes.data ?? []);
      if (currRes?.data) {
        const active: string[] = currRes.data.active ?? ["USD", "EUR"];
        const functional: string = currRes.data.functional ?? "USD";
        setActiveCurrencies(active);
        setFunctionalCurrency(functional);
        setForm(f => ({ ...f, currency: functional }));
      }
    } catch { setEntries([]); } finally { setLoading(false); }
  }, [statusFilter]);

  useEffect(() => { load(); }, [load]);

  // Deep link: /finance/journal?entry=<id> opens that entry (added to the list when it falls
  // outside the loaded page), e.g. from a "Journal JE-..." link on an invoice or payroll run.
  const deepLinkDone = useRef(false);
  useEffect(() => {
    if (loading || deepLinkDone.current) return;
    deepLinkDone.current = true;
    const entryId = new URLSearchParams(window.location.search).get("entry");
    if (!entryId) return;
    setExpanded(entryId);
    setDetailLoading(true);
    financeApi.getJournalEntry(entryId)
      .then(res => {
        const d: JournalDetail = res.data;
        setDetail(d);
        setEntries(es => (es.some(e => e.id === d.id) ? es : [d, ...es]));
        setTimeout(() => document.querySelector(`[data-row-id="${d.id}"]`)?.scrollIntoView({ block: "center" }), 0);
      })
      .catch(() => setExpanded(null))
      .finally(() => setDetailLoading(false));
  }, [loading]);

  const fetchRate = useCallback(async (currency: string, entryDate: string, funcCur: string) => {
    if (currency === funcCur) { setForm(f => ({ ...f, exchange_rate: "" })); return; }
    setFetchingRate(true);
    try {
      const res = await financeApi.getSpotRate(currency, funcCur, entryDate);
      setForm(f => ({ ...f, exchange_rate: res.data?.rate ? String(res.data.rate) : "" }));
    } catch {
      setForm(f => ({ ...f, exchange_rate: "" }));
    } finally { setFetchingRate(false); }
  }, []);

  useEffect(() => {
    if (creating && form.currency !== functionalCurrency) {
      fetchRate(form.currency, form.entry_date, functionalCurrency);
    }
  }, [form.currency, form.entry_date, functionalCurrency, fetchRate, creating]);

  async function toggleExpand(id: string) {
    if (expanded === id) { setExpanded(null); setDetail(null); return; }
    setExpanded(id);
    setDetailLoading(true);
    try {
      const res = await financeApi.getJournalEntry(id);
      setDetail(res.data);
    } finally { setDetailLoading(false); }
  }

  async function postEntry(id: string) {
    setActionLoading(id);
    try { await financeApi.postJournalEntry(id); load(); } finally { setActionLoading(null); }
  }

  async function unpostEntry(id: string) {
    setActionLoading(id);
    try { await financeApi.unpostJournalEntry(id); load(); } finally { setActionLoading(null); }
  }

  async function reverseEntry(id: string) {
    setActionLoading(id);
    try {
      await financeApi.reverseJournalEntry(id, { reversal_date: new Date().toISOString().slice(0, 10) });
      load();
    } finally { setActionLoading(null); }
  }

  async function deleteEntry(id: string) {
    try {
      await financeApi.deleteJournalEntry(id);
      setConfirmDeleteId(null);
      setExpanded(null);
      setEntries((prev) => prev.filter((e) => e.id !== id));
    } catch { /* keep */ }
  }

  function addLine() {
    setLines(l => [...l, { account_id: "", description: "", debit: "", credit: "" }]);
  }

  function removeLine(idx: number) {
    setLines(l => l.filter((_, i) => i !== idx));
  }

  function updateLine(idx: number, field: string, value: string) {
    setLines(l => l.map((line, i) => i === idx ? { ...line, [field]: value } : line));
  }

  // Compared in cents: the server requires debits to equal credits exactly (no 0.01 tolerance).
  const cents = (v: string) => Math.round((parseFloat(v) || 0) * 100);
  const totalDebit = lines.reduce((s, l) => s + cents(l.debit), 0) / 100;
  const totalCredit = lines.reduce((s, l) => s + cents(l.credit), 0) / 100;
  const usedLines = lines.filter(l => l.account_id);
  const linesValid = usedLines.length >= 2 && usedLines.every(l => (cents(l.debit) > 0) !== (cents(l.credit) > 0)
    && cents(l.debit) >= 0 && cents(l.credit) >= 0);
  const balanced = Math.round(totalDebit * 100) === Math.round(totalCredit * 100) && totalDebit > 0 && linesValid;
  const isForeign = form.currency !== functionalCurrency;
  const rateNum = parseFloat(form.exchange_rate) || 0;

  async function submitCreate() {
    setSaving(true); setCreateError("");
    try {
      await financeApi.createJournalEntry({
        entry_date: form.entry_date,
        narration: form.narration,
        reference: form.reference || null,
        currency: form.currency,
        // Never 1 for a foreign entry: without a rate the server looks it up, or refuses with a 400.
        exchange_rate: isForeign && rateNum > 0 ? rateNum : undefined,
        lines: lines
          .filter(l => l.account_id)
          .map(l => ({
            account_id: l.account_id,
            description: l.description || null,
            // As typed (a string), so no float noise reaches the server's exact balance check.
            debit: l.debit.trim() || "0",
            credit: l.credit.trim() || "0",
            currency: form.currency,
          })),
      });
      setCreating(false);
      setForm({ entry_date: new Date().toISOString().slice(0, 10), narration: "", reference: "", currency: functionalCurrency, exchange_rate: "" });
      setLines([
        { account_id: "", description: "", debit: "", credit: "" },
        { account_id: "", description: "", debit: "", credit: "" },
      ]);
      load();
    } catch (e: unknown) {
      setCreateError(extractApiError(e, "Failed to save journal entry"));
    } finally { setSaving(false); }
  }

  const columns = useMemo<ColumnDef<JournalEntry>[]>(() => [
    { accessorKey: "entry_number", header: "Number", meta: { width: "130px" }, cell: (c) => <span className="font-mono text-[13px] font-medium">{c.getValue<string>()}</span> },
    {
      accessorKey: "narration", header: "Narration",
      cell: (c) => (
        <div className="min-w-0">
          <p className="truncate font-medium">{c.row.original.narration}</p>
          {c.row.original.reference && <p className="truncate text-xs text-fg-muted">Ref: {c.row.original.reference}</p>}
        </div>
      ),
    },
    { accessorKey: "entry_date", header: "Date", meta: { width: "130px", hideOnMobile: true }, cell: (c) => <span className="tabular">{c.getValue<string>() ? fmtDay(c.getValue<string>()) : "-"}</span> },
    {
      accessorKey: "total_debit", header: "Amount", meta: { numeric: true, width: "170px" },
      cell: (c) => {
        const je = c.row.original;
        return (
          <div>
            <span className="font-medium">{formatCurrency(je.total_debit, je.currency)}</span>
            {je.currency !== functionalCurrency && je.exchange_rate && (
              <div className="text-xs text-fg-muted">≈ {formatCurrency(je.total_debit * je.exchange_rate, functionalCurrency)}</div>
            )}
          </div>
        );
      },
    },
    { id: "status", header: "Status", accessorFn: displayStatus, meta: { width: "110px" }, cell: (c) => <StatusPill family="journal" status={c.getValue<string>()} /> },
  ], [functionalCurrency]);

  function renderDetails(je: JournalEntry) {
    if (detailLoading || !detail || detail.id !== je.id) {
      return <p className="py-2 text-sm text-fg-muted">{detailLoading ? "Loading..." : "No details"}</p>;
    }
    return (
      <div className="space-y-3">
        {detail.exchange_rate && detail.exchange_rate !== 1 && (
          <p className="text-xs text-fg-muted tabular">
            1 {detail.currency} = {getCurrencySymbol(functionalCurrency)} {Number(detail.exchange_rate).toLocaleString("en-US", { maximumFractionDigits: 6 })} (at entry date)
          </p>
        )}
        <div className="overflow-x-auto rounded-lg border border-line bg-surface max-md:overflow-visible max-md:border-0 max-md:bg-transparent">
          <table className={cn("w-full text-sm", stack.table)}>
            <thead className={stack.thead}>
              <tr className="bg-surface-2 text-[11px] font-medium uppercase tracking-[0.04em] text-fg-muted">
                <th className="px-3 py-2 text-left">Account</th>
                <th className="px-3 py-2 text-left">Description</th>
                <th className="px-3 py-2 text-right">Debit</th>
                <th className="px-3 py-2 text-right">Credit</th>
              </tr>
            </thead>
            <tbody className={cn(stack.tbody, "max-md:p-0")}>
              {detail.lines.map(l => {
                const lineForeign = l.currency !== functionalCurrency;
                const lineRate = detail.exchange_rate ?? 1;
                const amount = (v: number) => v > 0 ? (
                  <div>
                    <div>{formatCurrency(v, l.currency)}</div>
                    {lineForeign && lineRate !== 1 && (
                      <div className="text-xs font-normal text-fg-muted">≈ {formatCurrency(v * lineRate, functionalCurrency)}</div>
                    )}
                  </div>
                ) : "";
                return (
                  <tr key={l.id} className={cn("border-t border-line-soft", stack.tr)}>
                    <td className={cn("px-3 py-1.5 text-fg", stack.title)}>
                      <span className="font-mono text-xs">{l.account_code ?? "-"}</span> {l.account_name ?? "-"}
                    </td>
                    <td data-label="Description" className={cn("px-3 py-1.5 text-fg-muted", stack.td)}>{l.description || "-"}</td>
                    <td data-label="Debit" className={cn("px-3 py-1.5 text-right font-medium tabular", stack.td, !(l.debit > 0) && "max-md:hidden")}>{amount(l.debit)}</td>
                    <td data-label="Credit" className={cn("px-3 py-1.5 text-right font-medium tabular", stack.td, !(l.credit > 0) && "max-md:hidden")}>{amount(l.credit)}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
        <div className="flex flex-wrap justify-end gap-2">
          {je.status === "draft" && (
            <>
              {confirmDeleteId === je.id ? (
                <div className="flex items-center gap-2">
                  <span className="text-[13px] font-medium text-brand-ink">Delete this entry?</span>
                  <Button size="sm" variant="danger" onClick={() => deleteEntry(je.id)}>Yes, delete</Button>
                  <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
                </div>
              ) : (
                <Button size="sm" variant="danger" onClick={() => setConfirmDeleteId(je.id)}>
                  <Trash2 aria-hidden /> Delete
                </Button>
              )}
              <Button size="sm" variant="primary" onClick={() => postEntry(je.id)} disabled={actionLoading === je.id}>
                <Send aria-hidden /> Post
              </Button>
            </>
          )}
          {je.status === "posted" && !isReversed(je) && je.source !== "reversal" && (
            <>
              <Button size="sm" onClick={() => unpostEntry(je.id)} disabled={actionLoading === je.id}>
                <Clock aria-hidden /> Unpost
              </Button>
              <Button size="sm" onClick={() => reverseEntry(je.id)} disabled={actionLoading === je.id}>
                <RotateCcw aria-hidden /> Reverse
              </Button>
            </>
          )}
        </div>
      </div>
    );
  }

  const outOfBalance = Math.round(totalDebit * 100) !== Math.round(totalCredit * 100);

  return (
    <>
      <ListFrame>
        <PageHeader flush title="Journal entries" summary={`${entries.length} entr${entries.length === 1 ? "y" : "ies"}, double-entry journal of all financial transactions`}
          actions={<Button variant="primary" onClick={() => setCreating(true)}><Plus aria-hidden /> New entry</Button>} />
        <SavedViewTabs state={views} />
        <DataTable label="Journal entries" columns={columns} data={entries} getRowId={(je) => je.id} loading={loading}
          expandedId={expanded} onExpandedChange={(id) => { if (id) toggleExpand(id); else { setExpanded(null); setDetail(null); } }}
          empty={{ title: "No journal entries yet", description: "Create an entry, or pick another view." }}
          renderDetails={renderDetails} />
      </ListFrame>

      <Dialog open={creating} onOpenChange={setCreating} title="New journal entry" size="xl"
        footer={<>
          <Button onClick={() => setCreating(false)}>Cancel</Button>
          <Button variant="primary" onClick={submitCreate} loading={saving} disabled={!form.narration || !balanced}>
            {saving ? "Saving..." : "Save as draft"}
          </Button>
        </>}>
        <div className="space-y-4">
          <FieldGrid cols={4}>
            <Field label="Date" required>
              <Input type="date" value={form.entry_date} onChange={e => setForm(f => ({ ...f, entry_date: e.target.value }))} />
            </Field>
            <Field label="Narration" required className="md:col-span-2">
              <Input value={form.narration} onChange={e => setForm(f => ({ ...f, narration: e.target.value }))} placeholder="Description of this entry" />
            </Field>
            <Field label="Reference">
              <Input value={form.reference} onChange={e => setForm(f => ({ ...f, reference: e.target.value }))} placeholder="Invoice #, etc." />
            </Field>
            <Field label="Currency">
              <Select value={form.currency} onChange={e => setForm(f => ({ ...f, currency: e.target.value, exchange_rate: "" }))}>
                {activeCurrencies.map(c => (
                  <option key={c} value={c}>{c}: {CURRENCY_META[c as keyof typeof CURRENCY_META]?.name ?? c}</option>
                ))}
              </Select>
            </Field>
            {isForeign && (
              <Field label="Rate" hint={`1 ${form.currency} = ? ${functionalCurrency}`} className="md:col-span-2">
                <div className="flex items-center gap-2">
                  <Input type="number" numeric step="0.000001" min="0" value={form.exchange_rate}
                    onChange={e => setForm(f => ({ ...f, exchange_rate: e.target.value }))} placeholder="e.g. 15900" />
                  <IconButton variant="secondary" icon={RefreshCw} label="Fetch the rate" disabled={fetchingRate}
                    className={fetchingRate ? "[&_svg]:animate-spin" : undefined}
                    onClick={() => fetchRate(form.currency, form.entry_date, functionalCurrency)} />
                </div>
              </Field>
            )}
          </FieldGrid>

          <div className="overflow-x-auto rounded-lg border border-line max-md:overflow-visible max-md:border-0">
            <div className="md:min-w-[640px]">
              <div className="grid grid-cols-[2fr_2fr_1fr_1fr_36px] gap-2 bg-surface-2 px-3 py-2 max-md:hidden text-[11px] font-medium uppercase tracking-[0.04em] text-fg-muted">
                <span>Account</span><span>Description</span>
                <span className="text-right">Debit ({form.currency})</span>
                <span className="text-right">Credit ({form.currency})</span>
                <span />
              </div>
              {lines.map((line, idx) => (
                <div key={idx} className="grid grid-cols-[2fr_2fr_1fr_1fr_36px] items-center gap-2 border-t border-line-soft px-3 py-2 max-md:mb-2 max-md:grid-cols-2 max-md:rounded-lg max-md:border max-md:border-line max-md:p-3">
                  <p className="text-xs font-medium text-fg-muted max-md:order-[-2] md:hidden">Line {idx + 1}</p>
                  <div className="max-md:col-span-2">
                  <Select value={line.account_id} onChange={e => updateLine(idx, "account_id", e.target.value)} aria-label={`Line ${idx + 1} account`}>
                    <option value="">Select account</option>
                    {accounts.map(a => (
                      <option key={a.id} value={a.id}>{a.code} {a.name}</option>
                    ))}
                  </Select>
                  </div>
                  <Input value={line.description} onChange={e => updateLine(idx, "description", e.target.value)}
                    placeholder="Line description" aria-label={`Line ${idx + 1} description`} className="max-md:col-span-2" />
                  <div>
                    <span className="mb-1 block text-xs text-fg-muted md:hidden" aria-hidden>Debit ({form.currency})</span>
                    <Input type="number" numeric min="0" value={line.debit} onChange={e => updateLine(idx, "debit", e.target.value)}
                      placeholder="0" aria-label={`Line ${idx + 1} debit`} />
                  </div>
                  <div>
                    <span className="mb-1 block text-xs text-fg-muted md:hidden" aria-hidden>Credit ({form.currency})</span>
                    <Input type="number" numeric min="0" value={line.credit} onChange={e => updateLine(idx, "credit", e.target.value)}
                      placeholder="0" aria-label={`Line ${idx + 1} credit`} />
                  </div>
                  <IconButton size="sm" icon={X} label={`Remove line ${idx + 1}`} onClick={() => removeLine(idx)}
                    className="max-md:order-[-1] max-md:justify-self-end" />
                </div>
              ))}
              <div className="grid grid-cols-[2fr_2fr_1fr_1fr_36px] gap-2 border-t border-line bg-surface-2 px-3 py-2 max-md:grid-cols-2 max-md:rounded-lg max-md:border">
                <div className="col-span-2 flex flex-wrap items-center gap-3">
                  <Button size="sm" variant="ghost" onClick={addLine}><Plus aria-hidden /> Add line</Button>
                  {!balanced && totalDebit > 0 && (
                    <span className="text-xs font-medium text-tone-red">
                      {outOfBalance
                        ? `Out of balance by ${formatCurrency(Math.abs(totalDebit - totalCredit), form.currency)}`
                        : "Each line needs either a debit or a credit, and an entry needs at least two lines"}
                    </span>
                  )}
                  {balanced && (
                    <span className="flex items-center gap-1 text-xs font-medium text-tone-blue">
                      <CheckCircle2 className="size-3" aria-hidden /> Balanced
                    </span>
                  )}
                </div>
                <div className="text-right text-sm font-semibold text-fg tabular">
                  <span className="block text-xs font-normal text-fg-muted md:hidden">Total debit</span>
                  {formatCurrency(totalDebit, form.currency)}
                  {isForeign && form.exchange_rate && (
                    <div className="text-xs font-normal text-fg-muted">≈ {formatCurrency(totalDebit * rateNum, functionalCurrency)}</div>
                  )}
                </div>
                <div className="text-right text-sm font-semibold text-fg tabular">
                  <span className="block text-xs font-normal text-fg-muted md:hidden">Total credit</span>
                  {formatCurrency(totalCredit, form.currency)}
                  {isForeign && form.exchange_rate && (
                    <div className="text-xs font-normal text-fg-muted">≈ {formatCurrency(totalCredit * rateNum, functionalCurrency)}</div>
                  )}
                </div>
                <div className="max-md:hidden" />
              </div>
            </div>
          </div>

          {createError && <p role="alert" className="text-sm text-tone-red">{createError}</p>}
        </div>
      </Dialog>
    </>
  );
}
