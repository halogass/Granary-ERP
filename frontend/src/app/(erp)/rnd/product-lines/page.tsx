"use client";
import { useEffect, useMemo, useState, useCallback } from "react";
import { Plus, Pencil, Trash2, Package } from "lucide-react";
import { catalogApi, salesApi } from "@/lib/api";
import {
  Button, Checkbox, DataTable, Dialog, Field, FieldGrid, IconButton, Input, ListFrame, PageHeader, Select,
  StatusPill, Textarea, type ColumnDef,
} from "@/components/ui";

interface ProductLine {
  id: string; code: string; name: string; category: string;
  color: string; description?: string | null; is_active: boolean; sort_order: number;
}

const CATEGORIES = ["software", "hardware", "service", "solution"];
// color: the new line's own color (data, editable per line)
const BLANK = { code: "", name: "", category: "software", color: "#6366f1", description: "", is_active: true, sort_order: 0 };

export default function ProductLinesPage() {
  const [lines, setLines] = useState<ProductLine[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [editing, setEditing] = useState<ProductLine | null>(null);
  const [form, setForm] = useState({ ...BLANK });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try { const r = await catalogApi.list({ include_inactive: true }); setLines(r.data ?? []); }
    catch { setLines([]); } finally { setLoading(false); }
  }, []);
  useEffect(() => { load(); }, [load]);

  // Only the Owner and the Executive role edit the catalog.
  const [canEdit, setCanEdit] = useState(false);
  useEffect(() => {
    salesApi.catalogAccess().then((r) => setCanEdit(!!r.data.can_edit)).catch(() => setCanEdit(false));
  }, []);

  function openNew() { setEditing(null); setForm({ ...BLANK }); setError(""); setShowModal(true); }
  function openEdit(l: ProductLine) {
    setEditing(l);
    setForm({ code: l.code, name: l.name, category: l.category, color: l.color, description: l.description ?? "", is_active: l.is_active, sort_order: l.sort_order });
    setError(""); setShowModal(true);
  }

  async function save() {
    if (!form.name.trim() || (!editing && !form.code.trim())) { setError("Code and name are required"); return; }
    setSaving(true); setError("");
    try {
      if (editing) {
        await catalogApi.update(editing.id, {
          name: form.name, category: form.category, color: form.color,
          description: form.description || null, is_active: form.is_active, sort_order: form.sort_order,
        });
      } else {
        await catalogApi.create(form);
      }
      setShowModal(false);
      await load();
    } catch (e: any) { // eslint-disable-line @typescript-eslint/no-explicit-any
      setError(e?.response?.data?.detail ?? "Could not save the product line");
    } finally { setSaving(false); }
  }

  async function remove(l: ProductLine) {
    if (!confirm(`Delete product line "${l.name}"?`)) return;
    try { await catalogApi.delete(l.id); await load(); } catch { /* ignore */ }
  }

  const columns = useMemo<ColumnDef<ProductLine>[]>(() => [
    {
      accessorKey: "name", header: "Product",
      cell: (c) => (
        <div className="flex items-center gap-2.5">
          {/* the line's own color (data, set per product line) */}
          <span className="size-3 shrink-0 rounded-full" style={{ background: c.row.original.color }} aria-hidden />
          <span className="font-medium text-fg">{c.row.original.name}</span>
        </div>
      ),
    },
    { accessorKey: "code", header: "Code", cell: (c) => <code className="font-mono text-xs text-fg-muted">{c.getValue<string>()}</code> },
    { accessorKey: "category", header: "Category", meta: { hideOnMobile: true }, cell: (c) => <span className="capitalize text-fg-muted">{c.getValue<string>()}</span> },
    { accessorKey: "is_active", header: "Status", cell: (c) => <StatusPill status={c.getValue<boolean>() ? "active" : "inactive"} /> },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => canEdit ? (
        <div className="flex items-center justify-end gap-1">
          <IconButton size="sm" icon={Pencil} label={`Edit ${c.row.original.name}`} onClick={() => openEdit(c.row.original)} />
          <IconButton size="sm" icon={Trash2} label={`Delete ${c.row.original.name}`} onClick={() => remove(c.row.original)} />
        </div>
      ) : null,
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [canEdit]);

  return (
    <div className="mx-auto max-w-5xl">
      <ListFrame>
        <PageHeader flush title="Product lines"
          summary="Shared catalog of the company's product lines, used across R&D, manufacturing and products."
          actions={canEdit && <Button variant="primary" onClick={openNew}><Plus aria-hidden /> New product line</Button>} />
        <DataTable label="Product lines" columns={columns} data={lines} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: Package, title: "No product lines yet." }} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title={editing ? "Edit product line" : "New product line"}
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={save} loading={saving}>{saving ? "Saving..." : editing ? "Save" : "Create"}</Button>
        </>}>
        <div className="space-y-4">
          <FieldGrid cols={2}>
            <Field label="Code" required>
              <Input value={form.code} disabled={!!editing} onChange={(e) => setForm({ ...form, code: e.target.value })} placeholder="inventory_app" className="font-mono" />
            </Field>
            <Field label="Name" required>
              <Input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="Customer Portal" />
            </Field>
            <Field label="Category">
              <Select value={form.category} onChange={(e) => setForm({ ...form, category: e.target.value })}
                className="capitalize" options={CATEGORIES.map((c) => ({ value: c, label: c }))} />
            </Field>
            <Field label="Color">
              <div className="flex items-center gap-2">
                <input type="color" aria-label="Pick color" value={form.color} onChange={(e) => setForm({ ...form, color: e.target.value })}
                  className="h-9 w-10 cursor-pointer rounded-lg border border-line-strong bg-surface p-0.5" />
                <Input value={form.color} onChange={(e) => setForm({ ...form, color: e.target.value })} className="font-mono" />
              </div>
            </Field>
          </FieldGrid>
          <Field label="Description">
            <Textarea value={form.description} onChange={(e) => setForm({ ...form, description: e.target.value })} rows={2} />
          </Field>
          <Checkbox label="Active" checked={form.is_active} onChange={(e) => setForm({ ...form, is_active: e.target.checked })} />
          {error && <p className="text-sm text-tone-red" role="alert">{error}</p>}
        </div>
      </Dialog>
    </div>
  );
}
