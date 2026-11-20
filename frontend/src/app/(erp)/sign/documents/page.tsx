"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useState, useEffect, useCallback, useMemo } from "react";
import dynamic from "next/dynamic";
import {
  Plus, PenLine, CheckCircle2, X, Send, UserPlus, Trash2, ExternalLink,
  Layout, FileText, Users, MousePointer2, MailCheck, Activity, ChevronRight,
  ChevronLeft, ArrowRight, Download, RefreshCw, ShieldCheck, Search,
} from "lucide-react";
import SignerPickerModal from "@/components/sign/SignerPickerModal";
import { type FieldWidget } from "@/components/sign/PDFFieldEditor";
import api from "@/lib/api";
import { useSession } from "next-auth/react";
import {
  Button, Card, CardBody, CardHeader, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton,
  Input, KeyValues, ListFrame, PageHeader, Pill, SavedViewTabs, Select, Skeleton, StatusPill, Textarea, toast,
  useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";
import { cn } from "@/lib/utils";

const PDFFieldEditor = dynamic(() => import("@/components/sign/PDFFieldEditor"), { ssr: false });

interface Signer {
  id: string; name: string; email: string; sign_order: number; role: string;
  status: string; signed_at: string | null; token?: string;
  signer_type?: string; partner_account_id?: string;
}
interface Doc {
  id: string; title: string; status: string; source_type: string | null;
  document_hash: string | null; certificate_url: string | null; file_url: string | null;
  created_at: string; signer_count: number; signed_count: number;
  completed_at: string | null; message: string | null;
  sealed_at?: string | null; seal_fingerprint?: string | null;
  signature_level?: string | null; seal_level?: string | null;
  signers?: Signer[];
  fields?: FieldWidget[];
}

const SEAL_LEVEL_LABEL: Record<string, string> = {
  "pades-b-b": "PAdES-B-B", "pades-b-t": "PAdES-B-T (timestamped)",
  "pades-b-lt": "PAdES-B-LT", "pades-b-lta": "PAdES-B-LTA (long-term)",
};
const SIG_LEVEL_LABEL: Record<string, string> = {
  ses: "SES", aes: "AES", qes: "QES",
};

const STATUS_LABEL: Record<string, string> = {
  draft: "Draft", pending: "Pending", completed: "Completed", declined: "Declined", cancelled: "Cancelled",
};

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "draft", label: "Drafts", filters: { status: "draft" } },
  { id: "pending", label: "Pending", filters: { status: "pending" } },
  { id: "completed", label: "Completed", filters: { status: "completed" } },
  { id: "declined", label: "Declined", filters: { status: "declined" } },
];

const fmtShort = (d: string) => new Date(d).toLocaleDateString("en-GB", { day: "2-digit", month: "short" });

const STEPS = [
  { n: 1 as const, label: "Upload",  Icon: FileText },
  { n: 2 as const, label: "Signers", Icon: Users },
  { n: 3 as const, label: "Fields",  Icon: MousePointer2 },
  { n: 4 as const, label: "Invite",  Icon: MailCheck },
  { n: 5 as const, label: "Track",   Icon: Activity },
];

type Step = 1 | 2 | 3 | 4 | 5;

function deriveStep(doc: Doc): Step {
  if (doc.status !== "draft") return 5;
  if ((doc.signers?.length ?? 0) === 0) return 2;
  if ((doc.fields?.length ?? 0) === 0) return 3;
  return 4;
}

