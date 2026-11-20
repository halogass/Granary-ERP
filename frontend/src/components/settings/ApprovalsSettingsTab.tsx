"use client";

import { useCallback, useEffect, useState } from "react";
import { AlertCircle, ArrowDown, ArrowUp, CheckCircle, Plus, Save, Trash2 } from "lucide-react";
import {
  Button, Card, CardBody, CardFooter, CardHeader, Checkbox, DataTable, EmptyState, Field, FieldGrid, IconButton,
  Input, Select, Skeleton, StatusPill, Textarea, type ColumnDef,
} from "@/components/ui";
import { approvalsApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { GRANARY_DEPARTMENTS } from "@/lib/departments";
import { cn } from "@/lib/utils";
import { useDraftSource } from "@/components/settings/SettingsDraft";

// Types (backend/app/routers/approvals.py)

type ValueKind = "amount" | "pct" | "days" | "list" | "flag" | "none" | "quotes" | "flag_amount";
interface RuleMeta { label: string; value: ValueKind; options?: string[] }
interface RulesPayload {
  rules: Record<string, RuleMeta>;
  preconditions: Record<string, RuleMeta>;
  approver_modes: string[];
}

type Condition = Record<string, unknown> | null;
interface Leaf { rule: string; value?: unknown }

interface Step {
  sequence: number; name: string; approver_mode: string;
  role_slug: string | null; department: string | null; snapshot_user_key: string | null; user_id: string | null;
  condition: Condition; sla_hours: number | null; is_notify_only: boolean; allow_self_approval: boolean;
}

interface Precondition { rule: string; message?: string; value?: unknown }

interface Policy {
  id: string; entity_type: string; entity_label: string; name: string; description: string | null;
  sop_reference: string | null; is_active: boolean; version: number; preconditions: Precondition[];
  signature_flow: string; signer_role: string | null; sign_deadline_days: number | null; max_extension_days: number | null;
  qr_enabled: boolean; view_scope: string; allow_download: boolean; requires_login: boolean; token_ttl_days: number | null;
  sla_hours: number | null; whatsapp_escalate_above_base: number | null; updated_at: string | null; steps: Step[];
}

interface Allowance {
  kind: string; label: string; min_qty: number | null; max_qty: number | null;
  max_discount_pct: number; notify_owner: boolean; is_active: boolean;
}
type AllowanceRow = Allowance & { key: string };

// Condition editing: one flat any/all group of rules; anything deeper is edited as JSON.
interface ConditionForm { mode: "always" | "any" | "all" | "advanced"; leaves: Leaf[]; json: string }
type EditorStep = Step & { key: string; form: ConditionForm };
type EditorPrecondition = { key: string; rule: string; message: string; valueText: string };
type Draft = Omit<Policy, "steps" | "preconditions"> & { steps: EditorStep[]; preconditions: EditorPrecondition[] };

const ROLE_SLUGS = ["executive", "system_admin", "finance_admin", "sales_bizdev", "team_lead",
  "project_manager", "hr_admin", "manufacturing_tech"];
const MODE_LABEL: Record<string, string> = {
  owner: "Owner", role: "Holder of a role", manager: "Submitter's line manager",
  snapshot_user: "Person named on the document", user: "A specific user",
};
const FLOW_LABEL: Record<string, string> = {
  none: "No e-signature (approval + QR)", counterparty: "Counterparty signs (our approval + QR is our side)",
  internal: "Our signatory signs", internal_then_counterparty: "Our signatory, then the counterparty",
};
const ALLOWANCE_KINDS = ["volume", "bundle", "loyalty", "multi_year_saas"];

let keySeq = 0;
const newKey = () => `k${++keySeq}`;

function toForm(c: Condition): ConditionForm {
  if (!c) return { mode: "always", leaves: [], json: "" };
  if (typeof c.rule === "string") return { mode: "any", leaves: [c as unknown as Leaf], json: "" };
  const list = (c.any ?? c.all) as Condition[] | undefined;
  if (Array.isArray(list) && list.every((x) => x && typeof x.rule === "string")) {
    return { mode: c.any ? "any" : "all", leaves: list as unknown as Leaf[], json: "" };
  }
  return { mode: "advanced", leaves: [], json: JSON.stringify(c, null, 2) };
}

function toDraft(p: Policy): Draft {
  return {
    ...p,
    steps: p.steps.map((s) => ({ ...s, key: newKey(), form: toForm(s.condition) })),
    preconditions: p.preconditions.map((pre) => ({
      key: newKey(), rule: pre.rule, message: pre.message ?? "",
      valueText: pre.value === undefined ? "" : JSON.stringify(pre.value),
    })),
  };
}

const num = (v: string) => (v.trim() === "" ? null : Number(v));
const toRows = (rows: Allowance[]): AllowanceRow[] => rows.map((row) => ({ ...row, key: newKey() }));

// Condition builder

function LeafEditor({ leaf, rules, onChange, onRemove }: {
  leaf: Leaf; rules: Record<string, RuleMeta>; onChange: (l: Leaf) => void; onRemove: () => void;
}) {
  const meta = rules[leaf.rule];
  function pickRule(rule: string) {
    const m = rules[rule];
    const value = m?.value === "list" ? [] : m?.value === "flag" ? m.options?.[0] : undefined;
    onChange(value === undefined ? { rule } : { rule, value });
  }
  const listValue = Array.isArray(leaf.value) ? (leaf.value as string[]) : [];
  return (
    <div className="flex flex-col gap-2 rounded-lg bg-surface-2 p-2 sm:flex-row sm:items-start">
      <div className="sm:flex-1">
        <Select aria-label="Rule" value={leaf.rule} onChange={(e) => pickRule(e.target.value)}
          options={Object.entries(rules).map(([name, m]) => ({ value: name, label: m.label }))} />
      </div>
      {meta && ["amount", "pct", "days"].includes(meta.value) && (
        <Input type="number" numeric min="0" step="any" aria-label="Rule value" value={leaf.value === undefined ? "" : String(leaf.value)}
          placeholder={meta.value === "amount" ? "Amount" : meta.value === "pct" ? "%" : "days"}
          onChange={(e) => onChange({ rule: leaf.rule, ...(e.target.value === "" ? {} : { value: Number(e.target.value) }) })}
          className="sm:w-40" />
      )}
      {meta?.value === "flag" && (
        <div className="sm:w-48">
          <Select aria-label="Flag" value={String(leaf.value ?? "")} onChange={(e) => onChange({ rule: leaf.rule, value: e.target.value })}
            options={(meta.options ?? []).map((o) => ({ value: o, label: o.replace(/_/g, " ") }))} />
        </div>
      )}
      {meta?.value === "list" && (
        <div className="flex flex-wrap gap-x-3 gap-y-1 py-1 sm:flex-1">
          {(meta.options ?? []).map((o) => (
            <Checkbox key={o} label={o.replace(/_/g, " ")} className="text-xs" checked={listValue.includes(o)}
              onChange={(e) => onChange({ rule: leaf.rule, value: e.target.checked ? [...listValue, o] : listValue.filter((x) => x !== o) })} />
          ))}
        </div>
      )}
      <IconButton label="Remove rule" icon={Trash2} onClick={onRemove} className="self-end sm:self-center" />
    </div>
  );
}

function ConditionEditor({ form, rules, onChange }: {
  form: ConditionForm; rules: Record<string, RuleMeta>; onChange: (f: ConditionForm) => void;
}) {
  const firstRule = Object.keys(rules)[0] ?? "amount_gt_base";
  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-2 text-[13px] text-fg-muted">
        <span>Required</span>
        <div className="w-60">
          <Select aria-label="When the step is required" value={form.mode} onChange={(e) => {
            const mode = e.target.value as ConditionForm["mode"];
            if (mode === "advanced") onChange({ mode, leaves: [], json: form.json || JSON.stringify(form.leaves.length ? { [form.mode === "all" ? "all" : "any"]: form.leaves } : null, null, 2) });
            else onChange({ ...form, mode, leaves: mode === "always" ? [] : form.leaves });
          }} options={[
            { value: "always", label: "always" },
            { value: "any", label: "when ANY of these apply" },
            { value: "all", label: "when ALL of these apply" },
            { value: "advanced", label: "advanced (JSON)" },
          ]} />
        </div>
      </div>
      {form.mode === "advanced" ? (
        <Textarea rows={6} aria-label="Condition JSON" value={form.json} onChange={(e) => onChange({ ...form, json: e.target.value })}
          className="font-mono text-xs" spellCheck={false} />
      ) : form.mode !== "always" && (
        <>
          {form.leaves.map((leaf, i) => (
            <LeafEditor key={i} leaf={leaf} rules={rules}
              onChange={(l) => onChange({ ...form, leaves: form.leaves.map((x, j) => (j === i ? l : x)) })}
              onRemove={() => onChange({ ...form, leaves: form.leaves.filter((_, j) => j !== i) })} />
          ))}
          <Button variant="ghost" size="sm" onClick={() => onChange({ ...form, leaves: [...form.leaves, { rule: firstRule }] })}>
            <Plus aria-hidden /> Add rule
          </Button>
        </>
      )}
    </div>
  );
}

