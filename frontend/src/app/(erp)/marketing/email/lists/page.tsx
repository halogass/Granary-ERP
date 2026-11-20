"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { Plus, Trash2, Users } from "lucide-react";
import { emailMarketingApi } from "@/lib/api";
import {
  Button, DataTable, Dialog, Field, IconButton, Input, ListFrame, PageHeader, type ColumnDef,
} from "@/components/ui";

interface MailingList { id: string; name: string; description?: string; subscriber_count: number; created_at: string }

export default function MailingListsPage() {
  const [lists, setLists]     = useState<MailingList[]>([]);
  const [loading, setLoading] = useState(true);
  const [showNew, setShowNew] = useState(false);
  const [form, setForm]       = useState({ name: "", description: "" });
  const [saving, setSaving]   = useState(false);

  async function load() {
    setLoading(true);
    const { rows } = await fetchAllPages<MailingList>(emailMarketingApi.listLists);
    setLists(rows);
    setLoading(false);
  }
  useEffect(() => { load(); }, []);

  async function create() {
    if (!form.name.trim()) return;
    setSaving(true);
    await emailMarketingApi.createList({ name: form.name.trim(), description: form.description || undefined });
    setForm({ name: "", description: "" });
    setShowNew(false);
    setSaving(false);
    load();
  }

  async function del(id: string, name: string) {
    if (!confirm(`Delete list "${name}"? This cannot be undone.`)) return;
    await emailMarketingApi.deleteList(id);
    load();
  }

  const columns = useMemo<ColumnDef<MailingList>[]>(() => [
    {
      accessorKey: "name", header: "Name",
      cell: (c) => <Link href={`/marketing/email/lists/${c.row.original.id}`} className="font-medium text-fg hover:text-brand-ink">{c.getValue<string>()}</Link>,
    },
    { accessorKey: "description", header: "Description", meta: { hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{c.getValue<string>() || "-"}</span> },
    { accessorKey: "subscriber_count", header: "Subscribers", meta: { numeric: true }, cell: (c) => <span className="font-medium">{c.getValue<number>().toLocaleString()}</span> },
    { accessorKey: "created_at", header: "Created", meta: { numeric: true, hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{new Date(c.getValue<string>()).toLocaleDateString()}</span> },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => <IconButton size="sm" icon={Trash2} label={`Delete ${c.row.original.name}`} onClick={() => del(c.row.original.id, c.row.original.name)} />,
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], []);

  return (
    <>
      <ListFrame>
        <PageHeader flush title="Mailing lists" summary="Manage your subscriber lists"
          back={{ href: "/marketing/email", label: "Email marketing" }}
          actions={<Button variant="primary" onClick={() => setShowNew(true)}><Plus aria-hidden /> New list</Button>} />
        <DataTable label="Mailing lists" columns={columns} data={lists} getRowId={(r) => r.id} loading={loading}
          rowHref={(l) => `/marketing/email/lists/${l.id}`}
          empty={{
            icon: Users, title: "No mailing lists yet", description: "Create your first list to get started.",
            action: <Button variant="primary" onClick={() => setShowNew(true)}><Plus aria-hidden /> Create list</Button>,
          }} />
      </ListFrame>

      <Dialog open={showNew} onOpenChange={setShowNew} title="Create new list"
        footer={<>
          <Button onClick={() => setShowNew(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} loading={saving} disabled={!form.name.trim()}>Create list</Button>
        </>}>
        <div className="space-y-4">
          <Field label="List name" required>
            <Input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="e.g. Newsletter subscribers" />
          </Field>
          <Field label="Description">
            <Input value={form.description} onChange={(e) => setForm({ ...form, description: e.target.value })} placeholder="Optional description" />
          </Field>
        </div>
      </Dialog>
    </>
  );
}
