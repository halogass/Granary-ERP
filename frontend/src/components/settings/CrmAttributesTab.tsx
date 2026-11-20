"use client";

import { useCallback, useEffect, useState } from "react";
import { Plus, Trash2, Zap } from "lucide-react";
import {
  Button, Card, CardHeader, Checkbox, DataTable, Dialog, Field, FieldGrid, IconButton, Input, Pill, Select,
  Textarea, tabTriggerClass, type ColumnDef,
} from "@/components/ui";
import { crmRecordsApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import type { AttributeDef } from "@/components/crm/AttributePanel";

const OBJECT_TYPES = [
  { id: "organization", label: "Organizations" },
  { id: "contact", label: "Contacts" },
  { id: "lead", label: "Leads" },
  { id: "deal", label: "Deals" },
] as const;

const FIELD_TYPES = [
  "text", "long_text", "number", "currency", "date", "select", "multi_select",
  "checkbox", "url", "email", "phone", "rating", "user_ref", "record_ref",
];

const AI_KINDS = [
  { id: "summary", label: "Summary: free text from the record and its history" },
  { id: "classify", label: "Classify: pick one value against a rubric" },
  { id: "research", label: "Web research: not available yet" },
];

const BLANK = {
  object_type: "organization",
  key: "",
  label: "",
  field_type: "text",
  is_ai: false,
  ai_kind: "summary",
  rubric: "",
  instruction: "",
  choices: "",
};

export default function CrmAttributesTab() {
  const [objectType, setObjectType] = useState<string>("organization");
  const [defs, setDefs] = useState<AttributeDef[]>([]);
  const [loading, setLoading] = useState(true);
  const [showForm, setShowForm] = useState(false);
  const [form, setForm] = useState({ ...BLANK });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const r = await crmRecordsApi.listAttributes({ object_type: objectType });
      setDefs(r.data.items ?? []);
    } catch {
      setDefs([]);
    } finally { setLoading(false); }
  }, [objectType]);

  useEffect(() => { load(); }, [load]);

  function openForm() {
    setForm({ ...BLANK, object_type: objectType });
    setError("");
    setShowForm(true);
  }

  // The key addresses the value inside every record's `custom` map, so it's derived from
  // the label once and then left alone.
  function onLabelChange(label: string) {
    setForm((f) => ({
      ...f,
      label,
      key: f.key || label.toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "").slice(0, 64),
    }));
  }

  async function save() {
    setSaving(true); setError("");
    const choices = form.choices.split("\n").map((c) => c.trim()).filter(Boolean);
    const isClassify = form.is_ai && form.ai_kind === "classify";
    try {
      await crmRecordsApi.createAttribute({
        object_type: form.object_type,
        key: form.key,
        label: form.label,
        // A classify attribute stores one of a fixed set, so it is a select underneath.
        field_type: isClassify ? "select" : form.field_type,
        options: (isClassify || form.field_type === "select" || form.field_type === "multi_select")
          ? { choices } : {},
        is_ai: form.is_ai,
        ai_kind: form.is_ai ? form.ai_kind : null,
        ai_config: form.is_ai
          ? (form.ai_kind === "classify" ? { rubric: form.rubric, choices } : { instruction: form.instruction })
          : {},
      });
      setShowForm(false);
      await load();
    } catch (e) {
      setError(extractApiError(e, "Could not create the attribute."));
    } finally { setSaving(false); }
  }

  async function remove(id: string) {
    try {
      await crmRecordsApi.deleteAttribute(id);
      await load();
    } catch (e) {
      setError(extractApiError(e, "Could not remove the attribute."));
    }
  }

  const needsChoices = form.is_ai
    ? form.ai_kind === "classify"
    : form.field_type === "select" || form.field_type === "multi_select";

  const columns: ColumnDef<AttributeDef>[] = [
    { accessorKey: "label", header: "Label", cell: ({ row }) => (
      <span className="inline-flex items-center gap-1.5">
        <span className="font-medium">{row.original.label}</span>
        {row.original.is_ai && <Pill tone="violet" icon={Zap}>{row.original.ai_kind}</Pill>}
      </span>
    ) },
    { accessorKey: "key", header: "Key", cell: ({ row }) => <span className="font-mono text-[13px] text-fg-muted">{row.original.key}</span> },
    { accessorKey: "field_type", header: "Type", meta: { hideOnMobile: true }, cell: ({ row }) => (
      <span className="text-fg-muted">
        {row.original.field_type.replace(/_/g, " ")}
        {row.original.options?.choices?.length ? `, ${row.original.options.choices.length} choices` : ""}
      </span>
    ) },
    { id: "remove", header: "", enableSorting: false, meta: { width: "48px" }, cell: ({ row }) => (
      <IconButton label={`Deactivate ${row.original.label}`} icon={Trash2} onClick={() => remove(row.original.id)} />
    ) },
  ];

  const typeLabel = OBJECT_TYPES.find((t) => t.id === objectType)?.label.toLowerCase();

  return (
    <div className="space-y-3">
      <Card className="overflow-hidden">
        <CardHeader description="Custom fields on CRM records. AI attributes fill themselves on demand."
          actions={<Button size="sm" onClick={openForm}><Plus aria-hidden /> New attribute</Button>} />
        <nav aria-label="Record type" className="flex gap-1 overflow-x-auto px-2 shadow-[inset_0_-1px_0_rgb(var(--line))]">
          {OBJECT_TYPES.map((t) => (
            <button key={t.id} type="button" onClick={() => setObjectType(t.id)}
              aria-current={objectType === t.id ? "page" : undefined} className={tabTriggerClass}>
              {t.label}
            </button>
          ))}
        </nav>
        <DataTable label={`Attributes on ${typeLabel}`} columns={columns} data={defs} getRowId={(d) => d.id}
          loading={loading} maxHeight="none"
          empty={{ title: `No attributes defined for ${typeLabel} yet`, description: "Add one with New attribute.", compact: true }} />
      </Card>

      {error && !showForm && (
        <p role="alert" className="rounded-lg border border-tone-red/30 bg-tone-red-soft px-4 py-2.5 text-sm text-tone-red">{error}</p>
      )}

      <Dialog open={showForm} onOpenChange={setShowForm} title="New attribute" size="lg" dismissable={!saving}
        footer={<>
          <Button onClick={() => setShowForm(false)} disabled={saving}>Cancel</Button>
          <Button variant="primary" onClick={save} loading={saving} disabled={!form.label || !form.key}>Create attribute</Button>
        </>}>
        <div className="space-y-4">
          <FieldGrid cols={2}>
            <Field label="Applies to">
              <Select value={form.object_type} onChange={(e) => setForm({ ...form, object_type: e.target.value })}
                options={OBJECT_TYPES.map((t) => ({ value: t.id, label: t.label }))} />
            </Field>
            <Field label="Label">
              <Input value={form.label} onChange={(e) => onLabelChange(e.target.value)} placeholder="e.g. ICP Tier" />
            </Field>
          </FieldGrid>

          <Field label="Key" hint="Permanent: it addresses the stored value.">
            <Input value={form.key} onChange={(e) => setForm({ ...form, key: e.target.value })} placeholder="icp_tier" className="font-mono" />
          </Field>

          <Checkbox label="Fill this field with AI" checked={form.is_ai}
            onChange={(e) => setForm({ ...form, is_ai: e.target.checked })} />

          {form.is_ai ? (
            <div className="space-y-4 border-l-2 border-brand/20 pl-4">
              <Field label="AI kind">
                <Select value={form.ai_kind} onChange={(e) => setForm({ ...form, ai_kind: e.target.value })}
                  options={AI_KINDS.map((k) => ({ value: k.id, label: k.label, disabled: k.id === "research" }))} />
              </Field>
              {form.ai_kind === "classify" ? (
                <Field label="Rubric">
                  <Textarea rows={5} value={form.rubric} onChange={(e) => setForm({ ...form, rubric: e.target.value })}
                    placeholder={"Tier 1: manufacturer, 100+ employees, active deal over USD 20,000.\nTier 2: manufacturer under 100 employees, or no active deal.\nTier 3: non-manufacturer."}
                    className="font-mono text-xs" />
                </Field>
              ) : (
                <Field label="Instruction">
                  <Textarea rows={3} value={form.instruction} onChange={(e) => setForm({ ...form, instruction: e.target.value })}
                    placeholder="Summarize the last 90 days of this relationship in two sentences." />
                </Field>
              )}
            </div>
          ) : (
            <Field label="Field type">
              <Select value={form.field_type} onChange={(e) => setForm({ ...form, field_type: e.target.value })}
                options={FIELD_TYPES.map((t) => ({ value: t, label: t.replace(/_/g, " ") }))} />
            </Field>
          )}

          {needsChoices && (
            <Field label="Choices" hint="One per line.">
              <Textarea rows={4} value={form.choices} onChange={(e) => setForm({ ...form, choices: e.target.value })}
                placeholder={"Tier 1\nTier 2\nTier 3\nNot a fit"} />
            </Field>
          )}

          {error && (
            <p role="alert" className="rounded-lg border border-tone-red/30 bg-tone-red-soft px-4 py-2.5 text-sm text-tone-red">{error}</p>
          )}
        </div>
      </Dialog>
    </div>
  );
}
