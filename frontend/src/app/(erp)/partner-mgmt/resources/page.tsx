"use client";

import { useEffect, useMemo, useState, useCallback } from "react";
import { partnerApi } from "@/lib/api";
import {
  FolderOpen, Plus, FileText, Video, BookOpen, DollarSign, Monitor, Code2, Image, Trash2, Search,
} from "lucide-react";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, ListFrame, PageHeader, SavedViewTabs,
  Select, Stat, StatGroup, StatusPill, Switch, Textarea, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface PartnerResource {
  id: string;
  title: string;
  description?: string;
  resource_type: string;
  file_url?: string;
  min_tier: string;
  version?: string;
  download_count: number;
  is_active: boolean;
}

const RESOURCE_TYPES = [
  { key: "datasheet",     label: "Datasheet",    icon: FileText },
  { key: "case_study",    label: "Case Study",   icon: BookOpen },
  { key: "presentation",  label: "Presentation", icon: Monitor },
  { key: "training",      label: "Training",     icon: Video },
  { key: "branding",      label: "Branding",     icon: Image },
  { key: "price_list",    label: "Price List",   icon: DollarSign },
  { key: "api_doc",       label: "API Doc",      icon: Code2 },
];

const TIERS = ["authorized", "silver", "gold", "platinum"];

type Filters = { active?: boolean };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "active", label: "Active", filters: { active: true } },
];

function typeIcon(type: string): React.ElementType {
  return RESOURCE_TYPES.find((t) => t.key === type)?.icon ?? FileText;
}
function typeLabel(type: string): string {
  return RESOURCE_TYPES.find((t) => t.key === type)?.label ?? type;
}

