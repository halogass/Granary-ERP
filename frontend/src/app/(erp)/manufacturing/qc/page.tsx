"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { manufacturingApi } from "@/lib/api";
import { ShieldCheck, Plus } from "lucide-react";
import { extractApiError } from "@/lib/apiError";
import { cn } from "@/lib/utils";
import {
  Button, DataTable, Dialog, Field, Input, KeyValues, ListFrame, PageHeader, SavedViewTabs, Select, Stat,
  StatGroup, StatusPill, Textarea, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface ChecklistItem { item: string; result: "pass" | "fail" }

// Shape of GET /manufacturing/qc-records.
interface QCRecord {
  id: string;
  unit_id: string;
  serial_number: string | null;
  product?: { name: string; product_code: string } | null;
  inspector_name?: string | null;
  result: string;
  checklist_items?: ChecklistItem[] | null;
  notes?: string | null;
  checked_at: string | null;
  created_at: string;
}

interface ProductionUnit {
  id: string;
  serial_number: string;
  status: string;
  product?: { sku: string; name: string } | null;
}

const RESULT_LABELS: Record<string, string> = { pass: "Pass", fail: "Fail", conditional: "Passed with conditions" };

const DEFAULT_CHECKS = ["Power on test", "Function test", "Burn-in test", "Enclosure & cosmetics"];
const emptyForm = () => ({
  unit_id: "",
  overall_result: "pass",
  notes: "",
  checklist: DEFAULT_CHECKS.map((item) => ({ item, result: "pass" as const })) as ChecklistItem[],
  newItem: "",
});

type Filters = { result?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "pass", label: "Pass", filters: { result: "pass" } },
  { id: "fail", label: "Fail", filters: { result: "fail" } },
  { id: "conditional", label: "Conditional", filters: { result: "conditional" } },
];

const fmtDate = (d: string) => new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });

