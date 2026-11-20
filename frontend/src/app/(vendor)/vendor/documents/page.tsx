"use client";

import { useEffect, useMemo, useState, useCallback, useRef } from "react";
import { portalVendorApi } from "@/lib/api";
import { downloadStamped } from "@/lib/documentLibrary";
import { FileText, Plus, CheckCircle2, Clock, Trash2, Upload, AlertTriangle } from "lucide-react";
import {
  Button, DataTable, Dialog, Field, FieldGrid, IconButton, Input, ListFrame, PageHeader, Pill, Select, Stat, StatGroup,
  Textarea, toast, type ColumnDef,
} from "@/components/ui";
import { cn } from "@/lib/utils";
import { useDropToInput } from "@/hooks/useFileDrop";

interface VendorDocument {
  id: string;
  document_type: string;
  name: string;
  file_url?: string;
  signed_at?: string | null;
  expires_at?: string | null;
  is_verified: boolean;
  notes?: string | null;
}

const DOC_TYPES = [
  "nda", "service_agreement", "certificate", "insurance",
  "license", "tax_registration", "other",
];

const EMPTY = { document_type: "nda", name: "", signed_at: "", expires_at: "", notes: "" };
const fmtDate = (d?: string | null) => d ? new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "-";
const typeLabel = (t: string) => { const s = t.replace(/_/g, " "); return s.charAt(0).toUpperCase() + s.slice(1); };

function isExpiringSoon(expires_at?: string | null): boolean {
  if (!expires_at) return false;
  const expiry = new Date(expires_at);
  const cutoff = new Date();
  cutoff.setDate(cutoff.getDate() + 30);
  return expiry <= cutoff;
}

