"use client";

import { useEffect, useMemo, useState, useCallback } from "react";
import { financeApi } from "@/lib/api";
import { Plus, Landmark, FileText, Link2, CheckCircle2, Pencil, ArrowLeft } from "lucide-react";
import {
  Button, Card, CardHeader, Checkbox, DataTable, Dialog, EmptyState, Field, FieldGrid, IconButton, Input,
  PageHeader, Select, Skeleton, StatusPill, Textarea, toast, type ColumnDef,
} from "@/components/ui";
import { cn } from "@/lib/utils";
import { extractApiError } from "@/lib/apiError";
import { replaceUrlParams, useUrlParam } from "@/hooks/useUrlParam";

interface BankAccount {
  id: string;
  name: string;
  bank_name: string;
  account_number: string | null;
  currency: string;
  current_balance: number;
  is_active: boolean;
  gl_account_id: string | null;
  notes: string | null;
}

interface Statement {
  id: string;
  period_start: string | null;
  period_end: string | null;
  opening_balance: number;
  closing_balance: number;
  status: string;
  pdf_url: string | null;
}

interface StatementLine {
  id: string;
  transaction_date: string;
  description: string | null;
  reference: string | null;
  debit: number;
  credit: number;
  balance: number | null;
  is_matched: boolean;
  is_excluded: boolean;
  matched_journal_line_id: string | null;
}

// ─── Statement paste-import ──────────────────────────────────────────────────

type DecimalSep = "," | ".";

interface ParsedRow {
  transaction_date: string;
  description: string | null;
  reference: string | null;
  debit: number;    // money in
  credit: number;   // money out
  balance: number | null;
  error: string | null;
}

/** "1.234.567,89" (decimal comma) or "1,234,567.89" (decimal point) → number; null if unreadable. */
function parseAmount(raw: string | undefined, sep: DecimalSep): number | null {
  const s = (raw ?? "").replace(/[^\d.,()-]/g, "");
  if (!s) return 0;
  if (s.includes("(") || s.startsWith("-")) return null;     // signed amounts belong in the in/out columns
  const thousands = sep === "," ? "." : ",";
  const normalised = s.split(thousands).join("").replace(sep, ".");
  if (!/^\d+(\.\d+)?$/.test(normalised)) return null;
  return Number(normalised);
}

/** yyyy-mm-dd, or day first: dd/mm/yyyy, dd-mm-yyyy, dd.mm.yyyy (two-digit years are 20yy). */
function parseDate(raw: string | undefined): string | null {
  const s = (raw ?? "").trim();
  let y: number, m: number, d: number;
  let match = s.match(/^(\d{4})-(\d{1,2})-(\d{1,2})$/);
  if (match) { [y, m, d] = [Number(match[1]), Number(match[2]), Number(match[3])]; }
  else {
    match = s.match(/^(\d{1,2})[/.-](\d{1,2})[/.-](\d{2}|\d{4})$/);
    if (!match) return null;
    [d, m, y] = [Number(match[1]), Number(match[2]), Number(match[3])];
    if (y < 100) y += 2000;
  }
  const dt = new Date(Date.UTC(y, m - 1, d));
  if (dt.getUTCFullYear() !== y || dt.getUTCMonth() !== m - 1 || dt.getUTCDate() !== d) return null;
  return `${y}-${String(m).padStart(2, "0")}-${String(d).padStart(2, "0")}`;
}

/** Tab-separated rows: date, description, reference, debit, credit, balance. With crIsIn (the bank's
 *  own convention: DB = money out, CR = money in) the debit and credit columns are swapped. */
