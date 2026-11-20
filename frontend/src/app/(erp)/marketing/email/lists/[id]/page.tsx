"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState } from "react";
import { useParams } from "next/navigation";
import { Plus, Trash2, UserCheck, UserX, Loader2, Upload, Search, Users } from "lucide-react";
import { emailMarketingApi } from "@/lib/api";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, ListFrame, PageHeader, Pill,
  Tabs, TabsContent, TabsList, TabsTrigger, Textarea, type ColumnDef,
} from "@/components/ui";

interface Contact {
  id: string; email: string; first_name?: string; last_name?: string;
  company?: string; is_subscribed: boolean; created_at: string;
}
interface ListDetail { id: string; name: string; description?: string; subscriber_count: number }

export default function ListDetailPage() {
  const { id }     = useParams<{ id: string }>();
  const [list, setList]       = useState<ListDetail | null>(null);
  const [contacts, setContacts] = useState<Contact[]>([]);
  const [loading, setLoading]   = useState(true);
  const [showAdd, setShowAdd]   = useState(false);
  const [saving, setSaving]     = useState(false);
  const [search, setSearch]     = useState("");
  const [form, setForm]         = useState({ email: "", first_name: "", last_name: "", company: "" });
  const [csvText, setCsvText]   = useState("");
  const [tab, setTab]           = useState<"manual" | "csv">("manual");

  async function load() {
    setLoading(true);
    const [lr, cr] = await Promise.all([
      emailMarketingApi.getList(id),
      fetchAllPages<Contact>((p) => emailMarketingApi.listContacts(id, p)),
    ]);
    setList(lr.data);
    setContacts(cr.rows);
    setLoading(false);
  }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { load(); }, [id]);

  async function addContact() {
    if (!form.email.trim()) return;
    setSaving(true);
    await emailMarketingApi.addContact(id, form);
    setForm({ email: "", first_name: "", last_name: "", company: "" });
    setShowAdd(false);
    setSaving(false);
    load();
  }

  async function importCsv() {
    const lines = csvText.trim().split("\n").filter(Boolean);
    setSaving(true);
    for (const line of lines) {
      const [email, first_name = "", last_name = "", company = ""] = line.split(",").map((s) => s.trim().replace(/^"|"$/g, ""));
      if (email && email.includes("@")) {
        await emailMarketingApi.addContact(id, { email, first_name, last_name, company }).catch(() => {});
      }
    }
    setCsvText("");
    setShowAdd(false);
    setSaving(false);
    load();
  }

  async function del(contactId: string, email: string) {
    if (!confirm(`Remove ${email} from this list?`)) return;
    await emailMarketingApi.deleteContact(id, contactId);
    load();
  }

  const filtered = contacts.filter((c) =>
    !search || c.email.toLowerCase().includes(search.toLowerCase()) ||
    `${c.first_name} ${c.last_name}`.toLowerCase().includes(search.toLowerCase())
  );

  const columns = useMemo<ColumnDef<Contact>[]>(() => [
    { accessorKey: "email", header: "Email", cell: (c) => <span className="text-fg">{c.getValue<string>()}</span> },
    { id: "name", accessorFn: (c) => [c.first_name, c.last_name].filter(Boolean).join(" "), header: "Name", cell: (c) => <span className="text-fg-muted">{c.getValue<string>() || "-"}</span> },
    { accessorKey: "company", header: "Company", meta: { hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{c.getValue<string>() || "-"}</span> },
    {
      accessorKey: "is_subscribed", header: "Status",
      cell: (c) => c.getValue<boolean>()
        ? <Pill tone="blue" icon={UserCheck}>Active</Pill>
        : <Pill tone="neutral" icon={UserX}>Unsubscribed</Pill>,
    },
    { accessorKey: "created_at", header: "Added", meta: { numeric: true, hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{new Date(c.getValue<string>()).toLocaleDateString()}</span> },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => <IconButton size="sm" icon={Trash2} label={`Remove ${c.row.original.email}`} onClick={() => del(c.row.original.id, c.row.original.email)} />,
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], []);

  if (loading && !list) return <div className="flex justify-center py-20"><Loader2 className="size-6 animate-spin text-fg-muted" aria-label="Loading" /></div>;

  return (
    <>
      <ListFrame>
        <PageHeader flush title={list?.name ?? "Mailing list"} back={{ href: "/marketing/email/lists", label: "Mailing lists" }}
          summary={<>{list?.subscriber_count.toLocaleString()} active subscribers{list?.description ? `, ${list.description}` : ""}</>}
          actions={<Button variant="primary" onClick={() => setShowAdd(true)}><Plus aria-hidden /> Add contacts</Button>} />
        <FilterChips chips={[]} onToggle={() => {}} className="justify-start">
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            placeholder="Search contacts" aria-label="Search contacts" value={search} onChange={(e) => setSearch(e.target.value)} className="w-64" />
        </FilterChips>
        <DataTable label="Contacts" columns={columns} data={filtered} getRowId={(r) => r.id} loading={loading}
          empty={contacts.length === 0
            ? { icon: Users, title: "No contacts yet", description: "Add your first contact, or import a CSV." }
            : { title: "No contacts match your search." }} />
      </ListFrame>

      <Dialog open={showAdd} onOpenChange={setShowAdd} title="Add contacts" size="lg">
        <Tabs value={tab} onValueChange={(v) => setTab(v as "manual" | "csv")}>
          <TabsList>
            <TabsTrigger value="manual">Manual entry</TabsTrigger>
            <TabsTrigger value="csv">Import CSV</TabsTrigger>
          </TabsList>
          <TabsContent value="manual" className="pt-4">
            <FieldGrid cols={2}>
              <Field label="Email" required>
                <Input type="email" value={form.email} onChange={(e) => setForm({ ...form, email: e.target.value })} />
              </Field>
              <Field label="Company">
                <Input value={form.company} onChange={(e) => setForm({ ...form, company: e.target.value })} />
              </Field>
              <Field label="First name">
                <Input value={form.first_name} onChange={(e) => setForm({ ...form, first_name: e.target.value })} />
              </Field>
              <Field label="Last name">
                <Input value={form.last_name} onChange={(e) => setForm({ ...form, last_name: e.target.value })} />
              </Field>
            </FieldGrid>
            <div className="mt-4 flex justify-end gap-2">
              <Button onClick={() => setShowAdd(false)}>Cancel</Button>
              <Button variant="primary" onClick={addContact} loading={saving} disabled={!form.email.trim()}>Add contact</Button>
            </div>
          </TabsContent>
          <TabsContent value="csv" className="pt-4">
            <Field label="Contacts" hint={<>One contact per line: <code className="font-mono">email, first_name, last_name, company</code></>}>
              <Textarea className="font-mono" rows={6} value={csvText} onChange={(e) => setCsvText(e.target.value)}
                placeholder={"jane@example.com, Jane, Doe, Acme Corp\njohn@example.com, John, Smith,"} />
            </Field>
            <div className="mt-4 flex justify-end gap-2">
              <Button onClick={() => setShowAdd(false)}>Cancel</Button>
              <Button variant="primary" onClick={importCsv} loading={saving} disabled={!csvText.trim()}>
                {!saving && <Upload aria-hidden />} Import
              </Button>
            </div>
          </TabsContent>
        </Tabs>
      </Dialog>
    </>
  );
}
