"use client";
import { fetchAllPages } from "@/lib/fetchAll";
import { useDropToInput, isImage } from "@/hooks/useFileDrop";
import { GRANARY_DEPARTMENTS } from "@/lib/departments";
import { useEffect, useState, useCallback, useRef, useMemo } from "react";
import { hrApi } from "@/lib/api";
import EmployeeAllowancesModal from "@/components/hr/EmployeeAllowancesModal";
import { usePermissions } from "@/hooks/usePermissions";
import { useCurrencySettings } from "@/hooks/useCurrencySettings";
import { employeeSource } from "@/lib/entitySources";
import { UserCheck, Plus, Search, ScanFace, Trash2, Upload, X, ImagePlus, AlertCircle, Pencil, Wallet } from "lucide-react";
import {
  Button, Combobox, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame, PageHeader, Pill,
  SavedViewTabs, Select, Skeleton, Stat, StatGroup, StatusPill, Switch, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";
import { cn } from "@/lib/utils";

interface Employee {
  id: string;
  employee_id: string;
  full_name: string;
  first_name: string;
  last_name?: string;
  email: string;
  phone?: string;
  department?: string;
  job_title?: string;
  employment_type: string;
  status: string;
  start_date?: string;
  user_id?: string;
  reports_to_id?: string | null;
}

interface JobTitleInfo {
  title: string;
  default_roles: string[];
  default_permissions: string[];
}

interface FaceEnrollment {
  enrolled: boolean;
  face_uid?: string;
  image_count?: number;
  enrolled_at?: string;
}

interface FaceModalState {
  employeeId: string;
  employeeName: string;
  enrollment: FaceEnrollment | null;
  loading: boolean;
}

const EMP_TYPE_LABELS: Record<string, string> = {
  full_time: "Full-time",
  part_time: "Part-time",
  contract: "Contract",
  intern: "Intern",
};

const STATUS_LABELS: Record<string, string> = { active: "Active", on_leave: "On leave", terminated: "Terminated", probation: "Probation" };

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "active", label: "Active", filters: { status: "active" } },
  { id: "probation", label: "Probation", filters: { status: "probation" } },
  { id: "on_leave", label: "On leave", filters: { status: "on_leave" } },
];

function EmployeeAvatar({ name }: { name: string }) {
  const initials = name.split(" ").map((w) => w[0]).slice(0, 2).join("").toUpperCase();
  return (
    <div aria-hidden className="flex size-8 shrink-0 items-center justify-center rounded-full bg-surface-2 text-xs font-semibold text-fg-muted">
      {initials}
    </div>
  );
}

function ErrorNote({ message }: { message: string }) {
  return (
    <div role="alert" className="mt-4 flex items-start gap-2 rounded-lg border border-tone-red/30 bg-tone-red-soft px-3 py-2.5">
      <AlertCircle className="mt-0.5 size-4 shrink-0 text-tone-red" aria-hidden />
      <p className="text-xs text-tone-red">{message}</p>
    </div>
  );
}