function parseStatementRows(text: string, sep: DecimalSep, crIsIn: boolean): ParsedRow[] {
  return text.split("\n").filter(l => l.trim()).map(line => {
    const parts = line.split("\t");
    const date = parseDate(parts[0]);
    const colDebit = parseAmount(parts[3], sep);
    const colCredit = parseAmount(parts[4], sep);
    const balance = parts[5]?.trim() ? parseAmount(parts[5], sep) : null;
    const [moneyIn, moneyOut] = crIsIn ? [colCredit, colDebit] : [colDebit, colCredit];
    let error: string | null = null;
    if (!date) error = `Unreadable date "${(parts[0] ?? "").trim()}"`;
    else if (moneyIn === null || moneyOut === null) error = "Unreadable amount";
    else if ((moneyIn > 0) === (moneyOut > 0)) error = "Needs exactly one of in / out";
    else if (parts[5]?.trim() && balance === null) error = "Unreadable balance";
    return {
      transaction_date: date ?? "", description: parts[1]?.trim() || null, reference: parts[2]?.trim() || null,
      debit: moneyIn ?? 0, credit: moneyOut ?? 0, balance, error,
    };
  });
}

interface GLAccount { id: string; code: string; name: string; }

interface CandidateLine {
  id: string;
  entry_number: string;
  entry_date: string;
  narration: string;
  description: string | null;
  debit: number;
  credit: number;
  currency: string;
}

const fmt = (n: number, cur = "USD") =>
  new Intl.NumberFormat("en-US", { style: "currency", currency: cur, minimumFractionDigits: 0 }).format(n);