// Block

export default function ApprovalsSettingsTab() {
  const [policies, setPolicies] = useState<Policy[]>([]);
  const [rules, setRules] = useState<RulesPayload | null>(null);
  const [allowances, setAllowances] = useState<AllowanceRow[]>([]);
  const [savedAllowances, setSavedAllowances] = useState<Allowance[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState("");
  const [draft, setDraft] = useState<Draft | null>(null);
  const [policyDirty, setPolicyDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const [allowanceMsg, setAllowanceMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [savingAllowances, setSavingAllowances] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [p, r, a] = await Promise.all([approvalsApi.listPolicies(), approvalsApi.rules(), approvalsApi.listAllowances()]);
      setPolicies(p.data ?? []);
      setRules(r.data);
      setAllowances(toRows(a.data ?? []));
      setSavedAllowances(a.data ?? []);
      setLoadError("");
    } catch (e) {
      setLoadError(extractApiError(e, "Could not load approval policies."));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { load(); }, [load]);

  const update = (patch: Partial<Draft>) => { setPolicyDirty(true); setDraft((d) => (d ? { ...d, ...patch } : d)); };
  const updateStep = (key: string, patch: Partial<EditorStep>) => {
    setPolicyDirty(true);
    setDraft((d) => (d ? { ...d, steps: d.steps.map((s) => (s.key === key ? { ...s, ...patch } : s)) } : d));
  };
  const moveStep = (index: number, delta: number) => {
    setPolicyDirty(true);
    setDraft((d) => {
      if (!d) return d;
      const steps = [...d.steps];
      const target = index + delta;
      if (target < 0 || target >= steps.length) return d;
      [steps[index], steps[target]] = [steps[target], steps[index]];
      return { ...d, steps };
    });
  };

  function buildPayload(d: Draft) {
    const steps = d.steps.map((s, i) => {
      let condition: Condition = null;
      if (s.form.mode === "advanced") {
        try {
          condition = s.form.json.trim() ? JSON.parse(s.form.json) : null;
        } catch {
          throw new Error(`Step "${s.name}": the advanced condition is not valid JSON`);
        }
      } else if (s.form.mode !== "always") {
        for (const leaf of s.form.leaves) {
          const kind = rules!.rules[leaf.rule]?.value;
          if (kind && ["amount", "pct", "days"].includes(kind) && typeof leaf.value !== "number") {
            throw new Error(`Step "${s.name}": enter a value for "${rules!.rules[leaf.rule].label}"`);
          }
        }
        condition = s.form.leaves.length ? { [s.form.mode]: s.form.leaves } : null;
      }
      if (!s.name.trim()) throw new Error(`Step ${i + 1} needs a name`);
      return {
        sequence: i + 1, name: s.name.trim(), approver_mode: s.approver_mode,
        role_slug: s.approver_mode === "role" ? s.role_slug || null : null,
        department: s.approver_mode === "role" ? s.department || null : null,
        snapshot_user_key: s.approver_mode === "snapshot_user" ? s.snapshot_user_key || null : null,
        user_id: s.approver_mode === "user" ? s.user_id || null : null,
        condition, sla_hours: s.sla_hours, is_notify_only: s.is_notify_only, allow_self_approval: s.allow_self_approval,
      };
    });
    const preconditions = d.preconditions.map((p) => {
      let value: unknown;
      if (p.valueText.trim()) {
        try {
          value = JSON.parse(p.valueText);
        } catch {
          throw new Error(`Precondition "${rules!.preconditions[p.rule]?.label ?? p.rule}": the value is not valid JSON`);
        }
      }
      return { rule: p.rule, ...(p.message.trim() ? { message: p.message.trim() } : {}), ...(value === undefined ? {} : { value }) };
    });
    return {
      name: d.name, description: d.description, is_active: d.is_active, preconditions,
      signature_flow: d.signature_flow, signer_role: d.signer_role || null,
      sign_deadline_days: d.sign_deadline_days, max_extension_days: d.max_extension_days,
      qr_enabled: d.qr_enabled, view_scope: d.view_scope, allow_download: d.allow_download,
      requires_login: d.requires_login, token_ttl_days: d.token_ttl_days,
      sla_hours: d.sla_hours, whatsapp_escalate_above_base: d.whatsapp_escalate_above_base, steps,
    };
  }

  function payloadProblem(): string | null {
    if (!draft) return null;
    try {
      buildPayload(draft);
      return null;
    } catch (e) {
      return (e as Error).message;
    }
  }

  async function persistPolicy() {
    if (!draft) return;
    setError("");
    setSaved(false);
    try {
      const res = await approvalsApi.updatePolicy(draft.id, buildPayload(draft));
      setPolicies((prev) => prev.map((p) => (p.id === res.data.id ? res.data : p)));
      setDraft(toDraft(res.data));
      setPolicyDirty(false);
      setSaved(true);
    } catch (e) {
      setError(extractApiError(e, "Could not save the policy."));
      throw e;
    }
  }

  async function savePolicy() {
    const problem = payloadProblem();
    if (problem) { setError(problem); return; }
    setSaving(true);
    try { await persistPolicy(); } catch { /* shown inline */ } finally { setSaving(false); }
  }

  async function persistAllowances() {
    setAllowanceMsg(null);
    try {
      const res = await approvalsApi.replaceAllowances(allowances.map(({ key: _key, ...row }) => row));
      setAllowances(toRows(res.data ?? []));
      setSavedAllowances(res.data ?? []);
      setAllowanceMsg({ ok: true, text: "Discount allowances saved." });
    } catch (e) {
      setAllowanceMsg({ ok: false, text: extractApiError(e, "Could not save the allowances.") });
      throw e;
    }
  }

  async function saveAllowances() {
    setSavingAllowances(true);
    try { await persistAllowances(); } catch { /* shown inline */ } finally { setSavingAllowances(false); }
  }

  const allowancesDirty = JSON.stringify(allowances.map(({ key: _key, ...row }) => row)) !== JSON.stringify(savedAllowances);

  const { inSaveBar } = useDraftSource("approval-policy", {
    count: policyDirty && draft ? 1 : 0,
    save: persistPolicy,
    discard: () => {
      const original = draft && policies.find((p) => p.id === draft.id);
      setDraft(original ? toDraft(original) : null);
      setPolicyDirty(false);
      setError("");
    },
    validate: payloadProblem,
  });
  useDraftSource("discount-allowances", {
    count: allowancesDirty ? 1 : 0,
    save: persistAllowances,
    discard: () => { setAllowances(toRows(savedAllowances)); setAllowanceMsg(null); },
  });

  function selectPolicy(p: Policy) {
    if (policyDirty && draft && draft.id !== p.id
      && !window.confirm("This policy has unsaved changes. Open another one and drop them?")) return;
    setDraft(toDraft(p));
    setPolicyDirty(false);
    setError("");
    setSaved(false);
  }

  const setAllowance = (key: string, patch: Partial<Allowance>) =>
    setAllowances((rows) => rows.map((r) => (r.key === key ? { ...r, ...patch } : r)));

  if (loading) {
    return <Card><CardBody className="space-y-3"><Skeleton className="h-4 w-1/3" /><Skeleton className="h-24 w-full" /></CardBody></Card>;
  }
  if (loadError || !rules) return <p className="text-sm text-tone-red">{loadError || "Could not load approval policies."}</p>;

  const allowanceColumns: ColumnDef<AllowanceRow>[] = [
    { id: "kind", header: "Type", meta: { width: "150px" }, cell: ({ row }) => (
      <Select aria-label="Type" value={row.original.kind} onChange={(e) => setAllowance(row.original.key, { kind: e.target.value })}
        options={ALLOWANCE_KINDS.map((k) => ({ value: k, label: k.replace(/_/g, " ") }))} />
    ) },
    { id: "label", header: "Label", cell: ({ row }) => (
      <Input aria-label="Label" value={row.original.label} onChange={(e) => setAllowance(row.original.key, { label: e.target.value })} />
    ) },
    { id: "min_qty", header: "Min qty", meta: { numeric: true, width: "96px" }, cell: ({ row }) => (
      <Input type="number" numeric min="0" aria-label="Min qty" value={row.original.min_qty ?? ""}
        onChange={(e) => setAllowance(row.original.key, { min_qty: num(e.target.value) })} />
    ) },
    { id: "max_qty", header: "Max qty", meta: { numeric: true, width: "96px" }, cell: ({ row }) => (
      <Input type="number" numeric min="0" aria-label="Max qty" value={row.original.max_qty ?? ""}
        onChange={(e) => setAllowance(row.original.key, { max_qty: num(e.target.value) })} />
    ) },
    { id: "max_discount_pct", header: "Max %", meta: { numeric: true, width: "96px" }, cell: ({ row }) => (
      <Input type="number" numeric min="0" max="100" step="any" aria-label="Max %" value={row.original.max_discount_pct}
        onChange={(e) => setAllowance(row.original.key, { max_discount_pct: Number(e.target.value) })} />
    ) },
    { id: "notify_owner", header: "Inform Owner", meta: { align: "center", width: "96px" }, cell: ({ row }) => (
      <Checkbox aria-label="Inform Owner" checked={row.original.notify_owner}
        onChange={(e) => setAllowance(row.original.key, { notify_owner: e.target.checked })} />
    ) },
    { id: "is_active", header: "Active", meta: { align: "center", width: "80px" }, cell: ({ row }) => (
      <Checkbox aria-label="Active" checked={row.original.is_active}
        onChange={(e) => setAllowance(row.original.key, { is_active: e.target.checked })} />
    ) },
    { id: "remove", header: "", meta: { width: "48px" }, cell: ({ row }) => (
      <IconButton label="Remove allowance" icon={Trash2}
        onClick={() => setAllowances((rows) => rows.filter((r) => r.key !== row.original.key))} />
    ) },
  ];

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-1 items-start gap-4 lg:grid-cols-3">
        {/* Policy list */}
        <Card className="overflow-hidden">
          <CardHeader title="Approval policies" description="One per document type, seeded from the SOPs" />
          <ul className="divide-y divide-line">
            {policies.map((p) => (
              <li key={p.id}>
                <button type="button" onClick={() => selectPolicy(p)} aria-current={draft?.id === p.id ? "true" : undefined}
                  className={cn("w-full px-4 py-2.5 text-left transition-colors",
                    draft?.id === p.id ? "bg-brand/[0.07]" : "hover:bg-surface-2")}>
                  <div className="flex items-center gap-2">
                    <span className="truncate text-sm font-medium text-fg">{p.name}</span>
                    <StatusPill className="ml-auto" status={p.is_active ? "active" : "inactive"} />
                  </div>
                  <p className="mt-0.5 truncate text-xs text-fg-muted">{p.entity_label}{p.sop_reference ? ` · ${p.sop_reference}` : ""} · v{p.version}</p>
                </button>
              </li>
            ))}
          </ul>
        </Card>

        {/* Editor */}
        <Card className="lg:col-span-2">
          {!draft ? (
            <EmptyState title="Select a policy to edit its approval chain" />
          ) : (
            <>
              <CardBody className="space-y-6">
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="min-w-0">
                    <p className="text-sm font-semibold text-fg">{draft.entity_label}</p>
                    <p className="mt-0.5 text-xs text-fg-muted">Changes apply to new submissions; requests already in flight keep their chain.</p>
                  </div>
                  <Checkbox checked={draft.is_active} onChange={(e) => update({ is_active: e.target.checked })} label="Active" />
                </div>

                <FieldGrid cols={2}>
                  <Field label="Name" className="md:col-span-2">
                    <Input value={draft.name} onChange={(e) => update({ name: e.target.value })} />
                  </Field>
                  <Field label="Description" className="md:col-span-2">
                    <Textarea rows={2} value={draft.description ?? ""} onChange={(e) => update({ description: e.target.value })} />
                  </Field>
                  <Field label="Default SLA (hours)">
                    <Input type="number" numeric min="1" value={draft.sla_hours ?? ""} onChange={(e) => update({ sla_hours: num(e.target.value) })} />
                  </Field>
                  <Field label="WhatsApp approvers above (base currency)">
                    <Input type="number" numeric min="0" value={draft.whatsapp_escalate_above_base ?? ""} placeholder="Never"
                      onChange={(e) => update({ whatsapp_escalate_above_base: num(e.target.value) })} />
                  </Field>
                </FieldGrid>

                {/* Steps */}
                <section className="space-y-3">
                  <h3 className="text-sm font-semibold text-fg">Approval chain</h3>
                  {draft.steps.length === 0 && <p className="text-xs text-fg-muted">No steps: every submission is approved automatically.</p>}
                  {draft.steps.map((s, i) => (
                    <div key={s.key} className="space-y-3 rounded-lg border border-line p-3">
                      <div className="flex items-center gap-1">
                        <span className="w-6 text-xs font-semibold text-fg-muted tabular">{i + 1}.</span>
                        <Input aria-label={`Step ${i + 1} name`} value={s.name} onChange={(e) => updateStep(s.key, { name: e.target.value })} className="flex-1" />
                        <IconButton label="Move up" icon={ArrowUp} onClick={() => moveStep(i, -1)} disabled={i === 0} />
                        <IconButton label="Move down" icon={ArrowDown} onClick={() => moveStep(i, 1)} disabled={i === draft.steps.length - 1} />
                        <IconButton label="Remove step" icon={Trash2} onClick={() => update({ steps: draft.steps.filter((x) => x.key !== s.key) })} />
                      </div>
                      <FieldGrid cols={3}>
                        <Field label="Approver">
                          <Select value={s.approver_mode} onChange={(e) => updateStep(s.key, { approver_mode: e.target.value })}
                            options={rules.approver_modes.map((m) => ({ value: m, label: MODE_LABEL[m] ?? m }))} />
                        </Field>
                        {s.approver_mode === "role" && (
                          <>
                            <Field label="Role">
                              <Input list="approval-role-slugs" value={s.role_slug ?? ""} onChange={(e) => updateStep(s.key, { role_slug: e.target.value })} />
                            </Field>
                            <Field label="Department">
                              <Select value={s.department ?? ""} onChange={(e) => updateStep(s.key, { department: e.target.value || null })}
                                placeholder="Any" options={GRANARY_DEPARTMENTS.map((d) => ({ value: d, label: d }))} />
                            </Field>
                          </>
                        )}
                        {s.approver_mode === "snapshot_user" && (
                          <Field label="Document field">
                            <Input list="approval-snapshot-keys" value={s.snapshot_user_key ?? ""} onChange={(e) => updateStep(s.key, { snapshot_user_key: e.target.value })} />
                          </Field>
                        )}
                        {s.approver_mode === "user" && (
                          <Field label="User ID" className="md:col-span-2">
                            <Input value={s.user_id ?? ""} onChange={(e) => updateStep(s.key, { user_id: e.target.value })} className="font-mono text-xs" />
                          </Field>
                        )}
                        <Field label="SLA (hours)">
                          <Input type="number" numeric min="1" value={s.sla_hours ?? ""} placeholder="Policy default"
                            onChange={(e) => updateStep(s.key, { sla_hours: num(e.target.value) })} />
                        </Field>
                      </FieldGrid>
                      <div className="flex flex-wrap gap-x-5 gap-y-1">
                        <Checkbox checked={s.is_notify_only} onChange={(e) => updateStep(s.key, { is_notify_only: e.target.checked })} label="Inform only (no decision)" />
                        <Checkbox checked={s.allow_self_approval} onChange={(e) => updateStep(s.key, { allow_self_approval: e.target.checked })} label="Approver may approve their own submission" />
                      </div>
                      <ConditionEditor form={s.form} rules={rules.rules} onChange={(form) => updateStep(s.key, { form })} />
                    </div>
                  ))}
                  <Button size="sm" onClick={() => update({ steps: [...draft.steps, {
                    key: newKey(), sequence: draft.steps.length + 1, name: "Owner approval", approver_mode: "owner", role_slug: null,
                    department: null, snapshot_user_key: null, user_id: null, condition: null, sla_hours: null,
                    is_notify_only: false, allow_self_approval: false, form: toForm(null),
                  }] })}>
                    <Plus aria-hidden /> Add step
                  </Button>
                </section>

                {/* Preconditions */}
                <section className="space-y-3">
                  <div>
                    <h3 className="text-sm font-semibold text-fg">Submission requirements</h3>
                    <p className="text-xs text-fg-muted">A document that fails one of these cannot be submitted at all.</p>
                  </div>
                  {draft.preconditions.map((p) => {
                    const meta = rules.preconditions[p.rule];
                    return (
                      <div key={p.key} className="space-y-2 rounded-lg bg-surface-2 p-3">
                        <div className="flex gap-2">
                          <div className="flex-1">
                            <Select aria-label="Requirement" value={p.rule}
                              onChange={(e) => update({ preconditions: draft.preconditions.map((x) => (x.key === p.key ? { ...x, rule: e.target.value, valueText: "" } : x)) })}
                              options={Object.entries(rules.preconditions).map(([name, m]) => ({ value: name, label: m.label }))} />
                          </div>
                          <IconButton label="Remove requirement" icon={Trash2}
                            onClick={() => update({ preconditions: draft.preconditions.filter((x) => x.key !== p.key) })} />
                        </div>
                        <Input aria-label="Message shown to the submitter" value={p.message} placeholder="Message shown to the submitter"
                          onChange={(e) => update({ preconditions: draft.preconditions.map((x) => (x.key === p.key ? { ...x, message: e.target.value } : x)) })} />
                        {meta && meta.value !== "none" && (
                          <Input aria-label="Requirement value (JSON)" value={p.valueText}
                            placeholder={meta.value === "quotes" ? '{"min_amount_base": 10000000, "min_quotes": 2}' : '{"amount_base": 500000, "flag": "pre_approved"}'}
                            className="font-mono text-xs"
                            onChange={(e) => update({ preconditions: draft.preconditions.map((x) => (x.key === p.key ? { ...x, valueText: e.target.value } : x)) })} />
                        )}
                      </div>
                    );
                  })}
                  <Button size="sm"
                    onClick={() => update({ preconditions: [...draft.preconditions, { key: newKey(), rule: Object.keys(rules.preconditions)[0], message: "", valueText: "" }] })}>
                    <Plus aria-hidden /> Add requirement
                  </Button>
                </section>

                {/* Signing + QR */}
                <section className="space-y-3">
                  <h3 className="text-sm font-semibold text-fg">E-signature and QR verification</h3>
                  <FieldGrid cols={2}>
                    <Field label="E-signature">
                      <Select value={draft.signature_flow} onChange={(e) => update({ signature_flow: e.target.value })}
                        options={Object.entries(FLOW_LABEL).map(([v, l]) => ({ value: v, label: l }))} />
                    </Field>
                    <Field label="Signatory role">
                      <Input list="approval-role-slugs" value={draft.signer_role ?? ""} placeholder="owner"
                        onChange={(e) => update({ signer_role: e.target.value })} />
                    </Field>
                    {(draft.signature_flow === "internal_then_counterparty" || draft.signature_flow === "counterparty") && (
                      <>
                        <Field label="Counterparty deadline (business days)">
                          <Input type="number" numeric min="1" value={draft.sign_deadline_days ?? ""} onChange={(e) => update({ sign_deadline_days: num(e.target.value) })} />
                        </Field>
                        <Field label="Max extension (business days)">
                          <Input type="number" numeric min="0" value={draft.max_extension_days ?? ""} onChange={(e) => update({ max_extension_days: num(e.target.value) })} />
                        </Field>
                      </>
                    )}
                  </FieldGrid>
                  <div className="flex flex-wrap gap-x-5 gap-y-1">
                    <Checkbox checked={draft.qr_enabled} onChange={(e) => update({ qr_enabled: e.target.checked })} label="Print a verification QR" />
                    {draft.qr_enabled && (
                      <>
                        <Checkbox checked={draft.view_scope === "metadata_document"} label="Scanner can view the document"
                          onChange={(e) => update({ view_scope: e.target.checked ? "metadata_document" : "metadata" })} />
                        <Checkbox checked={draft.allow_download} onChange={(e) => update({ allow_download: e.target.checked })} label="Scanner can download" />
                        <Checkbox checked={draft.requires_login} onChange={(e) => update({ requires_login: e.target.checked })} label="Require login to verify" />
                      </>
                    )}
                  </div>
                  {draft.qr_enabled && (
                    <FieldGrid cols={2}>
                      <Field label="Code valid for (days)">
                        <Input type="number" numeric min="1" value={draft.token_ttl_days ?? ""} placeholder="No expiry"
                          onChange={(e) => update({ token_ttl_days: num(e.target.value) })} />
                      </Field>
                    </FieldGrid>
                  )}
                </section>

                {error && <p className="flex items-start gap-2 text-sm text-tone-red"><AlertCircle className="mt-0.5 size-4 shrink-0" aria-hidden />{error}</p>}
                {saved && <p className="flex items-center gap-2 text-sm text-fg-muted"><CheckCircle className="size-4" aria-hidden />Saved, version {draft.version}</p>}
              </CardBody>
              {!inSaveBar && (
                <CardFooter>
                  <Button variant="primary" onClick={savePolicy} loading={saving}>
                    {!saving && <Save aria-hidden />} Save policy
                  </Button>
                </CardFooter>
              )}
            </>
          )}
        </Card>
      </div>

      {/* Discount allowances */}
      <Card>
        <CardHeader title="Discount allowances"
          description={<>Discounts within an allowance don&apos;t need Owner approval. &quot;Inform Owner&quot; notifies without blocking.</>} />
        <div className="overflow-x-auto">
          <div className="min-w-[720px]">
            <DataTable label="Discount allowances" columns={allowanceColumns} data={allowances} getRowId={(r) => r.key}
              maxHeight="none" striped={false}
              empty={{ title: "No allowances", description: "Every discount needs Owner approval. Add an allowance to change that." }} />
          </div>
        </div>
        <CardFooter className="justify-between">
          <Button size="sm"
            onClick={() => setAllowances((rows) => [...rows, { key: newKey(), kind: "volume", label: "", min_qty: null, max_qty: null, max_discount_pct: 5, notify_owner: false, is_active: true }])}>
            <Plus aria-hidden /> Add allowance
          </Button>
          <div className="flex items-center gap-3">
            {allowanceMsg && <span className={cn("text-sm", allowanceMsg.ok ? "text-fg-muted" : "text-tone-red")}>{allowanceMsg.text}</span>}
            {!inSaveBar && (
              <Button variant="primary" onClick={saveAllowances} loading={savingAllowances}>
                {!savingAllowances && <Save aria-hidden />} Save allowances
              </Button>
            )}
          </div>
        </CardFooter>
      </Card>

      <datalist id="approval-role-slugs">
        <option value="owner" />
        {ROLE_SLUGS.map((r) => <option key={r} value={r} />)}
      </datalist>
      <datalist id="approval-snapshot-keys">
        <option value="responsible_user_id" />
      </datalist>
    </div>
  );
}