export default function PartnerResourcesPage() {
  const views = useSavedViews<Filters>("partner.resources", VIEWS);
  const [search, setSearch] = useState("");
  const [resources, setResources] = useState<PartnerResource[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [filterType, setFilterType] = useState("all");
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [form, setForm] = useState({
    title: "", description: "", resource_type: "datasheet",
    file_url: "", min_tier: "authorized", version: "",
  });

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await partnerApi.listResources();
      setResources(res.data.items ?? res.data ?? []);
    } catch { setResources([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  async function create() {
    if (!form.title.trim()) return;
    setSaving(true);
    try {
      await partnerApi.createResource({
        title: form.title.trim(),
        description: form.description.trim() || undefined,
        resource_type: form.resource_type,
        file_url: form.file_url.trim() || undefined,
        min_tier: form.min_tier,
        version: form.version.trim() || undefined,
      });
      setShowModal(false);
      setForm({ title: "", description: "", resource_type: "datasheet", file_url: "", min_tier: "authorized", version: "" });
      await load();
    } finally { setSaving(false); }
  }

  async function toggleActive(resource: PartnerResource) {
    await partnerApi.updateResource(resource.id, { is_active: !resource.is_active });
    await load();
  }

  async function deleteResource(id: string) {
    try {
      await partnerApi.deleteResource(id);
      setConfirmDeleteId(null);
      setResources((prev) => prev.filter((r) => r.id !== id));
    } catch { /* keep */ }
  }

  const onlyActive = views.active?.filters.active;
  const q = search.trim().toLowerCase();
  const filtered = resources.filter((r) => (filterType === "all" || r.resource_type === filterType) &&
    (!onlyActive || r.is_active) && (!q || r.title.toLowerCase().includes(q)));

  const columns = useMemo<ColumnDef<PartnerResource>[]>(() => [
    {
      accessorKey: "title", header: "Resource",
      cell: (c) => {
        const r = c.row.original;
        const TypeIcon = typeIcon(r.resource_type);
        return (
          <div className={r.is_active ? "flex min-w-0 items-center gap-3" : "flex min-w-0 items-center gap-3 opacity-60"}>
            <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-surface-2">
              <TypeIcon className="size-4 text-brand-ink" aria-hidden />
            </span>
            <div className="min-w-0">
              <p className="truncate font-medium text-fg">{r.title}</p>
              {r.description && <p className="truncate text-xs text-fg-muted">{r.description}</p>}
            </div>
          </div>
        );
      },
    },
    { accessorKey: "resource_type", header: "Type", meta: { hideOnMobile: true }, cell: (c) => typeLabel(c.getValue<string>()) },
    { accessorKey: "min_tier", header: "Min tier", cell: (c) => <StatusPill family="partner_tier" status={c.getValue<string>()} /> },
    {
      accessorKey: "version", header: "Version", meta: { hideOnMobile: true },
      cell: (c) => <span className="font-mono text-xs text-fg-muted">{c.getValue<string | undefined>() || "-"}</span>,
    },
    { accessorKey: "download_count", header: "Downloads", meta: { numeric: true } },
    {
      accessorKey: "is_active", header: "Active", meta: { width: "1%" },
      cell: (c) => (
        <Switch checked={c.row.original.is_active} onCheckedChange={() => toggleActive(c.row.original)}
          aria-label={`${c.row.original.is_active ? "Deactivate" : "Activate"} ${c.row.original.title}`} />
      ),
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const r = c.row.original;
        return (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            {confirmDeleteId === r.id ? (
              <>
                <span className="text-xs font-semibold text-tone-red">Delete?</span>
                <Button size="sm" variant="danger" onClick={() => deleteResource(r.id)}>Yes</Button>
                <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
              </>
            ) : (
              <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${r.title}`} onClick={() => setConfirmDeleteId(r.id)} />
            )}
          </div>
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={resources.length} />
        <Stat label="Active" value={resources.filter((r) => r.is_active).length} />
        <Stat label="Types" value={new Set(resources.map((r) => r.resource_type)).size} />
        <Stat label="Downloads" value={resources.reduce((s, r) => s + r.download_count, 0)} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Partner resources" summary="Sales kits, training materials and documents for partners."
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Add resource</Button>} />
        <SavedViewTabs state={views} currentFilters={views.active?.filters ?? {}} />
        <FilterChips
          chips={RESOURCE_TYPES.map((t) => ({ id: t.key, label: t.label, active: filterType === t.key }))}
          onToggle={(id) => setFilterType((cur) => (cur === id ? "all" : id))} onClear={() => setFilterType("all")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search resources" aria-label="Search resources" className="w-56" />
        </FilterChips>
        <DataTable label="Partner resources" columns={columns} data={filtered} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: FolderOpen, title: "No resources yet", description: "Add sales kits, training materials and documents for partners." }} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Add partner resource"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} disabled={!form.title.trim()} loading={saving}>{saving ? "Adding" : "Add resource"}</Button>
        </>}>
        <FieldGrid cols={3}>
          <Field label="Title" required className="md:col-span-3">
            <Input value={form.title} onChange={(e) => setForm({ ...form, title: e.target.value })} placeholder="e.g. Granary Product Datasheet v2" />
          </Field>
          <Field label="Type" className="md:col-span-2">
            <Select value={form.resource_type} onChange={(e) => setForm({ ...form, resource_type: e.target.value })}
              options={RESOURCE_TYPES.map((t) => ({ value: t.key, label: t.label }))} />
          </Field>
          <Field label="Min tier">
            <Select value={form.min_tier} onChange={(e) => setForm({ ...form, min_tier: e.target.value })}
              options={TIERS.map((t) => ({ value: t, label: t.charAt(0).toUpperCase() + t.slice(1) }))} />
          </Field>
          <Field label="Description" className="md:col-span-3">
            <Textarea value={form.description} onChange={(e) => setForm({ ...form, description: e.target.value })} rows={2} />
          </Field>
          <Field label="File URL" className="md:col-span-2">
            <Input value={form.file_url} onChange={(e) => setForm({ ...form, file_url: e.target.value })} placeholder="https://..." />
          </Field>
          <Field label="Version">
            <Input value={form.version} onChange={(e) => setForm({ ...form, version: e.target.value })} placeholder="v2.1" />
          </Field>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