export default function QCPage() {
  const views = useSavedViews<Filters>("manufacturing.qc", VIEWS);
  const [records, setRecords] = useState<QCRecord[]>([]);
  const [units, setUnits] = useState<ProductionUnit[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState(emptyForm);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [qRes, uRes] = await Promise.all([
        fetchAllPages<QCRecord>(manufacturingApi.listQCRecords),
        fetchAllPages<ProductionUnit>(manufacturingApi.listUnits),
      ]);
      setRecords(qRes.rows);
      setUnits(uRes.rows);
    } catch { setRecords([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  function toggleCheck(idx: number) {
    setForm((f) => ({
      ...f,
      checklist: f.checklist.map((c, i) => (i === idx ? { ...c, result: c.result === "pass" ? "fail" : "pass" } : c)),
    }));
  }

  function addCheck() {
    const item = form.newItem.trim();
    if (!item) return;
    setForm((f) => ({ ...f, checklist: [...f.checklist, { item, result: "pass" }], newItem: "" }));
  }

  async function create() {
    if (!form.unit_id) return;
    setSaving(true);
    try {
      await manufacturingApi.createQCRecord({
        unit_id: form.unit_id,
        overall_result: form.overall_result,
        checklist_items: form.checklist,
        notes: form.notes.trim() || undefined,
      });
      setShowModal(false);
      setForm(emptyForm());
      await load();
    } catch (err) {
      toast.error(extractApiError(err, "Failed to log inspection"));
    } finally { setSaving(false); }
  }

  const filterResult = views.active?.filters.result ?? "";
  const filters: Filters = filterResult ? { result: filterResult } : {};
  const filtered = !filterResult ? records : records.filter((r) => r.result === filterResult);
  const passCount = records.filter((r) => r.result === "pass").length;
  const failCount = records.filter((r) => r.result === "fail").length;
  const passRate = records.length ? Math.round((passCount / records.length) * 100) : 0;
  // Units waiting for inspection have no QC record yet, so the record counts alone read 0
  // while units sit in "QC pending": count those units too.
  const awaitingCount = units.filter((u) => u.status === "qc_pending").length;

  const columns = useMemo<ColumnDef<QCRecord>[]>(() => [
    { accessorKey: "serial_number", header: "Serial", cell: (c) => <span className="font-mono text-[13px] font-medium text-fg">{c.getValue<string>() ?? "-"}</span> },
    { id: "product", accessorFn: (r) => r.product?.name ?? "", header: "Product", cell: (c) => c.getValue<string>() || "-" },
    { accessorKey: "inspector_name", header: "Inspector", meta: { hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{c.getValue<string>() ?? "-"}</span> },
    {
      id: "failed", header: "Failed checks", enableSorting: false, meta: { hideOnMobile: true },
      cell: (c) => {
        const failed = (c.row.original.checklist_items ?? []).filter((i) => i.result === "fail").map((i) => i.item);
        return failed.length ? <span className="text-tone-red">{failed.join(", ")}</span> : <span className="text-fg-subtle">-</span>;
      },
    },
    { id: "date", accessorFn: (r) => r.checked_at ?? r.created_at, header: "Checked", meta: { numeric: true }, cell: (c) => <span className="text-fg-muted">{fmtDate(c.getValue<string>())}</span> },
    { accessorKey: "result", header: "Result", cell: (c) => <StatusPill family="qc" status={c.getValue<string>()} label={RESULT_LABELS[c.getValue<string>()]} /> },
  ], []);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Awaiting QC" value={awaitingCount} note={awaitingCount > 0 ? "units in QC pending" : undefined} />
        <Stat label="Inspections" value={records.length} />
        <Stat label="Pass" value={passCount} />
        <Stat label="Fail" value={failCount} alert={failCount > 0} />
        <Stat label="Pass rate" value={records.length ? `${passRate}%` : "-"} alert={records.length > 0 && passRate < 80} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Quality control" summary="Per-unit inspection records and defect tracking"
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Log inspection</Button>} />
        <SavedViewTabs state={views} currentFilters={filters} />
        <DataTable label="Inspection records" columns={columns} data={filtered} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: ShieldCheck, title: "No inspection records", description: "Log an inspection when a unit leaves production." }}
          renderDetails={(r) => (
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[240px] flex-1" cols={2} items={[
                { label: "Product code", value: r.product?.product_code ?? "-" },
                { label: "Notes", value: r.notes ?? "-" },
              ]} />
              {(r.checklist_items ?? []).length > 0 && (
                <ul className="min-w-[240px] flex-1 divide-y divide-line-soft rounded-lg border border-line bg-surface">
                  {(r.checklist_items ?? []).map((c, i) => (
                    <li key={i} className="flex items-center justify-between px-3 py-1.5 text-[13px]">
                      <span className="text-fg">{c.item}</span>
                      <StatusPill family="qc" status={c.result} />
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Log inspection"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} loading={saving}
            disabled={!form.unit_id || (form.overall_result === "conditional" && !form.notes.trim())}>
            {saving ? "Saving..." : "Log inspection"}
          </Button>
        </>}>
        <div className="space-y-4">
          <Field label="Unit" required>
            <Select value={form.unit_id} onChange={(e) => setForm({ ...form, unit_id: e.target.value })} className="font-mono"
              placeholder="Select unit by serial"
              options={units.map((u) => ({ value: u.id, label: `${u.serial_number}${u.product ? `, ${u.product.name}` : ""} (${u.status})` }))} />
          </Field>
          <fieldset>
            <legend className="mb-1.5 text-[13px] font-medium text-fg">Checklist</legend>
            <div className="space-y-1.5">
              {form.checklist.map((c, i) => (
                <button key={`${c.item}-${i}`} type="button" onClick={() => toggleCheck(i)} aria-pressed={c.result === "pass"}
                  className="flex min-h-11 w-full items-center justify-between rounded-lg border border-line px-3 text-sm hover:bg-surface-2 md:min-h-9">
                  <span className="text-fg">{c.item}</span>
                  <StatusPill family="qc" status={c.result} />
                </button>
              ))}
            </div>
            <div className="mt-2 flex gap-2">
              <Input aria-label="New check" value={form.newItem} onChange={(e) => setForm({ ...form, newItem: e.target.value })}
                onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); addCheck(); } }}
                placeholder="Add a check" />
              <Button onClick={addCheck}>Add</Button>
            </div>
          </fieldset>
          <fieldset>
            <legend className="mb-1.5 text-[13px] font-medium text-fg">Overall result <span className="text-brand-ink" aria-hidden>*</span></legend>
            <div className="flex gap-2" role="radiogroup">
              {["pass", "fail", "conditional"].map((r) => (
                <button key={r} type="button" role="radio" aria-checked={form.overall_result === r}
                  onClick={() => setForm({ ...form, overall_result: r })}
                  className={cn("min-h-11 flex-1 rounded-lg border text-sm font-medium capitalize transition-colors md:min-h-9",
                    form.overall_result === r ? "border-fg bg-fg text-surface" : "border-line bg-surface text-fg-muted hover:bg-surface-2")}>
                  {r}
                </button>
              ))}
            </div>
          </fieldset>
          <Field label={form.overall_result === "conditional" ? "Conditions" : "Notes"} required={form.overall_result === "conditional"}>
            <Textarea value={form.notes} onChange={(e) => setForm({ ...form, notes: e.target.value })} rows={2}
              placeholder={form.overall_result === "conditional" ? "What must be true for this unit to be used (it counts as passed)" : undefined} />
          </Field>
        </div>
      </Dialog>
    </div>
  );
}