export default function SignDocumentsPage() {
  const { data: session } = useSession();
  // session.access_token is the correct field name from auth.ts session callback
  const authToken = (session as { access_token?: string } | null)?.access_token;
  const currentUserEmail = session?.user?.email?.toLowerCase() ?? "";

  const [docs, setDocs] = useState<Doc[]>([]);
  const [loading, setLoading] = useState(true);
  const views = useSavedViews<Filters>("sign.documents", VIEWS);
  const [chipStatus, setChipStatus] = useState("");
  const [search, setSearch] = useState("");
  const statusFilter = chipStatus || views.active?.filters.status || "";
  const [active, setActive] = useState<Doc | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);

  // Wizard state
  const [activeStep, setActiveStep] = useState<Step>(1);
  const [isCreatingNew, setIsCreatingNew] = useState(false);

  // Step 1: Upload
  const [createTitle, setCreateTitle] = useState("");
  const [createMessage, setCreateMessage] = useState("");
  const [createFile, setCreateFile] = useState<File | null>(null);
  const [creating, setCreating] = useState(false);
  const [dragActive, setDragActive] = useState(false);
  const [uploadError, setUploadError] = useState("");

  // Accept a chosen/dropped PDF: validate, set it, and prefill the title from the filename.
  const acceptFile = (file: File | null) => {
    setUploadError("");
    if (!file) return;
    if (file.type !== "application/pdf" && !file.name.toLowerCase().endsWith(".pdf")) {
      setUploadError("Only PDF files are supported.");
      return;
    }
    setCreateFile(file);
    if (!createTitle.trim()) {
      setCreateTitle(file.name.replace(/\.pdf$/i, "").replace(/[-_]+/g, " "));
    }
  };

  // Step 2: Signers
  const [signerForm, setSignerForm] = useState({ name: "", email: "", sign_order: "1", role: "signer" });
  const [addingSigner, setAddingSigner] = useState(false);
  const [showSignerPicker, setShowSignerPicker] = useState(false);

  // Step 3: Fields
  const [fields, setFields] = useState<FieldWidget[]>([]);
  const [savingFields, setSavingFields] = useState(false);
  const [showFieldEditor, setShowFieldEditor] = useState(false);

  // Step 4: Invite
  const [sending, setSending] = useState(false);
  const [selfSigning, setSelfSigning] = useState(false);

  // Step 5: Track
  const [remindingId, setRemindingId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const params: Record<string, string> = {};
      if (statusFilter) params.status = statusFilter;
      const { rows } = await fetchAllPages<Doc>((p) => api.get("/sign/documents", { params: p }), params);
      setDocs(rows);
    } catch { setDocs([]); } finally { setLoading(false); }
  }, [statusFilter]);

  useEffect(() => { load(); }, [load]);

  async function refreshActive(docId: string) {
    try {
      const res = await api.get(`/sign/documents/${docId}`);
      const detail = res.data as Doc;
      setActive(detail);
      setFields(detail.fields ?? []);
    } catch {}
  }

  async function openDetail(doc: Doc) {
    setIsCreatingNew(false);
    setActive(doc);
    setDetailLoading(true);
    try {
      const res = await api.get(`/sign/documents/${doc.id}`);
      const detail = res.data as Doc;
      setActive(detail);
      setFields(detail.fields ?? []);
      setActiveStep(deriveStep(detail));
    } catch {} finally { setDetailLoading(false); }
  }

  function startNew() {
    setActive(null);
    setIsCreatingNew(true);
    setActiveStep(1);
    setCreateTitle("");
    setCreateMessage("");
    setCreateFile(null);
  }

  async function createDocument() {
    if (!createTitle.trim() || !createFile) return toast.error("Title and PDF file are required");
    setCreating(true);
    try {
      const fd = new FormData();
      fd.append("title", createTitle.trim());
      if (createMessage) fd.append("message", createMessage);
      fd.append("file", createFile);
      const res = await api.post("/sign/documents", fd, { headers: { "Content-Type": "multipart/form-data" } });
      toast.success("Document uploaded");
      setCreateTitle(""); setCreateMessage(""); setCreateFile(null);
      load();
      const detail = await api.get(`/sign/documents/${res.data.id}`);
      setActive(detail.data as Doc);
      setFields([]);
      setIsCreatingNew(false);
      setActiveStep(2);
    } catch { toast.error("Failed to upload document"); } finally { setCreating(false); }
  }

  async function addSigner(overrides?: { name: string; email: string; signer_type?: string; partner_account_id?: string }) {
    const name = overrides?.name ?? signerForm.name.trim();
    const email = overrides?.email ?? signerForm.email.trim();
    if (!active || !name || !email) return toast.error("Name and email required");
    const docId = active.id;
    setAddingSigner(true);
    try {
      const res = await api.post(`/sign/documents/${docId}/signers`, {
        name, email,
        sign_order: parseInt(signerForm.sign_order) || 1,
        role: signerForm.role,
        signer_type: overrides?.signer_type ?? "external",
        partner_account_id: overrides?.partner_account_id ?? null,
      });
      toast.success("Signer added");
      setSignerForm({ name: "", email: "", sign_order: "1", role: "signer" });
      // Show the new signer straight away (the 201 body is the signer), so "Next: place fields" and the
      // Fields step open without waiting for the detail reload; the reload then brings the full document.
      const added = res?.data as Signer | undefined;
      if (added?.id) {
        setActive((cur) => {
          if (!cur || cur.id !== docId) return cur;
          const signers = [...(cur.signers ?? []).filter((x) => x.id !== added.id), added];
          return { ...cur, signers, signer_count: signers.length };
        });
      }
      await refreshActive(docId);
      load();
    } catch (err: unknown) {
      const msg = (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      toast.error(msg ?? "Failed to add signer");
    } finally { setAddingSigner(false); }
  }

  async function removeSigner(signerId: string) {
    if (!active) return;
    try {
      await api.delete(`/sign/documents/${active.id}/signers/${signerId}`);
      toast.success("Signer removed");
      await refreshActive(active.id);
      load();
    } catch { toast.error("Failed to remove signer"); }
  }

  async function saveFields() {
    if (!active) return;
    setSavingFields(true);
    try {
      await api.post(`/sign/documents/${active.id}/fields/batch`, { fields });
      toast.success(`${fields.length} field${fields.length !== 1 ? "s" : ""} saved`);
      setShowFieldEditor(false);
      await refreshActive(active.id);
      setActiveStep(4);
    } catch { toast.error("Failed to save fields"); }
    finally { setSavingFields(false); }
  }

  async function sendDocument() {
    if (!active) return;
    setSending(true);
    try {
      await api.post(`/sign/documents/${active.id}/send`);
      toast.success("Document sent for signing");
      load();
      await refreshActive(active.id);
      setActiveStep(5);
    } catch (err: unknown) {
      const msg = (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      toast.error(msg ?? "Failed to send");
    } finally { setSending(false); }
  }

  async function cancelDocument(id: string) {
    try {
      await api.delete(`/sign/documents/${id}`);
      toast.success("Document cancelled");
      if (active?.id === id) { setActive(null); setIsCreatingNew(false); }
      load();
    } catch { toast.error("Failed to cancel"); }
  }

  function saveBlob(data: BlobPart, filename: string, type = "application/pdf") {
    const url = URL.createObjectURL(new Blob([data], { type }));
    const a = document.createElement("a");
    a.href = url; a.download = filename; document.body.appendChild(a); a.click();
    a.remove(); URL.revokeObjectURL(url);
  }

  async function downloadSigned(doc: Doc) {
    try {
      const res = await api.get(`/sign/documents/${doc.id}/signed`, { responseType: "blob" });
      saveBlob(res.data, `${doc.title}_signed.pdf`);
    } catch { toast.error("Signed PDF not available yet. Try Finalize."); }
  }

  async function downloadCertificate(doc: Doc) {
    try {
      const res = await api.get(`/sign/documents/${doc.id}/certificate`, { responseType: "blob" });
      saveBlob(res.data, `${doc.title}_certificate.pdf`);
    } catch { toast.error("Certificate not available yet. Try Finalize."); }
  }

  async function finalizeDoc(doc: Doc) {
    try {
      await api.post(`/sign/documents/${doc.id}/finalize`);
      toast.success("Signed PDF & certificate generated");
      refreshActive(doc.id);
    } catch { toast.error("Could not finalize. Check the PDF service."); }
  }

  const [exporting, setExporting] = useState(false);
  async function downloadAll() {
    setExporting(true);
    try {
      const res = await api.get(`/sign/export?status=completed`, { responseType: "blob" });
      saveBlob(res.data, "signed_documents.zip", "application/zip");
    } catch { toast.error("No completed documents to export"); } finally { setExporting(false); }
  }

  async function remind(signerId: string) {
    if (!active) return;
    setRemindingId(signerId);
    try {
      await api.post(`/sign/documents/${active.id}/remind?signer_id=${signerId}`);
      toast.success("Reminder sent");
    } catch { toast.error("Failed to send reminder"); } finally { setRemindingId(null); }
  }

  async function selfSign() {
    if (!active) return;
    setSelfSigning(true);
    try {
      await api.post(`/sign/documents/${active.id}/self-sign`);
      toast.success("Added as signer. Use Sign now in the Track step when ready.");
      await refreshActive(active.id);
      load();
    } catch (err: unknown) {
      const msg = (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      toast.error(msg ?? "Failed to add self as signer");
    } finally { setSelfSigning(false); }
  }

  const showWizard = isCreatingNew || active !== null;
  const wizardStep: Step = activeStep;
  const sortedSigners = (active?.signers ?? []).slice().sort((a, b) => a.sign_order - b.sign_order);

  const q = search.trim().toLowerCase();
  const visible = q ? docs.filter((d) => d.title.toLowerCase().includes(q)) : docs;
  const activeInList = !!active && visible.some((d) => d.id === active.id);
  const pendingCount = docs.filter((d) => d.status === "pending").length;

  const columns = useMemo<ColumnDef<Doc>[]>(() => [
    {
      accessorKey: "title", header: "Document",
      cell: (c) => <span className="block max-w-md truncate font-medium text-fg">{c.row.original.title}</span>,
    },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="sign" status={c.row.original.status} label={STATUS_LABEL[c.row.original.status]} />,
    },
    {
      id: "signed", accessorFn: (d) => d.signed_count, header: "Signed", meta: { numeric: true },
      cell: (c) => `${c.row.original.signed_count}/${c.row.original.signer_count}`,
    },
    {
      accessorKey: "created_at", header: "Created", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => fmtShort(c.row.original.created_at),
    },
  ], []);

  function stepIndicator() {
    return (
      <nav aria-label="Signing steps" className="flex flex-wrap items-center gap-0.5">
        {STEPS.map(({ n, label, Icon }, idx) => {
          const isDone = wizardStep > n;
          const isActive = wizardStep === n;
          // Step 1 (upload) is the New document dialog; a document cannot be re-uploaded.
          // A draft's later steps open as soon as the document has what they need (signers for Fields).
          const reachable = active && active.status === "draft" ? n <= deriveStep(active) : false;
          const canClick = !isCreatingNew && n !== 1 && (isDone || isActive || reachable);
          return (
            <div key={n} className="flex items-center gap-0.5">
              {idx > 0 && <ChevronRight className={cn("mx-0.5 size-3.5", isDone || isActive ? "text-brand-ink" : "text-fg-subtle")} aria-hidden />}
              <button type="button" onClick={() => canClick && setActiveStep(n)} disabled={!canClick}
                aria-current={isActive ? "step" : undefined}
                className={cn("inline-flex min-h-11 items-center gap-1.5 rounded-lg px-3 text-xs font-medium transition-colors md:min-h-7",
                  isActive ? "bg-brand text-brand-on"
                    : isDone ? "bg-brand/10 text-brand-ink hover:bg-brand/20"
                    : "cursor-default text-fg-subtle")}>
                {isDone ? <CheckCircle2 className="size-3.5" aria-hidden /> : <Icon className="size-3.5" aria-hidden />}
                <span className="hidden sm:inline">{label}</span>
                <span className="sm:hidden">{n}</span>
              </button>
            </div>
          );
        })}
      </nav>
    );
  }

  function signerRow(s: Signer, extra?: React.ReactNode, right?: React.ReactNode) {
    return (
      <li key={s.id} className="group flex min-h-11 items-center gap-3 rounded-lg bg-surface-2 px-3 py-2">
        <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-brand/10 text-xs font-semibold text-brand-ink tabular">{s.sign_order}</span>
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium text-fg">{s.name}{extra}</p>
          <p className="truncate text-xs text-fg-muted">{s.email}</p>
        </div>
        {right}
      </li>
    );
  }

  function wizardBody() {
    if (!active) return null;
    if (detailLoading) return <div className="space-y-3"><Skeleton className="h-6 w-1/3" /><Skeleton className="h-24" /></div>;
    return (
      <>
        {/* Step 2: Signers */}
        {wizardStep === 2 && (
          <div className="max-w-2xl space-y-4">
            <div>
              <h3 className="text-sm font-semibold text-fg">Add signers</h3>
              <p className="text-[13px] text-fg-muted">Who needs to sign this document, and in what order.</p>
            </div>

            {sortedSigners.length > 0 && (
              <ul className="space-y-1.5">
                {sortedSigners.map(s => signerRow(s, null, (
                  <>
                    <span className="text-xs capitalize text-fg-muted">{s.role}</span>
                    <IconButton size="sm" icon={Trash2} label={`Remove ${s.name}`} onClick={() => removeSigner(s.id)} />
                  </>
                )))}
              </ul>
            )}

            <Card>
              <CardHeader title={<span className="inline-flex items-center gap-1.5"><UserPlus className="size-4" aria-hidden /> Add signer</span>}
                actions={<>
                  {!(active.signers ?? []).some(s => s.email.toLowerCase() === currentUserEmail) && (
                    <Button size="sm" onClick={selfSign} loading={selfSigning}><PenLine aria-hidden /> Self sign</Button>
                  )}
                  <Button size="sm" onClick={() => setShowSignerPicker(true)}><Search aria-hidden /> Search</Button>
                </>} />
              <CardBody>
                <FieldGrid cols={2}>
                  <Field label="Full name" required>
                    <Input value={signerForm.name} onChange={e => setSignerForm({ ...signerForm, name: e.target.value })} />
                  </Field>
                  <Field label="Email" required>
                    <Input type="email" value={signerForm.email} onChange={e => setSignerForm({ ...signerForm, email: e.target.value })} />
                  </Field>
                  <Field label="Signing order" hint="1 signs first.">
                    <Input type="number" min="1" numeric value={signerForm.sign_order} onChange={e => setSignerForm({ ...signerForm, sign_order: e.target.value })} />
                  </Field>
                  <Field label="Role">
                    <Select value={signerForm.role} onChange={e => setSignerForm({ ...signerForm, role: e.target.value })}
                      options={[{ value: "signer", label: "Signer" }, { value: "cc", label: "CC (observer)" }]} />
                  </Field>
                </FieldGrid>
                <Button className="mt-4" onClick={() => addSigner()} loading={addingSigner}>Add external signer</Button>
              </CardBody>
            </Card>

            <div className="flex justify-end">
              <Button variant="primary" onClick={() => setActiveStep(3)} disabled={(active.signers?.length ?? 0) === 0}>
                Next: place fields <ArrowRight aria-hidden />
              </Button>
            </div>
          </div>
        )}

        {/* Step 3: Fields */}
        {wizardStep === 3 && (
          <div className="max-w-2xl space-y-4">
            <div>
              <h3 className="text-sm font-semibold text-fg">Place signature fields</h3>
              <p className="text-[13px] text-fg-muted">Open the editor to place signature fields on the PDF.</p>
            </div>

            {fields.length > 0 ? (
              <div className="flex items-center gap-3 rounded-lg bg-tone-blue-soft p-3 text-tone-blue">
                <CheckCircle2 className="size-5 shrink-0" aria-hidden />
                <div>
                  <p className="text-sm font-semibold tabular">{fields.length} field{fields.length !== 1 ? "s" : ""} placed</p>
                  <p className="text-xs">Use Edit field placement below to adjust.</p>
                </div>
              </div>
            ) : (
              <div className="rounded-lg bg-tone-orange-soft p-3 text-tone-orange">
                <p className="text-sm font-semibold">No fields placed yet</p>
                <p className="text-xs">Open the editor to place where signers should sign.</p>
              </div>
            )}

            <div>
              <p className="mb-1.5 text-xs font-medium text-fg-muted">Signers</p>
              <div className="flex flex-wrap gap-1.5">
                {sortedSigners.map(s => <Pill key={s.id}>{s.sign_order}. {s.name}</Pill>)}
              </div>
            </div>

            <Button size="lg" className="w-full" onClick={() => setShowFieldEditor(true)}>
              <Layout aria-hidden /> {fields.length > 0 ? "Edit field placement" : "Open field placement editor"}
            </Button>

            <div className="flex items-center justify-between">
              <Button variant="ghost" onClick={() => setActiveStep(2)}><ChevronLeft aria-hidden /> Back to signers</Button>
              <Button variant="primary" onClick={() => setActiveStep(4)} disabled={fields.length === 0}>
                Next: review and invite <ArrowRight aria-hidden />
              </Button>
            </div>
          </div>
        )}

        {/* Step 4: Invite */}
        {wizardStep === 4 && (
          <div className="max-w-2xl space-y-4">
            <div>
              <h3 className="text-sm font-semibold text-fg">Review and send</h3>
              <p className="text-[13px] text-fg-muted">Check the signing request before inviting every signer.</p>
            </div>

            <Card>
              <CardBody>
                <KeyValues cols={3} items={[
                  { label: "Document", value: <span className="block truncate">{active.title}</span> },
                  { label: "Signers", value: active.signers?.length ?? 0, numeric: true },
                  { label: "Fields placed", value: fields.length, numeric: true },
                ]} />
                {active.message && (
                  <div className="mt-3 border-t border-line pt-3 text-[13px]">
                    <p className="text-fg-muted">Message</p>
                    <p className="mt-0.5 text-fg">{active.message}</p>
                  </div>
                )}
              </CardBody>
            </Card>

            <div>
              <p className="mb-1.5 text-xs font-medium text-fg-muted">Signing order</p>
              <ul className="space-y-1.5">
                {sortedSigners.map((s, idx, arr) => signerRow(s, null, (
                  <>
                    <Pill className="capitalize">{s.role}</Pill>
                    {idx < arr.length - 1 && <ArrowRight className="size-3.5 shrink-0 text-fg-subtle" aria-hidden />}
                  </>
                )))}
              </ul>
            </div>

            {fields.length === 0 && (
              <p className="rounded-lg bg-tone-orange-soft p-3 text-xs text-tone-orange">
                No signature fields placed. Signers will sign the document without specific field positions.
              </p>
            )}

            <div className="flex items-center justify-between">
              <Button variant="ghost" onClick={() => setActiveStep(3)}><ChevronLeft aria-hidden /> Back to fields</Button>
              <Button variant="primary" onClick={sendDocument} loading={sending}><Send aria-hidden /> Send for signing</Button>
            </div>
          </div>
        )}

        {/* Step 5: Track */}
        {wizardStep === 5 && (
          <div className="max-w-2xl space-y-4">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div>
                <h3 className="text-sm font-semibold text-fg">Signing progress</h3>
                <p className="text-[13px] text-fg-muted tabular">{active.signed_count} of {active.signer_count} signed</p>
              </div>
              <div className="flex flex-wrap items-center justify-end gap-2">
                {active.status === "completed" && (
                  <>
                    <Button size="sm" variant="primary" onClick={() => downloadSigned(active)}><Download aria-hidden /> Signed PDF</Button>
                    <Button size="sm" onClick={() => downloadCertificate(active)}><Download aria-hidden /> Certificate</Button>
                    <IconButton size="sm" variant="secondary" icon={RefreshCw} label="Regenerate signed PDF and certificate" onClick={() => finalizeDoc(active)} />
                    <Button size="sm" variant="ghost" asChild>
                      <a href={`/verify/${active.id}`} target="_blank" rel="noopener noreferrer" aria-label="Open verification page">
                        <ExternalLink aria-hidden />
                      </a>
                    </Button>
                  </>
                )}
                {["draft", "pending"].includes(active.status) && (
                  <Button size="sm" variant="danger" onClick={() => cancelDocument(active.id)}>Cancel</Button>
                )}
              </div>
            </div>

            {active.status === "completed" && (
              <div className="flex items-center gap-3 rounded-lg bg-tone-blue-soft p-3 text-tone-blue">
                <CheckCircle2 className="size-5 shrink-0" aria-hidden />
                <div className="min-w-0">
                  <p className="text-sm font-semibold">All parties have signed</p>
                  {active.completed_at && <p className="text-xs">Completed {new Date(active.completed_at).toLocaleDateString("en-GB")}</p>}
                  {active.sealed_at ? (
                    <p className="mt-1 flex flex-wrap items-center gap-1 text-xs">
                      <ShieldCheck className="size-3.5" aria-hidden />
                      {SIG_LEVEL_LABEL[active.signature_level || "ses"]}, {SEAL_LEVEL_LABEL[active.seal_level || "pades-b-b"] || "PAdES"}
                      {active.seal_fingerprint && <span className="truncate font-mono opacity-80">{active.seal_fingerprint.slice(0, 16)}...</span>}
                    </p>
                  ) : (
                    <p className="mt-1 text-xs text-tone-orange">Not sealed yet. Use regenerate to finalize.</p>
                  )}
                </div>
              </div>
            )}

            {active.status === "declined" && (
              <div className="flex items-center gap-3 rounded-lg bg-tone-red-soft p-3 text-tone-red" role="alert">
                <X className="size-5 shrink-0" aria-hidden />
                <p className="text-sm font-semibold">A signer has declined this document</p>
              </div>
            )}

            <ul className="space-y-1.5">
              {sortedSigners.map(s => {
                const isMe = s.email.toLowerCase() === currentUserEmail;
                return signerRow(s,
                  isMe ? <Pill tone="red" className="ml-1.5 h-[18px] px-1.5 text-[10px]">You</Pill> : null,
                  <>
                    <span className="flex shrink-0 items-center gap-1.5 text-xs text-fg-muted">
                      <StatusPill family="sign" status={s.status} />
                      {s.signed_at && <span className="tabular">{fmtShort(s.signed_at)}</span>}
                    </span>
                    {isMe && s.status !== "signed" && s.token && (
                      <Button size="sm" variant="primary" asChild>
                        <a href={`/sign/${s.token}`} target="_blank" rel="noopener noreferrer">Sign now <ArrowRight aria-hidden /></a>
                      </Button>
                    )}
                    {active.status === "pending" && ["sent", "viewed"].includes(s.status) && !isMe && (
                      <Button size="sm" variant="ghost" onClick={() => remind(s.id)} loading={remindingId === s.id}>Remind</Button>
                    )}
                  </>,
                );
              })}
            </ul>

            {active.document_hash && (
              <div className="rounded-lg bg-surface-2 p-3">
                <p className="mb-1 text-xs font-medium text-fg-muted">SHA-256 document hash</p>
                <p className="break-all font-mono text-xs text-fg">{active.document_hash}</p>
                <p className="mt-1.5 text-xs text-fg-muted">Run <code className="rounded bg-surface px-1 font-mono">sha256sum &lt;file.pdf&gt;</code> to verify integrity.</p>
              </div>
            )}
          </div>
        )}
      </>
    );
  }

  function wizard() {
    return (
      <Card>
        <CardHeader>{stepIndicator()}</CardHeader>
        <CardBody>{wizardBody()}</CardBody>
      </Card>
    );
  }

  return (
    <div className="space-y-4">
      {/* A document opened from outside the current view (just created, or filtered out) */}
      {showWizard && active && !activeInList && !isCreatingNew && (
        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <h2 className="truncate text-sm font-semibold text-fg">{active.title}</h2>
            <StatusPill family="sign" status={active.status} label={STATUS_LABEL[active.status]} />
            <IconButton size="sm" icon={X} label="Close document" className="ml-auto" onClick={() => setActive(null)} />
          </div>
          {wizard()}
        </div>
      )}

      <ListFrame>
        <PageHeader flush title="Document signing"
          summary={`${visible.length} document${visible.length !== 1 ? "s" : ""}${pendingCount ? `, ${pendingCount} waiting for signatures` : ""}. Legally binding e-signatures.`}
          actions={<>
            <Button onClick={downloadAll} loading={exporting} title="Download all completed documents (ZIP)">
              {!exporting && <Download aria-hidden />} Export completed
            </Button>
            <Button variant="primary" onClick={startNew}><Plus aria-hidden /> New document</Button>
          </>} />
        <SavedViewTabs state={views} currentFilters={statusFilter ? { status: statusFilter } : {}} onSelect={() => setChipStatus("")} />
        <FilterChips chips={[{ id: "cancelled", label: "Cancelled", active: chipStatus === "cancelled" }]}
          onToggle={(id) => setChipStatus((cur) => (cur === id ? "" : id))} onClear={() => setChipStatus("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search documents" aria-label="Search documents" className="w-56" />
        </FilterChips>
        <DataTable label="Signing documents" columns={columns} data={visible} getRowId={(d) => d.id} loading={loading}
          maxHeight="none"
          expandedId={!isCreatingNew && activeInList ? active!.id : null}
          onExpandedChange={(id) => {
            const doc = id ? visible.find((d) => d.id === id) : null;
            if (doc) openDetail(doc); else setActive(null);
          }}
          empty={{
            icon: PenLine, title: "No documents in this view",
            description: "Upload a PDF to collect legally binding signatures.",
            action: <Button variant="primary" onClick={startNew}><Plus aria-hidden /> New document</Button>,
          }}
          renderDetails={() => wizard()} />
      </ListFrame>

      {/* Step 1: Upload */}
      <Dialog open={isCreatingNew} onOpenChange={(o) => { if (!o) setIsCreatingNew(false); }} dismissable={!creating}
        title="New signing document" description="Give the document a title and upload the PDF to begin."
        footer={<>
          <Button onClick={() => setIsCreatingNew(false)} disabled={creating}>Cancel</Button>
          <Button variant="primary" onClick={createDocument} loading={creating} disabled={!createTitle.trim() || !createFile}>
            Upload and continue {!creating && <ArrowRight aria-hidden />}
          </Button>
        </>}>
        <div className="space-y-4">
          <Field label="Document title" required>
            <Input value={createTitle} onChange={e => setCreateTitle(e.target.value)} placeholder="e.g. Service agreement, Acme Corp" />
          </Field>
          <Field label="Message to signers" hint="Optional. Shown to each signer.">
            <Textarea value={createMessage} onChange={e => setCreateMessage(e.target.value)} rows={3} className="resize-none" />
          </Field>
          <Field label="PDF document" required error={uploadError || undefined}>
            <label
              onDragOver={e => { e.preventDefault(); setDragActive(true); }}
              onDragEnter={e => { e.preventDefault(); setDragActive(true); }}
              onDragLeave={e => { e.preventDefault(); setDragActive(false); }}
              onDrop={e => {
                e.preventDefault();
                setDragActive(false);
                acceptFile(e.dataTransfer.files?.[0] ?? null);
              }}
              className={cn("block cursor-pointer rounded-lg border-2 border-dashed p-6 text-center transition-colors",
                dragActive ? "border-brand bg-brand/5" : "border-line-strong hover:border-brand/40")}>
              {createFile ? (
                <div>
                  <FileText className="mx-auto mb-1.5 size-7 text-brand-ink" aria-hidden />
                  <p className="text-sm font-medium text-fg">{createFile.name}</p>
                  <p className="mt-0.5 text-xs text-fg-muted tabular">{(createFile.size / 1024 / 1024).toFixed(2)} MB, click or drop to replace</p>
                </div>
              ) : (
                <div>
                  <FileText className="mx-auto mb-2 size-8 text-fg-subtle" aria-hidden />
                  <p className="text-sm text-fg-muted">{dragActive ? "Drop the PDF here" : "Drag and drop a PDF here, or click to browse"}</p>
                </div>
              )}
              <input type="file" accept="application/pdf,.pdf" className="sr-only"
                onChange={e => acceptFile(e.target.files?.[0] ?? null)} />
            </label>
          </Field>
        </div>
      </Dialog>

      <SignerPickerModal
        isOpen={showSignerPicker}
        onClose={() => setShowSignerPicker(false)}
        onSelect={(result) => {
          setSignerForm(prev => ({ ...prev, name: result.name, email: result.email }));
          addSigner({ name: result.name, email: result.email, signer_type: result.signer_type, partner_account_id: result.partner_account_id });
        }}
      />

      {/* PDF field editor (full screen) */}
      {showFieldEditor && active && (
        <div className="fixed inset-0 z-50 flex flex-col bg-canvas" role="dialog" aria-modal="true" aria-label={`Place fields on ${active.title}`}>
          <div className="flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-line bg-surface px-5 py-3">
            <div className="min-w-0">
              <h2 className="truncate text-sm font-semibold text-fg">{active.title}</h2>
              <p className="text-xs text-fg-muted">Click on the PDF to place a field. Drag to move, resize from the corner.</p>
            </div>
            <div className="flex items-center gap-2">
              <span className="text-xs text-fg-muted tabular">{fields.length} field{fields.length !== 1 ? "s" : ""} placed</span>
              <Button size="sm" onClick={() => setShowFieldEditor(false)}>Cancel</Button>
              <Button size="sm" variant="primary" onClick={saveFields} loading={savingFields} disabled={fields.length === 0}>
                {`Save ${fields.length} field${fields.length !== 1 ? "s" : ""}`}
              </Button>
            </div>
          </div>
          <div className="flex-1 overflow-hidden">
            <PDFFieldEditor
              pdfUrl={active.file_url ?? ""}
              authToken={authToken}
              signers={(active.signers ?? []).map(s => ({ id: s.id, name: s.name, email: s.email, sign_order: s.sign_order }))}
              fields={fields}
              onFieldsChange={setFields}
            />
          </div>
        </div>
      )}
    </div>
  );
}