export default function VendorDocumentsPage() {
  const [docs, setDocs] = useState<VendorDocument[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const { dragActive, dropHandlers } = useDropToInput(fileRef);
  const [form, setForm] = useState(EMPTY);
  const [selectedFile, setSelectedFile] = useState<File | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await portalVendorApi.listMyDocuments();
      setDocs(res.data.items ?? res.data ?? []);
    } catch { setDocs([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  async function upload() {
    if (!form.name.trim() || !selectedFile) return;
    setSaving(true);
    try {
      const fd = new FormData();
      fd.append("document_type", form.document_type);
      fd.append("name", form.name.trim());
      if (form.signed_at) fd.append("signed_at", form.signed_at);
      if (form.expires_at) fd.append("expires_at", form.expires_at);
      if (form.notes.trim()) fd.append("notes", form.notes.trim());
      fd.append("file", selectedFile);
      await portalVendorApi.uploadDocument(fd);
      setShowModal(false);
      setForm(EMPTY);
      setSelectedFile(null);
      await load();
    } finally { setSaving(false); }
  }

  async function remove(id: string) {
    setDeleting(id);
    try {
      await portalVendorApi.deleteDocument(id);
      await load();
    } finally { setDeleting(null); }
  }

  // The file comes through the API (signed in), never as a bare storage key in a link.
  async function download(doc: VendorDocument) {
    try {
      await downloadStamped(() => portalVendorApi.downloadDocument(doc.id), doc.name);
    } catch { toast.error("The file could not be downloaded."); }
  }

  const verified = docs.filter((d) => d.is_verified).length;

  function close() { setShowModal(false); setSelectedFile(null); setForm(EMPTY); }

  const columns = useMemo<ColumnDef<VendorDocument>[]>(() => [
    { accessorKey: "name", header: "Document", cell: ({ row }) => (
      <span className="flex items-center gap-2 font-medium">
        <FileText className="size-4 shrink-0 text-fg-subtle" aria-hidden />
        {row.original.file_url
          ? <button type="button" className="text-left hover:underline" onClick={() => download(row.original)}>{row.original.name}</button>
          : row.original.name}
      </span>
    ) },
    { accessorKey: "document_type", header: "Type", meta: { hideOnMobile: true }, cell: (c) => typeLabel(c.getValue<string>()) },
    { accessorKey: "signed_at", header: "Signed", meta: { numeric: true, hideOnMobile: true }, cell: (c) => fmtDate(c.getValue<string>()) },
    { accessorKey: "expires_at", header: "Expires", meta: { numeric: true },
      cell: (c) => {
        const v = c.getValue<string | null>();
        if (!v) return "-";
        const soon = isExpiringSoon(v);
        return (
          <span className={cn("inline-flex items-center gap-1", soon && "font-medium text-tone-red")}>
            {soon && <AlertTriangle className="size-3.5" aria-label="Expires within 30 days" />}{fmtDate(v)}
          </span>
        );
      } },
    { accessorKey: "is_verified", header: "Status",
      cell: (c) => c.getValue<boolean>()
        ? <Pill tone="blue" icon={CheckCircle2}>Verified</Pill>
        : <Pill tone="orange" icon={Clock}>Pending review</Pill> },
    { id: "actions", header: "", enableSorting: false,
      cell: ({ row }) => !row.original.is_verified && (
        <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${row.original.name}`}
          className="text-fg-muted hover:text-tone-red" disabled={deleting === row.original.id}
          onClick={() => remove(row.original.id)} />
      ) },
  // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [deleting]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={docs.length} />
        <Stat label="Verified" value={verified} />
        <Stat label="Pending review" value={docs.length - verified} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="My documents" summary="Compliance documents, agreements, and certificates."
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Upload document</Button>} />
        <DataTable label="Documents" columns={columns} data={docs} getRowId={(d) => d.id} loading={loading}
          empty={{ icon: FileText, title: "No documents uploaded yet", description: "Upload your NDA, agreements and certificates for review.",
            action: <Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Upload document</Button> }} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={(o) => { if (!o) close(); }} title="Upload document" dismissable={!saving}
        footer={<>
          <Button onClick={close}>Cancel</Button>
          <Button variant="primary" onClick={upload} loading={saving} disabled={!form.name.trim() || !selectedFile}>{saving ? "Uploading..." : "Upload"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Document type" required>
            <Select value={form.document_type} onChange={(e) => setForm({ ...form, document_type: e.target.value })}
              options={DOC_TYPES.map((t) => ({ value: t, label: typeLabel(t) }))} />
          </Field>
          <Field label="Document name" required>
            <Input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="e.g. Company NDA 2026" />
          </Field>
          <Field label="Signed date">
            <Input type="date" value={form.signed_at} onChange={(e) => setForm({ ...form, signed_at: e.target.value })} />
          </Field>
          <Field label="Expiry date">
            <Input type="date" value={form.expires_at} onChange={(e) => setForm({ ...form, expires_at: e.target.value })} />
          </Field>
          <Field label="Notes" className="md:col-span-2">
            <Textarea rows={2} value={form.notes} onChange={(e) => setForm({ ...form, notes: e.target.value })} />
          </Field>
          <div className="md:col-span-2">
            <p className="mb-1.5 text-[13px] font-medium text-fg">File<span className="ml-0.5 text-brand-ink" aria-hidden>*</span></p>
            <button type="button" onClick={() => fileRef.current?.click()} {...dropHandlers}
              className={cn("flex w-full flex-col items-center rounded-lg border-2 border-dashed px-4 py-6 text-center transition-colors",
                dragActive ? "border-brand bg-brand/5" : "border-line-strong hover:border-brand/40")}>
              <Upload className="mb-2 size-5 text-fg-subtle" aria-hidden />
              {selectedFile ? (
                <span className="text-sm font-medium text-fg">{selectedFile.name}</span>
              ) : (
                <span className="text-sm text-fg-muted">{dragActive ? "Drop the file here" : "Drag and drop, or click to choose a file"}</span>
              )}
            </button>
            <input ref={fileRef} type="file" className="hidden" aria-label="File" accept=".pdf,.png,.jpg,.jpeg,.webp,.docx,.xlsx"
              onChange={(e) => setSelectedFile(e.target.files?.[0] ?? null)} />
          </div>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