export default function BankPage() {
  const [bankAccounts, setBankAccounts] = useState<BankAccount[]>([]);
  const [glAccounts, setGLAccounts] = useState<GLAccount[]>([]);
  const [loading, setLoading] = useState(true);
  const [selectedBank, setSelectedBank] = useState<BankAccount | null>(null);
  const [statements, setStatements] = useState<Statement[]>([]);
  const [selectedStmt, setSelectedStmt] = useState<Statement | null>(null);
  const [lines, setLines] = useState<StatementLine[]>([]);

  // Modals
  const [showNewBank, setShowNewBank] = useState(false);
  const [showNewStmt, setShowNewStmt] = useState(false);
  const EMPTY_BANK_FORM = { name: "", bank_name: "", account_number: "", currency: "USD", gl_account_id: "", notes: "", is_active: true };
  const [bankForm, setBankForm] = useState(EMPTY_BANK_FORM);
  const [editingBankId, setEditingBankId] = useState<string | null>(null);   // null = creating
  const [stmtForm, setStmtForm] = useState({ period_start: "", period_end: "", opening_balance: "0", closing_balance: "0" });
  const [saving, setSaving] = useState(false);

  // Bulk line entry
  const [showAddLines, setShowAddLines] = useState(false);
  const [rawLines, setRawLines] = useState("");
  const [decimalSep, setDecimalSep] = useState<DecimalSep>(",");
  const [crIsIn, setCrIsIn] = useState(true);
  const [preview, setPreview] = useState<ParsedRow[] | null>(null);

  // Match picker: posted GL lines on the bank's account with the same amount
  const [matchingLine, setMatchingLine] = useState<StatementLine | null>(null);
  const [candidates, setCandidates] = useState<CandidateLine[]>([]);
  const [candidatesLoading, setCandidatesLoading] = useState(false);

  const [migrating, setMigrating] = useState(false);
  const [migrateResult, setMigrateResult] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [bRes, aRes] = await Promise.all([
        financeApi.listBankAccounts(),
        financeApi.listAccounts({ category: "assets" }),
      ]);
      setBankAccounts(bRes.data ?? []);
      setGLAccounts(aRes.data ?? []);
    } catch { setBankAccounts([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  // The open account and statement are in the URL (?bank=&stmt=): on phones each is its own pane
  // and the phone Back button steps back out of it.
  const [bankParam, setBankParam] = useUrlParam("bank");
  const [stmtParam, setStmtParam] = useUrlParam("stmt");

  function selectBank(bank: BankAccount) {
    if (stmtParam) replaceUrlParams({ bank: bank.id, stmt: null });
    else setBankParam(bank.id);
    loadBank(bank);
  }

  function selectStatement(stmt: Statement) {
    setStmtParam(stmt.id);
    loadStatement(stmt);
  }

  useEffect(() => {
    if (!bankParam) {
      if (selectedBank) { setSelectedBank(null); setSelectedStmt(null); setLines([]); }
      return;
    }
    if (bankParam === selectedBank?.id) return;
    const bank = bankAccounts.find((b) => b.id === bankParam);
    if (bank) loadBank(bank);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [bankParam, bankAccounts]);

  useEffect(() => {
    if (!stmtParam) {
      if (selectedStmt) { setSelectedStmt(null); setLines([]); }
      return;
    }
    if (stmtParam === selectedStmt?.id) return;
    const stmt = statements.find((st) => st.id === stmtParam);
    if (stmt) loadStatement(stmt);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stmtParam, statements]);

  async function loadBank(bank: BankAccount) {
    setSelectedBank(bank);
    setSelectedStmt(null);
    setLines([]);
    try {
      const res = await financeApi.listStatements(bank.id);
      setStatements(res.data ?? []);
    } catch { setStatements([]); }
  }

  async function loadStatement(stmt: Statement) {
    setSelectedStmt(stmt);
    try {
      const res = await financeApi.listStatementLines(stmt.id);
      setLines(res.data ?? []);
    } catch { setLines([]); }
  }

  function openNewBank() {
    setEditingBankId(null);
    setBankForm(EMPTY_BANK_FORM);
    setShowNewBank(true);
  }

  function openEditBank(bank: BankAccount) {
    setEditingBankId(bank.id);
    setBankForm({
      name: bank.name, bank_name: bank.bank_name, account_number: bank.account_number ?? "",
      currency: bank.currency, gl_account_id: bank.gl_account_id ?? "", notes: bank.notes ?? "",
      is_active: bank.is_active,
    });
    setShowNewBank(true);
  }

  function closeBankModal() {
    setShowNewBank(false);
    setEditingBankId(null);
  }

  async function saveBank() {
    setSaving(true);
    const data = {
      name: bankForm.name, bank_name: bankForm.bank_name,
      account_number: bankForm.account_number || null,
      currency: bankForm.currency,
      gl_account_id: bankForm.gl_account_id || null,
      notes: bankForm.notes || null,
    };
    try {
      if (editingBankId) {
        await financeApi.updateBankAccount(editingBankId, { ...data, is_active: bankForm.is_active });
        // Keep the open account's header in step with the edit.
        setSelectedBank((b) => b && b.id === editingBankId ? { ...b, ...data, is_active: bankForm.is_active } : b);
      } else {
        await financeApi.createBankAccount(data);
      }
      closeBankModal();
      setBankForm(EMPTY_BANK_FORM);
      load();
    } catch (err) {
      toast.error(extractApiError(err, "Failed to save the bank account"));
    } finally { setSaving(false); }
  }

  async function createStatement() {
    if (!selectedBank) return;
    setSaving(true);
    try {
      const res = await financeApi.createStatement(selectedBank.id, {
        period_start: stmtForm.period_start,
        period_end: stmtForm.period_end,
        opening_balance: parseFloat(stmtForm.opening_balance) || 0,
        closing_balance: parseFloat(stmtForm.closing_balance) || 0,
      });
      setShowNewStmt(false);
      setStmtForm({ period_start: "", period_end: "", opening_balance: "0", closing_balance: "0" });
      const stmtsRes = await financeApi.listStatements(selectedBank.id);
      setStatements(stmtsRes.data ?? []);
      // Auto-select new statement
      if (res.data?.id) {
        const newStmt = (stmtsRes.data ?? []).find((s: Statement) => s.id === res.data.id);
        if (newStmt) selectStatement(newStmt);
      }
    } finally { setSaving(false); }
  }

  async function submitLines() {
    if (!selectedStmt) return;
    if (!preview || preview.length === 0 || preview.some(r => r.error)) return;
    setSaving(true);
    try {
      await financeApi.addStatementLines(selectedStmt.id, preview.map(({ error: _e, ...row }) => row));
      setShowAddLines(false); setRawLines(""); setPreview(null);
      const res = await financeApi.listStatementLines(selectedStmt.id);
      setLines(res.data ?? []);
    } catch (err) {
      toast.error(extractApiError(err, "Failed to import the lines"));
    } finally { setSaving(false); }
  }

  async function toggleExcluded(line: StatementLine) {
    if (!selectedStmt) return;
    try {
      await financeApi.excludeStatementLine(selectedStmt.id, line.id, !line.is_excluded);
      const res = await financeApi.listStatementLines(selectedStmt.id);
      setLines(res.data ?? []);
    } catch (err) {
      toast.error(extractApiError(err, "Failed to update the line"));
    }
  }

  async function matchLine(lineId: string, journalLineId: string | null) {
    if (!selectedStmt) return;
    try {
      await financeApi.matchStatementLine(selectedStmt.id, lineId, { journal_line_id: journalLineId });
      setMatchingLine(null);
      const res = await financeApi.listStatementLines(selectedStmt.id);
      setLines(res.data ?? []);
    } catch (err) {
      toast.error(extractApiError(err, "Failed to update match"));
    }
  }

  async function openMatchPicker(line: StatementLine) {
    if (!selectedStmt) return;
    setMatchingLine(line);
    setCandidates([]);
    setCandidatesLoading(true);
    try {
      const res = await financeApi.listCandidateLines(selectedStmt.id, line.id);
      setCandidates(res.data ?? []);
    } catch (err) {
      toast.error(extractApiError(err, "Failed to load matching journal lines"));
      setMatchingLine(null);
    } finally { setCandidatesLoading(false); }
  }

  async function reconcile() {
    if (!selectedStmt) return;
    try {
      await financeApi.reconcileStatement(selectedStmt.id);
    } catch (err) {
      toast.error(extractApiError(err, "The statement can't be reconciled"));
      return;
    }
    if (selectedBank) {
      const res = await financeApi.listStatements(selectedBank.id);
      setStatements(res.data ?? []);
      setSelectedStmt(s => s ? { ...s, status: "reconciled" } : null);
    }
  }

  async function runMigration() {
    setMigrating(true); setMigrateResult(null);
    try {
      const res = await financeApi.migrateOpeningEntries();
      setMigrateResult(res.data?.message ?? "Done");
    } catch (e: unknown) {
      const msg = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      setMigrateResult(`Error: ${msg ?? "Unknown"}`);
    } finally { setMigrating(false); }
  }

  const matchedCount = lines.filter(l => l.is_matched).length;
  const excludedCount = lines.filter(l => l.is_excluded).length;
  const unmatchedCount = lines.filter(l => !l.is_matched && !l.is_excluded).length;

  const cur = selectedBank?.currency ?? "USD";
  const reconciled = selectedStmt?.status === "reconciled";

  const stmtColumns = useMemo<ColumnDef<Statement>[]>(() => [
    {
      id: "period", header: "Period", accessorFn: (st) => st.period_start ?? "",
      cell: (c) => {
        const st = c.row.original;
        return (
          <span className="font-medium tabular">
            {st.period_start ? new Date(st.period_start).toLocaleDateString("en-GB", { month: "short", year: "numeric" }) : "-"} to{" "}
            {st.period_end ? new Date(st.period_end).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "-"}
          </span>
        );
      },
    },
    { accessorKey: "opening_balance", header: "Opening", meta: { numeric: true, hideOnMobile: true }, cell: (c) => fmt(c.getValue<number>(), cur) },
    { accessorKey: "closing_balance", header: "Closing", meta: { numeric: true }, cell: (c) => fmt(c.getValue<number>(), cur) },
    { accessorKey: "status", header: "Status", meta: { width: "120px" }, cell: (c) => <StatusPill family="bank_statement" status={c.getValue<string>()} /> },
  ], [cur]);

  const lineColumns = useMemo<ColumnDef<StatementLine>[]>(() => [
    {
      accessorKey: "transaction_date", header: "Date", meta: { width: "90px" },
      cell: (c) => <span className="tabular text-fg-muted">{new Date(c.getValue<string>()).toLocaleDateString("en-GB", { day: "numeric", month: "short" })}</span>,
    },
    {
      accessorKey: "description", header: "Description",
      cell: (c) => (
        <div className="min-w-0">
          <p className={cn("truncate font-medium", c.row.original.is_excluded && "text-fg-muted")}>{c.row.original.description || "-"}</p>
          {c.row.original.reference && <p className="truncate text-xs text-fg-muted">{c.row.original.reference}</p>}
        </div>
      ),
    },
    { accessorKey: "debit", header: "In", meta: { numeric: true }, cell: (c) => c.getValue<number>() > 0 ? <span className="font-medium">{fmt(c.getValue<number>(), cur)}</span> : "" },
    { accessorKey: "credit", header: "Out", meta: { numeric: true }, cell: (c) => c.getValue<number>() > 0 ? <span className="font-medium">{fmt(c.getValue<number>(), cur)}</span> : "" },
    {
      accessorKey: "balance", header: "Balance", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<number | null>() !== null ? fmt(c.getValue<number>(), cur) : "-"}</span>,
    },
    {
      id: "match", header: "Match", enableSorting: false, meta: { width: "170px" },
      cell: (c) => {
        const l = c.row.original;
        if (reconciled) {
          return <span className="text-xs text-fg-muted">{l.is_matched ? "Matched" : l.is_excluded ? "Excluded" : "-"}</span>;
        }
        if (l.is_matched) {
          return (
            <Button size="sm" variant="ghost" onClick={() => matchLine(l.id, null)} title="Unmatch">
              <CheckCircle2 aria-hidden className="text-tone-blue" /> Matched
            </Button>
          );
        }
        if (l.is_excluded) {
          return <Button size="sm" variant="ghost" onClick={() => toggleExcluded(l)} title="Include in matching again">Excluded, undo</Button>;
        }
        return (
          <div className="flex items-center gap-1">
            <Button size="sm" variant="ghost" onClick={() => openMatchPicker(l)}><Link2 aria-hidden /> Match</Button>
            <Button size="sm" variant="ghost" onClick={() => toggleExcluded(l)}
              title="No ledger counterpart to match; the line still counts toward the closing balance">Exclude</Button>
          </div>
        );
      },
    },
  // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [cur, reconciled, selectedStmt]);

  const migrateFailed = migrateResult?.startsWith("Error");

  return (
    <div>
      <PageHeader title="Bank reconciliation" summary="Match bank statement lines to journal entries"
        actions={<>
          <Button onClick={runMigration} loading={migrating}>{migrating ? "Migrating..." : "Migrate opening entries"}</Button>
          <Button variant="primary" onClick={openNewBank}><Plus aria-hidden /> Add bank account</Button>
        </>} />

      {migrateResult && (
        <div role="status" className={cn("mb-4 rounded-lg border border-line p-3 text-sm font-medium",
          migrateFailed ? "bg-tone-red-soft text-tone-red" : "bg-tone-blue-soft text-tone-blue")}>
          {migrateResult}
        </div>
      )}

      <div className="grid grid-cols-1 gap-4 md:grid-cols-[260px_1fr]">
        <Card className={cn("self-start", selectedBank && "max-md:hidden")}>
          <CardHeader title="Bank accounts" />
          {loading ? (
            <div className="space-y-2 p-3">{[0, 1, 2].map(i => <Skeleton key={i} className="h-16" />)}</div>
          ) : bankAccounts.length === 0 ? (
            <EmptyState compact icon={Landmark} title="No bank accounts" description="Add a bank account to start reconciling." />
          ) : (
            <ul className="divide-y divide-line-soft">
              {bankAccounts.map(bank => {
                const on = selectedBank?.id === bank.id;
                return (
                  <li key={bank.id} className="relative">
                    <button type="button" onClick={() => selectBank(bank)} aria-current={on || undefined}
                      className={cn("w-full py-2.5 pl-4 pr-12 text-left transition-colors",
                        on ? "bg-brand/[0.07] shadow-[inset_2px_0_0_rgb(var(--brand))]" : "hover:bg-surface-2",
                        !bank.is_active && "opacity-60")}>
                      <p className="truncate text-sm font-semibold text-fg">{bank.name}</p>
                      <p className="text-xs text-fg-muted">{bank.bank_name}{!bank.is_active && " · Inactive"}</p>
                      {bank.account_number && <p className="mt-0.5 font-mono text-xs text-fg-muted">{bank.account_number}</p>}
                      <p className="mt-1 text-[13px] font-medium text-fg tabular">{fmt(bank.current_balance, bank.currency)}</p>
                    </button>
                    <IconButton size="sm" icon={Pencil} label={`Edit ${bank.name}`} onClick={() => openEditBank(bank)}
                      className="absolute right-2 top-2" />
                  </li>
                );
              })}
            </ul>
          )}
        </Card>

        <div className={cn("min-w-0 space-y-4", !selectedBank && "max-md:hidden")}>
          {!selectedBank ? (
            <Card>
              <EmptyState icon={Landmark} title="Select a bank account" description="Its statements and transactions show here." />
            </Card>
          ) : (
            <>
              <div className="md:hidden">
                {selectedStmt ? (
                  <Button variant="ghost" className="-ml-2" onClick={() => setStmtParam(null)}>
                    <ArrowLeft aria-hidden /> {selectedBank.name}: statements
                  </Button>
                ) : (
                  <Button variant="ghost" className="-ml-2" onClick={() => setBankParam(null)}>
                    <ArrowLeft aria-hidden /> Bank accounts
                  </Button>
                )}
              </div>
              <Card className={cn("overflow-hidden", selectedStmt && "max-md:hidden")}>
                <CardHeader title={`${selectedBank.name}: statements`}
                  actions={<Button size="sm" onClick={() => setShowNewStmt(true)}><Plus aria-hidden /> New statement</Button>} />
                <DataTable label="Statements" columns={stmtColumns} data={statements} getRowId={(st) => st.id}
                  maxHeight="none" selectedId={selectedStmt?.id ?? null} onRowClick={(st) => selectStatement(st)}
                  empty={{ icon: FileText, title: "No statements uploaded", description: "Create a statement for a period, then add its lines.", compact: true }} />
              </Card>

              {selectedStmt && (
                <Card className="overflow-hidden">
                  <CardHeader title="Transactions"
                    description={<span className="tabular">{matchedCount} matched{excludedCount > 0 && `, ${excludedCount} excluded`}{unmatchedCount > 0 && `, ${unmatchedCount} unmatched`}</span>}
                    actions={<>
                      {!reconciled && (
                        <Button size="sm" onClick={() => { setShowAddLines(true); setPreview(null); }}><Plus aria-hidden /> Add lines</Button>
                      )}
                      {!reconciled && unmatchedCount === 0 && lines.length > 0 && (
                        <Button size="sm" variant="primary" onClick={reconcile}><CheckCircle2 aria-hidden /> Mark reconciled</Button>
                      )}
                    </>} />
                  <DataTable label="Statement transactions" columns={lineColumns} data={lines} getRowId={(l) => l.id} maxHeight="20rem"
                    empty={{ title: "No transactions", description: "Add lines to start reconciling.", compact: true }} />
                </Card>
              )}
            </>
          )}
        </div>
      </div>

      <Dialog open={showNewBank} onOpenChange={(o) => { if (!o) closeBankModal(); }}
        title={editingBankId ? "Edit bank account" : "New bank account"}
        footer={<>
          <Button onClick={closeBankModal}>Cancel</Button>
          <Button variant="primary" onClick={saveBank} loading={saving} disabled={!bankForm.name || !bankForm.bank_name}>
            {saving ? "Saving..." : editingBankId ? "Save bank account" : "Create bank account"}
          </Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Account name" required className="md:col-span-2">
            <Input value={bankForm.name} onChange={e => setBankForm(f => ({ ...f, name: e.target.value }))} placeholder="Main bank, USD operations" />
          </Field>
          <Field label="Bank name" required>
            <Input value={bankForm.bank_name} onChange={e => setBankForm(f => ({ ...f, bank_name: e.target.value }))} placeholder="First National Bank" />
          </Field>
          <Field label="Currency">
            <Input value={bankForm.currency} onChange={e => setBankForm(f => ({ ...f, currency: e.target.value.toUpperCase() }))}
              maxLength={3} className="font-mono uppercase" />
          </Field>
          <Field label="Account number" className="md:col-span-2">
            <Input value={bankForm.account_number} onChange={e => setBankForm(f => ({ ...f, account_number: e.target.value }))}
              placeholder="1234567890" className="font-mono" />
          </Field>
          <Field label="GL account" hint="From the chart of accounts." className="md:col-span-2">
            <Select value={bankForm.gl_account_id} onChange={e => setBankForm(f => ({ ...f, gl_account_id: e.target.value }))}>
              <option value="">Link to account</option>
              {glAccounts.map(a => <option key={a.id} value={a.id}>{a.code} {a.name}</option>)}
            </Select>
          </Field>
          <Field label="Notes" className="md:col-span-2">
            <Textarea value={bankForm.notes} onChange={e => setBankForm(f => ({ ...f, notes: e.target.value }))} rows={2} placeholder="Optional notes" />
          </Field>
          {editingBankId && (
            <div className="md:col-span-2">
              <Checkbox checked={bankForm.is_active} onChange={e => setBankForm(f => ({ ...f, is_active: e.target.checked }))}
                label="Active" hint="Inactive accounts are kept for history but not offered for new payments." />
            </div>
          )}
        </FieldGrid>
      </Dialog>

      <Dialog open={!!matchingLine && !!selectedBank} onOpenChange={(o) => { if (!o) setMatchingLine(null); }}
        title="Match transaction"
        description={matchingLine ? `${matchingLine.description || "-"} · ${fmt(matchingLine.debit > 0 ? matchingLine.debit : matchingLine.credit, cur)} ${matchingLine.debit > 0 ? "in" : "out"}` : undefined}
        footer={<Button onClick={() => setMatchingLine(null)}>Cancel</Button>}>
        <div className="max-h-80 divide-y divide-line-soft overflow-y-auto rounded-lg border border-line">
          {candidatesLoading ? (
            <p className="p-6 text-center text-sm text-fg-muted">Loading...</p>
          ) : candidates.length === 0 ? (
            <p className="p-6 text-center text-sm text-fg-muted">
              No posted journal lines on this bank&apos;s GL account with the same amount.
            </p>
          ) : matchingLine && candidates.map(c => (
            <button key={c.id} type="button" onClick={() => matchLine(matchingLine.id, c.id)}
              className="flex w-full items-center gap-3 px-4 py-2.5 text-left hover:bg-surface-2">
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm text-fg">{c.narration}</p>
                <p className="text-xs text-fg-muted">
                  <span className="font-mono">{c.entry_number}</span> · <span className="tabular">{new Date(c.entry_date).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" })}</span>
                </p>
              </div>
              <span className="text-sm font-medium text-fg tabular">{fmt(c.debit > 0 ? c.debit : c.credit, c.currency)}</span>
            </button>
          ))}
        </div>
      </Dialog>

      <Dialog open={showNewStmt} onOpenChange={setShowNewStmt} title="New statement" size="sm"
        footer={<>
          <Button onClick={() => setShowNewStmt(false)}>Cancel</Button>
          <Button variant="primary" onClick={createStatement} loading={saving} disabled={!stmtForm.period_start || !stmtForm.period_end}>
            {saving ? "Creating..." : "Create statement"}
          </Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Period start">
            <Input type="date" value={stmtForm.period_start} onChange={e => setStmtForm(f => ({ ...f, period_start: e.target.value }))} />
          </Field>
          <Field label="Period end">
            <Input type="date" value={stmtForm.period_end} onChange={e => setStmtForm(f => ({ ...f, period_end: e.target.value }))} />
          </Field>
          <Field label="Opening balance">
            <Input type="number" numeric value={stmtForm.opening_balance} onChange={e => setStmtForm(f => ({ ...f, opening_balance: e.target.value }))} />
          </Field>
          <Field label="Closing balance">
            <Input type="number" numeric value={stmtForm.closing_balance} onChange={e => setStmtForm(f => ({ ...f, closing_balance: e.target.value }))} />
          </Field>
        </FieldGrid>
      </Dialog>

      <Dialog open={showAddLines} onOpenChange={(o) => { if (!o) { setShowAddLines(false); setPreview(null); } }}
        title="Add statement lines" size="lg"
        description={<>
          Paste tab-separated rows: <code className="font-mono text-xs">date</code>, <code className="font-mono text-xs">description</code>,{" "}
          <code className="font-mono text-xs">reference</code>, <code className="font-mono text-xs">debit</code>,{" "}
          <code className="font-mono text-xs">credit</code>, <code className="font-mono text-xs">balance</code>. Dates as dd/mm/yyyy or yyyy-mm-dd.
        </>}
        footer={<>
          <Button onClick={() => { setShowAddLines(false); setPreview(null); }}>Cancel</Button>
          {!preview ? (
            <Button variant="primary" onClick={() => setPreview(parseStatementRows(rawLines, decimalSep, crIsIn))} disabled={!rawLines.trim()}>
              Preview
            </Button>
          ) : (
            <Button variant="primary" onClick={submitLines} loading={saving} disabled={preview.length === 0 || preview.some(r => r.error)}>
              {saving ? "Importing..." : preview.some(r => r.error)
                ? `Fix ${preview.filter(r => r.error).length} row(s) first`
                : `Import ${preview.length} line(s)`}
            </Button>
          )}
        </>}>
        <div className="space-y-3">
          <FieldGrid cols={2}>
            <Field label="Number format">
              <Select value={decimalSep} onChange={e => { setDecimalSep(e.target.value as DecimalSep); setPreview(null); }}
                options={[{ value: ",", label: "1.234.567,89 (decimal comma)" }, { value: ".", label: "1,234,567.89 (decimal point)" }]} />
            </Field>
            <div className="self-end pb-1">
              <Checkbox checked={crIsIn} onChange={e => { setCrIsIn(e.target.checked); setPreview(null); }}
                label="CR = money in" hint="The bank's convention: the debit column is money out." />
            </div>
          </FieldGrid>
          <Field label="Rows">
            <Textarea value={rawLines} onChange={e => { setRawLines(e.target.value); setPreview(null); }}
              rows={preview ? 4 : 10} className="resize-none font-mono"
              placeholder={"01/05/2026\tPayment from Client A\tINV-001\t0\t5.000.000\t25.000.000\n03/05/2026\tRent payment\t\t3.000.000\t0\t22.000.000"} />
          </Field>
          {preview && (
            <div className="max-h-64 overflow-y-auto rounded-lg border border-line">
              <table className="w-full text-xs">
                <thead className="sticky top-0 bg-surface-2 uppercase text-fg-muted">
                  <tr>
                    <th className="px-3 py-1.5 text-left font-medium">Date</th><th className="px-3 py-1.5 text-left font-medium">Description</th>
                    <th className="px-3 py-1.5 text-right font-medium">In</th><th className="px-3 py-1.5 text-right font-medium">Out</th>
                    <th className="px-3 py-1.5 text-right font-medium">Balance</th>
                  </tr>
                </thead>
                <tbody>
                  {preview.map((r, i) => (
                    <tr key={i} className={cn("border-t border-line-soft", r.error && "bg-tone-red-soft")}>
                      <td className="px-3 py-1.5 tabular">{r.transaction_date || "-"}</td>
                      <td className="px-3 py-1.5 text-fg">
                        {r.description || "-"}
                        {r.error && <span className="block font-semibold text-tone-red">{r.error}</span>}
                      </td>
                      <td className="px-3 py-1.5 text-right tabular">{r.debit > 0 && selectedBank ? fmt(r.debit, selectedBank.currency) : ""}</td>
                      <td className="px-3 py-1.5 text-right tabular">{r.credit > 0 && selectedBank ? fmt(r.credit, selectedBank.currency) : ""}</td>
                      <td className="px-3 py-1.5 text-right text-fg-muted tabular">{r.balance !== null && selectedBank ? fmt(r.balance, selectedBank.currency) : "-"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      </Dialog>
    </div>
  );
}
