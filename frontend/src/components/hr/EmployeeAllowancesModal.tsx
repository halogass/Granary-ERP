"use client";

import { useCallback, useEffect, useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { Button, Checkbox, Dialog, Field, IconButton, Input, Skeleton } from "@/components/ui";
import { cn } from "@/lib/utils";
import { hrApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { formatCurrency } from "@/lib/currency";

interface Allowance {
  id: string; name: string; amount: number; currency: string;
  is_fixed: boolean; is_taxable: boolean; is_active: boolean; notes?: string | null;
}

const emptyDraft = { name: "", amount: "", currency: "USD", is_fixed: true, is_taxable: true };

/** Recurring monthly allowances, added to each payroll run (prorated for joiners and leavers).
 *  Fixed ones count toward the contribution wage and the overtime hourly wage; taxable ones toward income tax. */
export default function EmployeeAllowancesModal({ employeeId, employeeName, canEdit, onClose }: {
  employeeId: string; employeeName: string; canEdit: boolean; onClose: () => void;
}) {
  const [rows, setRows] = useState<Allowance[]>([]);
  const [loading, setLoading] = useState(true);
  const [draft, setDraft] = useState(emptyDraft);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setRows((await hrApi.listAllowances(employeeId)).data ?? []);
    } catch (e) {
      toast.error(extractApiError(e, "Could not load allowances."));
    } finally { setLoading(false); }
  }, [employeeId]);

  useEffect(() => { load(); }, [load]);

  async function add() {
    if (!draft.name.trim() || !(parseFloat(draft.amount) > 0)) return;
    setSaving(true);
    try {
      await hrApi.createAllowance(employeeId, { ...draft, name: draft.name.trim(), amount: parseFloat(draft.amount) });
      setDraft(emptyDraft);
      await load();
    } catch (e) {
      toast.error(extractApiError(e, "Could not add the allowance."));
    } finally { setSaving(false); }
  }

  async function toggle(a: Allowance, field: "is_active" | "is_fixed" | "is_taxable") {
    try {
      await hrApi.updateAllowance(a.id, { ...a, [field]: !a[field] });
      await load();
    } catch (e) {
      toast.error(extractApiError(e, "Could not update the allowance."));
    }
  }

  async function remove(a: Allowance) {
    try {
      await hrApi.deleteAllowance(a.id);
      await load();
    } catch (e) {
      toast.error(extractApiError(e, "Could not delete the allowance."));
    }
  }

  return (
    <Dialog open size="lg" onOpenChange={(o) => { if (!o) onClose(); }} title="Allowances"
      description={`${employeeName}, added to every payroll run`}>
      {loading ? (
        <div className="space-y-2 py-2"><Skeleton className="h-9 w-full" /><Skeleton className="h-9 w-full" /></div>
      ) : rows.length === 0 ? (
        <p className="py-4 text-sm text-fg-muted">No allowances yet.</p>
      ) : (
        <div className="mb-4 divide-y divide-line-soft rounded-lg border border-line">
          {rows.map((a) => (
            <div key={a.id} className={cn("flex flex-wrap items-center gap-2 px-3 py-2", !a.is_active && "opacity-60")}>
              <span className="min-w-0 flex-1 truncate text-sm font-medium text-fg">{a.name}</span>
              <span className="text-sm text-fg tabular">{formatCurrency(a.amount, a.currency)}</span>
              {(["is_fixed", "is_taxable", "is_active"] as const).map((f) => (
                <button key={f} type="button" disabled={!canEdit} onClick={() => toggle(a, f)} aria-pressed={a[f]}
                  className={cn("h-[22px] rounded-full px-2.5 text-xs font-medium disabled:cursor-default",
                    a[f] ? "bg-tone-blue-soft text-tone-blue" : "bg-surface-2 text-fg-subtle")}>
                  {f === "is_fixed" ? "Fixed" : f === "is_taxable" ? "Taxable" : "Active"}
                </button>
              ))}
              {canEdit && <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${a.name}`} onClick={() => remove(a)} />}
            </div>
          ))}
        </div>
      )}

      {canEdit && (
        <div className="grid grid-cols-2 items-end gap-2 md:grid-cols-12">
          <Field label="Name" className="col-span-2 md:col-span-4">
            <Input placeholder="e.g. Transport" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
          </Field>
          <Field label="Monthly amount" className="md:col-span-3">
            <Input numeric type="number" min={0} step="any" value={draft.amount} onChange={(e) => setDraft({ ...draft, amount: e.target.value })} />
          </Field>
          <Field label="Currency" className="md:col-span-2">
            <Input value={draft.currency} maxLength={3} className="text-center"
              onChange={(e) => setDraft({ ...draft, currency: e.target.value.toUpperCase() })} />
          </Field>
          <div className="flex h-11 items-center gap-3 md:col-span-3 md:h-9">
            <Checkbox label="Fixed" checked={draft.is_fixed} onChange={(e) => setDraft({ ...draft, is_fixed: e.target.checked })} />
            <Checkbox label="Taxable" checked={draft.is_taxable} onChange={(e) => setDraft({ ...draft, is_taxable: e.target.checked })} />
          </div>
          <Button variant="primary" className="col-span-2 md:col-span-12 md:justify-self-end" onClick={add} loading={saving}>
            {!saving && <Plus aria-hidden />} Add allowance
          </Button>
        </div>
      )}
      <p className="mt-3 text-xs text-fg-muted">
        Fixed allowances count toward the contribution wage and the overtime hourly wage. Taxable ones count as income for income tax.
      </p>
    </Dialog>
  );
}