export default function EmployeesPage() {
  const [employees, setEmployees] = useState<Employee[]>([]);
  const [jobTitles, setJobTitles] = useState<JobTitleInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [deptFilter, setDeptFilter] = useState("");
  const views = useSavedViews<Filters>("hr.employees", VIEWS);

  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState({
    full_name: "", email: "", job_title: "", department: "",
    employment_type: "full_time", start_date: new Date().toISOString().slice(0, 10),
    create_user: false, temp_password: "", reports_to_id: "", base_salary: "", salary_currency: "",
  });
  const [saving, setSaving] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  const [editModal, setEditModal] = useState<Employee | null>(null);
  const [editForm, setEditForm] = useState({ first_name: "", last_name: "", department: "", job_title: "", phone: "", reports_to_id: "", base_salary: "", salary_currency: "" });
  // The salary as loaded, so Save sends it only when it was changed.
  const [editSalaryLoaded, setEditSalaryLoaded] = useState({ base_salary: "", salary_currency: "" });
  const [editSaving, setEditSaving] = useState(false);
  const [editError, setEditError] = useState<string | null>(null);

  const [faceModal, setFaceModal] = useState<FaceModalState | null>(null);
  const [allowanceFor, setAllowanceFor] = useState<Employee | null>(null);
  const { can, holds } = usePermissions();
  // Payroll pays base_salary: setting it takes hr:manage_compensation, seeing it hr:view_compensation (the API's rules).
  const canSetSalary = holds("hr:manage_compensation");
  const canSeeSalary = holds("hr:view_compensation");
  const currencies = useCurrencySettings();
  const [faceFiles, setFaceFiles] = useState<File[]>([]);
  const [facePreviews, setFacePreviews] = useState<string[]>([]);
  const [faceUploading, setFaceUploading] = useState(false);
  const [faceDeleting, setFaceDeleting] = useState(false);
  const [faceError, setFaceError] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const { dragActive: faceDrag, dropHandlers: faceDrop } = useDropToInput(fileInputRef, { accept: isImage });

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [empRes, jtRes] = await Promise.all([
        fetchAllPages<Employee>(hrApi.listEmployees),
        hrApi.listJobTitles(),
      ]);
      setEmployees(empRes.rows);
      setJobTitles(jtRes.data ?? []);
    } catch { setEmployees([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  async function deleteEmployee(id: string) {
    try {
      await hrApi.deleteEmployee(id);
      setConfirmDeleteId(null);
      setEmployees((prev) => prev.filter((e) => e.id !== id));
    } catch { /* keep */ }
  }

  const departments = [...new Set(employees.map((e) => e.department).filter(Boolean) as string[])];
  const statusFilter = views.active?.filters.status ?? "";
  const filtered = employees.filter((e) => {
    const matchSearch = !search ||
      (e.full_name ?? "").toLowerCase().includes(search.toLowerCase()) ||
      (e.job_title ?? "").toLowerCase().includes(search.toLowerCase()) ||
      (e.department ?? "").toLowerCase().includes(search.toLowerCase());
    const matchDept = !deptFilter || e.department === deptFilter;
    const matchStatus = !statusFilter || e.status === statusFilter;
    return matchSearch && matchDept && matchStatus;
  });
  const active = employees.filter((e) => e.status === "active").length;

  async function createEmployee() {
    if (!form.email || !form.full_name) return;
    if (form.create_user && !form.temp_password) {
      setCreateError("Temporary password is required when creating a user account"); return;
    }
    setSaving(true); setCreateError(null);
    try {
      await hrApi.createEmployee({
        full_name: form.full_name,
        email: form.email,
        job_title: form.job_title || undefined,
        department: form.department || undefined,
        employment_type: form.employment_type,
        start_date: form.start_date,
        create_user: form.create_user,
        temp_password: form.create_user ? form.temp_password : undefined,
        reports_to_id: form.reports_to_id || undefined,
        ...(canSetSalary && form.base_salary.trim() ? {
          base_salary: form.base_salary.trim(),
          salary_currency: form.salary_currency || currencies.functional,
        } : {}),
      });
      setShowModal(false);
      setForm({ full_name: "", email: "", job_title: "", department: "", employment_type: "full_time", start_date: new Date().toISOString().slice(0, 10), create_user: false, temp_password: "", reports_to_id: "", base_salary: "", salary_currency: "" });
      await load();
    } catch (err: unknown) {
      setCreateError((err as { response?: { data?: { detail?: string } } })?.response?.data?.detail ?? "Failed to create employee");
    } finally { setSaving(false); }
  }

  function openEditModal(emp: Employee) {
    setEditModal(emp);
    setEditForm({
      first_name: emp.first_name ?? "",
      last_name: emp.last_name ?? "",
      department: emp.department ?? "",
      job_title: emp.job_title ?? "",
      phone: emp.phone ?? "",
      reports_to_id: emp.reports_to_id ?? "",
      base_salary: "",
      salary_currency: "",
    });
    setEditSalaryLoaded({ base_salary: "", salary_currency: "" });
    setEditError(null);
    // The current salary, for those who may see it (the list never carries it).
    if (canSetSalary && canSeeSalary) {
      hrApi.getEmployee(emp.id).then((res) => {
        const d = res.data as { base_salary?: number | null; salary_currency?: string | null };
        const loaded = {
          base_salary: d.base_salary != null ? String(d.base_salary) : "",
          salary_currency: d.salary_currency ?? "",
        };
        setEditSalaryLoaded(loaded);
        setEditForm((f) => ({ ...f, ...loaded }));
      }).catch(() => { /* the salary fields stay empty */ });
    }
  }

  async function saveEdit() {
    if (!editModal) return;
    setEditSaving(true); setEditError(null);
    try {
      await hrApi.updateEmployee(editModal.id, {
        first_name: editForm.first_name || undefined,
        last_name: editForm.last_name || undefined,
        department: editForm.department || undefined,
        job_title: editForm.job_title || undefined,
        phone: editForm.phone || undefined,
        reports_to_id: editForm.reports_to_id,
        ...(canSetSalary && editForm.base_salary.trim() && editForm.base_salary.trim() !== editSalaryLoaded.base_salary
          ? { base_salary: editForm.base_salary.trim() } : {}),
        ...(canSetSalary && editForm.salary_currency && editForm.salary_currency !== editSalaryLoaded.salary_currency
          ? { salary_currency: editForm.salary_currency } : {}),
      });
      setEditModal(null);
      await load();
    } catch (err: unknown) {
      setEditError((err as { response?: { data?: { detail?: string } } })?.response?.data?.detail ?? "Update failed");
    } finally { setEditSaving(false); }
  }

  async function openFaceModal(emp: Employee) {
    setFaceModal({ employeeId: emp.id, employeeName: emp.full_name, enrollment: null, loading: true });
    setFaceFiles([]); setFacePreviews([]); setFaceError(null);
    try {
      const res = await hrApi.getFaceEnrollment(emp.id);
      setFaceModal((m) => m ? { ...m, enrollment: res.data, loading: false } : null);
    } catch {
      setFaceModal((m) => m ? { ...m, enrollment: { enrolled: false }, loading: false } : null);
    }
  }

  function closeFaceModal() {
    facePreviews.forEach((url) => URL.revokeObjectURL(url));
    setFaceModal(null); setFaceFiles([]); setFacePreviews([]); setFaceError(null);
  }

  function handleFaceFileChange(e: React.ChangeEvent<HTMLInputElement>) {
    const files = Array.from(e.target.files ?? []);
    const allowed = files.slice(0, 4 - faceFiles.length);
    const previews = allowed.map((f) => URL.createObjectURL(f));
    setFaceFiles((prev) => [...prev, ...allowed]);
    setFacePreviews((prev) => [...prev, ...previews]);
    e.target.value = "";
  }

  function removeFacePreview(idx: number) {
    URL.revokeObjectURL(facePreviews[idx]);
    setFaceFiles((prev) => prev.filter((_, i) => i !== idx));
    setFacePreviews((prev) => prev.filter((_, i) => i !== idx));
  }

  async function submitFaceEnrollment() {
    if (!faceModal || faceFiles.length === 0) return;
    setFaceUploading(true); setFaceError(null);
    try {
      const fd = new FormData();
      faceFiles.forEach((f, i) => {
        fd.append(i === 0 ? "face_image" : `additional_face_image_${i - 1}`, f);
      });
      await hrApi.enrollFace(faceModal.employeeId, fd);
      const res = await hrApi.getFaceEnrollment(faceModal.employeeId);
      setFaceModal((m) => m ? { ...m, enrollment: res.data } : null);
      facePreviews.forEach((url) => URL.revokeObjectURL(url));
      setFaceFiles([]); setFacePreviews([]);
    } catch (err: unknown) {
      setFaceError((err as { response?: { data?: { detail?: string } } })?.response?.data?.detail ?? "Enrollment failed.");
    } finally { setFaceUploading(false); }
  }

  async function deleteEnrollment() {
    if (!faceModal) return;
    setFaceDeleting(true); setFaceError(null);
    try {
      await hrApi.deleteFaceEnrollment(faceModal.employeeId);
      setFaceModal((m) => m ? { ...m, enrollment: { enrolled: false } } : null);
    } catch (err: unknown) {
      setFaceError((err as { response?: { data?: { detail?: string } } })?.response?.data?.detail ?? "Delete failed.");
    } finally { setFaceDeleting(false); }
  }

  const jobTitleOptions = jobTitles.map((jt) => ({ value: jt.title, label: jt.title }));
  const deptOptions = GRANARY_DEPARTMENTS.map((d) => ({ value: d, label: d }));

  const columns = useMemo<ColumnDef<Employee>[]>(() => [
    {
      id: "name", accessorFn: (e) => e.full_name || e.email, header: "Employee",
      cell: (c) => (
        <div className="flex min-w-0 items-center gap-2.5">
          <EmployeeAvatar name={c.getValue<string>()} />
          <div className="min-w-0">
            <p className="max-w-56 truncate font-medium text-fg">{c.getValue<string>()}</p>
            <p className="max-w-56 truncate text-xs text-fg-muted">{c.row.original.job_title || c.row.original.email}</p>
          </div>
        </div>
      ),
    },
    { accessorKey: "department", header: "Department", cell: (c) => (c.row.original.department ? <Pill>{c.row.original.department}</Pill> : "-") },
    {
      accessorKey: "employment_type", header: "Type", meta: { hideOnMobile: true },
      cell: (c) => EMP_TYPE_LABELS[c.row.original.employment_type] ?? c.row.original.employment_type,
    },
    {
      accessorKey: "start_date", header: "Started", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => (c.row.original.start_date
        ? new Date(c.row.original.start_date).toLocaleDateString("en-GB", { month: "short", year: "numeric" }) : "-"),
    },
    {
      accessorKey: "employee_id", header: "ID", meta: { hideOnMobile: true },
      cell: (c) => <span className="font-mono text-xs text-fg-muted">{c.row.original.employee_id}</span>,
    },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="employee" status={c.row.original.status} label={STATUS_LABELS[c.row.original.status]} />,
    },
  ], []);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Active" value={active} />
        <Stat label="Departments" value={departments.length} />
        <Stat label="Total" value={employees.length} />
        <Stat label="On leave" value={employees.filter(e => e.status === "on_leave").length} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Employees" summary="Team members and employment records"
          actions={<Button variant="primary" onClick={() => { setShowModal(true); setCreateError(null); }}><Plus aria-hidden /> Add employee</Button>} />
        <SavedViewTabs state={views} currentFilters={views.active?.filters} />
        <FilterChips
          chips={departments.map((d) => ({ id: d, label: d.charAt(0).toUpperCase() + d.slice(1), active: deptFilter === d }))}
          onToggle={(id) => setDeptFilter(deptFilter === id ? "" : id)} onClear={() => setDeptFilter("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search employees" aria-label="Search employees" className="w-56" />
        </FilterChips>
        <DataTable label="Employees" columns={columns} data={filtered} getRowId={(e) => e.id} loading={loading}
          empty={{
            icon: UserCheck, title: "No employees here", description: "Add your team members to get started.",
            action: <Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Add employee</Button>,
          }}
          renderDetails={(emp) => (
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                { label: "Email", value: emp.email || "-" },
                { label: "Phone", value: emp.phone || "-" },
                { label: "Job title", value: emp.job_title || "-" },
                { label: "Employment", value: EMP_TYPE_LABELS[emp.employment_type] ?? emp.employment_type },
                { label: "Start date", value: emp.start_date ? new Date(emp.start_date).toLocaleDateString("en-GB") : "-" },
                { label: "Employee ID", value: <span className="font-mono">{emp.employee_id}</span> },
              ]} />
              <div className="flex flex-wrap items-center gap-2">
                <Button size="sm" onClick={() => openEditModal(emp)}><Pencil aria-hidden /> Edit</Button>
                {can("hr:view_compensation") && (
                  <Button size="sm" onClick={() => setAllowanceFor(emp)}><Wallet aria-hidden /> Allowances</Button>
                )}
                <Button size="sm" onClick={() => openFaceModal(emp)}><ScanFace aria-hidden /> Face ID</Button>
                {confirmDeleteId === emp.id ? (
                  <>
                    <span className="text-xs font-semibold text-tone-red">Delete?</span>
                    <Button size="sm" variant="danger" onClick={() => deleteEmployee(emp.id)}>Yes, delete</Button>
                    <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
                  </>
                ) : (
                  <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${emp.full_name || emp.email}`} onClick={() => setConfirmDeleteId(emp.id)} />
                )}
              </div>
            </div>
          )} />
      </ListFrame>

      <Dialog open={!!editModal} onOpenChange={(o) => { if (!o) setEditModal(null); }} title="Edit employee" description={editModal?.email}
        footer={<>
          <Button onClick={() => setEditModal(null)}>Cancel</Button>
          <Button variant="primary" onClick={saveEdit} loading={editSaving}>{editSaving ? "Saving..." : "Save changes"}</Button>
        </>}>
        {editModal && (
          <>
            <FieldGrid cols={2}>
              <Field label="First name"><Input value={editForm.first_name} onChange={(e) => setEditForm({ ...editForm, first_name: e.target.value })} /></Field>
              <Field label="Last name"><Input value={editForm.last_name} onChange={(e) => setEditForm({ ...editForm, last_name: e.target.value })} /></Field>
              <Field label="Department">
                <Select value={editForm.department} onChange={(e) => setEditForm({ ...editForm, department: e.target.value })} placeholder="Select a department" options={deptOptions} />
              </Field>
              <Field label="Job title">
                <Select value={editForm.job_title} onChange={(e) => setEditForm({ ...editForm, job_title: e.target.value })} placeholder="Select a title" options={jobTitleOptions} />
              </Field>
              <Field label="Reports to">
                <Combobox value={editForm.reports_to_id} source={employeeSource}
                  placeholder="None" emptyLabel="No employees found"
                  excludeIds={[editModal.id]}
                  onChange={(id) => setEditForm({ ...editForm, reports_to_id: id })} />
              </Field>
              <Field label="Phone"><Input value={editForm.phone} onChange={(e) => setEditForm({ ...editForm, phone: e.target.value })} placeholder="+1 ..." /></Field>
              {canSetSalary && (
                <>
                  <Field label="Base salary" hint={canSeeSalary ? "Monthly, as payroll pays it." : "Monthly, as payroll pays it. Leave empty to keep the current salary."}>
                    <Input type="number" inputMode="decimal" min={0} step="any" value={editForm.base_salary}
                      onChange={(e) => setEditForm({ ...editForm, base_salary: e.target.value })} placeholder="0" />
                  </Field>
                  <Field label="Salary currency">
                    <Select value={editForm.salary_currency || currencies.functional}
                      onChange={(e) => setEditForm({ ...editForm, salary_currency: e.target.value })}
                      options={currencies.active.map((c) => ({ value: c, label: c }))} />
                  </Field>
                </>
              )}
            </FieldGrid>
            {editModal.user_id && (
              <p className="mt-4 rounded-lg bg-tone-blue-soft px-3 py-2 text-xs text-tone-blue">
                Changes to name, department and job title also sync to the linked user account.
              </p>
            )}
            {editError && <ErrorNote message={editError} />}
          </>
        )}
      </Dialog>

      {allowanceFor && (
        <EmployeeAllowancesModal employeeId={allowanceFor.id} employeeName={allowanceFor.full_name}
          canEdit={can("hr:manage_compensation")} onClose={() => setAllowanceFor(null)} />
      )}

      <Dialog open={!!faceModal} onOpenChange={(o) => { if (!o) closeFaceModal(); }} title="Face enrollment" description={faceModal?.employeeName}
        footer={<>
          <Button onClick={closeFaceModal}>Close</Button>
          {faceFiles.length > 0 && (
            <Button variant="primary" onClick={submitFaceEnrollment} loading={faceUploading}>
              {!faceUploading && <Upload aria-hidden />}
              {faceUploading ? "Enrolling..." : `Enroll ${faceFiles.length} photo${faceFiles.length !== 1 ? "s" : ""}`}
            </Button>
          )}
        </>}>
        {faceModal && (faceModal.loading ? (
          <div className="space-y-2 py-4" aria-label="Loading enrollment status"><Skeleton className="h-14 w-full" /><Skeleton className="h-24 w-full" /></div>
        ) : (
          <>
            <div className={cn("mb-5 flex items-center gap-3 rounded-lg border p-3",
              faceModal.enrollment?.enrolled ? "border-tone-blue/30 bg-tone-blue-soft" : "border-line bg-surface-2")}>
              <div className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-surface">
                <ScanFace className={cn("size-5", faceModal.enrollment?.enrolled ? "text-tone-blue" : "text-fg-subtle")} aria-hidden />
              </div>
              <div className="min-w-0 flex-1">
                {faceModal.enrollment?.enrolled ? (
                  <>
                    <p className="text-sm font-semibold text-tone-blue">Face enrolled</p>
                    <p className="mt-0.5 text-xs text-fg-muted">
                      {faceModal.enrollment.image_count ?? 1} photo{(faceModal.enrollment.image_count ?? 1) !== 1 ? "s" : ""} registered
                      {faceModal.enrollment.enrolled_at && `, ${new Date(faceModal.enrollment.enrolled_at).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" })}`}
                    </p>
                  </>
                ) : (
                  <>
                    <p className="text-sm font-semibold text-fg">Not enrolled</p>
                    <p className="mt-0.5 text-xs text-fg-muted">Upload 1 to 4 clear face photos to enable face clock-in</p>
                  </>
                )}
              </div>
              {faceModal.enrollment?.enrolled && (
                <IconButton variant="danger" icon={Trash2} label="Remove face enrollment" onClick={deleteEnrollment} disabled={faceDeleting} />
              )}
            </div>

            <div className="mb-2">
              <p className="text-sm font-medium text-fg">
                {faceModal.enrollment?.enrolled ? "Re-enroll with new photos" : "Upload face photos"}
                <span className="ml-1 font-normal text-fg-muted">(1 to 4 clear photos of this person alone)</span>
              </p>
              <p className="mb-2 mt-1 text-xs text-fg-muted">
                Photos are turned into face data on our own server and then discarded; they are not stored.
                Re-enrolling replaces the previous face data.
              </p>
              {facePreviews.length > 0 && (
                <div className="mb-3 grid grid-cols-4 gap-2">
                  {facePreviews.map((url, i) => (
                    <div key={i} className="group relative aspect-square overflow-hidden rounded-lg bg-surface-2">
                      {/* eslint-disable-next-line @next/next/no-img-element */}
                      <img src={url} alt={`face ${i + 1}`} className="h-full w-full object-cover" />
                      <button type="button" onClick={() => removeFacePreview(i)} aria-label={`Remove photo ${i + 1}`}
                        className="absolute right-1 top-1 flex size-8 items-center justify-center rounded-full bg-black/60 text-white transition-opacity md:size-6 [@media(hover:hover)]:opacity-0 [@media(hover:hover)]:group-hover:opacity-100 [@media(hover:hover)]:focus-within:opacity-100 [@media(hover:hover)]:focus-visible:opacity-100">
                        <X className="size-3" aria-hidden />
                      </button>
                    </div>
                  ))}
                </div>
              )}
              {faceFiles.length < 4 && (
                <>
                  <input ref={fileInputRef} type="file" accept="image/*" multiple className="hidden" onChange={handleFaceFileChange} />
                  <button type="button" onClick={() => fileInputRef.current?.click()} {...faceDrop}
                    className={cn("flex w-full flex-col items-center gap-2 rounded-lg border-2 border-dashed py-6 transition-colors",
                      faceDrag ? "border-brand bg-brand/5 text-brand-ink" : "border-line-strong text-fg-muted hover:border-brand/50 hover:bg-surface-2 hover:text-fg")}>
                    <ImagePlus className="size-6" aria-hidden />
                    <span className="text-xs font-medium">{faceDrag ? "Drop photos here" : `Drag and drop or click to select photos (${faceFiles.length}/4)`}</span>
                    <span className="text-xs text-fg-subtle">JPG or PNG, clear face, good lighting</span>
                  </button>
                </>
              )}
            </div>
            {faceError && <ErrorNote message={faceError} />}
          </>
        ))}
      </Dialog>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Add employee"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={createEmployee} disabled={!form.email || !form.full_name} loading={saving}>
            {saving ? "Adding..." : "Add employee"}
          </Button>
        </>}>
        <div className="space-y-4">
          <FieldGrid cols={2}>
            <Field label="Full name" required className="md:col-span-2">
              <Input value={form.full_name} onChange={(e) => setForm({ ...form, full_name: e.target.value })} placeholder="e.g. Robert Smith" />
            </Field>
            <Field label="Email" required className="md:col-span-2">
              <Input type="email" value={form.email} onChange={(e) => setForm({ ...form, email: e.target.value })} placeholder="robert.smith@example.com" />
            </Field>
            <Field label="Job title">
              <Select value={form.job_title} onChange={(e) => setForm({ ...form, job_title: e.target.value })} placeholder="Select a title" options={jobTitleOptions} />
            </Field>
            <Field label="Department">
              <Select value={form.department} onChange={(e) => setForm({ ...form, department: e.target.value })} placeholder="Select a department" options={deptOptions} />
            </Field>
            <Field label="Reports to" className="md:col-span-2">
              <Combobox value={form.reports_to_id} source={employeeSource}
                placeholder="None" emptyLabel="No employees found"
                onChange={(id) => setForm({ ...form, reports_to_id: id })} />
            </Field>
            <Field label="Employment type">
              <Select value={form.employment_type} onChange={(e) => setForm({ ...form, employment_type: e.target.value })}
                options={Object.entries(EMP_TYPE_LABELS).map(([v, l]) => ({ value: v, label: l }))} />
            </Field>
            <Field label="Start date">
              <Input type="date" value={form.start_date} onChange={(e) => setForm({ ...form, start_date: e.target.value })} />
            </Field>
            {canSetSalary && (
              <>
                <Field label="Base salary" hint="Monthly, as payroll pays it.">
                  <Input type="number" inputMode="decimal" min={0} step="any" value={form.base_salary}
                    onChange={(e) => setForm({ ...form, base_salary: e.target.value })} placeholder="0" />
                </Field>
                <Field label="Salary currency">
                  <Select value={form.salary_currency || currencies.functional}
                    onChange={(e) => setForm({ ...form, salary_currency: e.target.value })}
                    options={currencies.active.map((c) => ({ value: c, label: c }))} />
                </Field>
              </>
            )}
          </FieldGrid>

          <div className="rounded-lg border border-line bg-surface-2 p-3">
            <Switch label="Also create a system user account" checked={form.create_user}
              onCheckedChange={(v) => setForm({ ...form, create_user: v })} />
            <p className="mt-1 text-xs text-fg-muted">The employee can sign in to the ERP with a temporary password.</p>
            {form.create_user && (
              <div className="mt-3 space-y-3">
                <Field label="Temporary password" required hint="They are asked to change it on first sign-in.">
                  <Input type="text" autoComplete="off" value={form.temp_password}
                    onChange={(e) => setForm({ ...form, temp_password: e.target.value })} placeholder="e.g. Welcome123!" />
                </Field>
                {(() => {
                  const jt = jobTitles.find((j) => j.title === form.job_title);
                  if (!jt || jt.default_roles.length === 0) return null;
                  return (
                    <div className="rounded-lg border border-tone-blue/30 bg-tone-blue-soft px-3 py-2.5">
                      <p className="mb-1.5 text-xs font-semibold text-tone-blue">Role bundle assigned automatically</p>
                      <div className="flex flex-wrap gap-1.5">
                        {jt.default_roles.map((r) => <Pill key={r} tone="blue">{r}</Pill>)}
                      </div>
                      <p className="mt-1.5 text-xs text-tone-blue">{jt.default_permissions.length} permissions across all modules</p>
                    </div>
                  );
                })()}
              </div>
            )}
          </div>
        </div>
        {createError && <ErrorNote message={createError} />}
      </Dialog>
    </div>
  );
}
