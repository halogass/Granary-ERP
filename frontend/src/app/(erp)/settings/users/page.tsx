"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import Link from "next/link";
import { useEffect, useState, useCallback, useMemo } from "react";
import { Plus, Edit3, Key, Search, ShieldOff, UserCheck, Users, Eye, EyeOff, ShieldCheck } from "lucide-react";
import { GRANARY_DEPARTMENTS } from "@/lib/departments";
import { usersApi, hrApi, rolesApi, type RoleRecord } from "@/lib/api";
import MyWhatsAppChats from "@/components/nary/MyWhatsAppChats";
import UserTwoFactorAdmin from "@/components/settings/UserTwoFactorAdmin";
import UserRolesEditor, { canGrantRole } from "@/components/settings/UserRolesEditor";
import { usePermissions } from "@/hooks/usePermissions";
import { localSource } from "@/lib/entitySources";
import {
  Button, Checkbox, Combobox, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues,
  ListFrame, PageHeader, Pill, Select, StatusPill, type ColumnDef, type Tone,
} from "@/components/ui";

interface UserRecord {
  id: string;
  email: string;
  full_name: string;
  phone?: string;
  department?: string;
  position?: string;
  wa_signature_name?: string | null;
  is_active: boolean;
  is_superadmin: boolean;
  is_system_admin: boolean;
  portal_type: string;
  last_login_at?: string;
  created_at: string;
  roles: string[];
  employee_id?: string;
}

interface Employee {
  id: string;
  full_name: string;
  employee_number?: string;
  department?: string;
  position?: string;
  user_id?: string;
}

const PORTAL_LABELS: Record<string, string> = {
  internal: "Internal",
  partner: "Partner",
  vendor: "Vendor",
};

const PORTAL_TONES: Record<string, Tone> = {
  internal: "blue",
  partner: "violet",
  vendor: "orange",
};

const DEPARTMENT_OPTIONS = GRANARY_DEPARTMENTS.map((d) => ({ value: d, label: d }));

function timeAgo(iso?: string | null) {
  if (!iso) return "Never";
  const d = new Date(iso);
  const diff = Date.now() - d.getTime();
  const days = Math.floor(diff / 86400000);
  if (days === 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days < 30) return `${days}d ago`;
  return d.toLocaleDateString();
}

function apiDetail(err: unknown, fallback: string) {
  return (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail ?? fallback;
}

/** A password input with a show/hide toggle inside it. */
function PasswordInput({ value, onChange, show, onToggle, placeholder }: {
  value: string; onChange: (v: string) => void; show: boolean; onToggle: () => void; placeholder?: string;
}) {
  return (
    <div className="relative">
      <Input type={show ? "text" : "password"} value={value} onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder} className="pr-11" autoComplete="new-password" />
      <IconButton label={show ? "Hide password" : "Show password"} icon={show ? EyeOff : Eye} size="sm"
        onClick={onToggle} className="absolute right-0.5 top-1/2 -translate-y-1/2" />
    </div>
  );
}

function FormError({ children }: { children: React.ReactNode }) {
  if (!children) return null;
  return <p role="alert" className="rounded-lg bg-tone-red-soft px-3 py-2 text-sm text-tone-red">{children}</p>;
}

export default function UsersPage() {
  const { holds, isAdmin, isSuperadmin } = usePermissions();
  const [users, setUsers] = useState<UserRecord[]>([]);
  const [employees, setEmployees] = useState<Employee[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [showInactive, setShowInactive] = useState(false);
  const [allRoles, setAllRoles] = useState<RoleRecord[]>([]);

  // Create dialog
  const [showCreate, setShowCreate] = useState(false);
  const emptyCreate = {
    email: "", name: "", password: "", portal_type: "internal",
    is_system_admin: false, phone: "", department: "", position: "", employee_id: "", role_names: [] as string[],
  };
  const [createForm, setCreateForm] = useState(emptyCreate);
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState("");
  const [showCreatePass, setShowCreatePass] = useState(false);

  // Edit dialog
  const [editUser, setEditUser] = useState<UserRecord | null>(null);
  const [editForm, setEditForm] = useState({ name: "", phone: "", department: "", position: "", wa_signature_name: "", portal_type: "internal", is_system_admin: false, is_active: true });
  const [saving, setSaving] = useState(false);
  const [editError, setEditError] = useState("");

  // Reset password dialog
  const [resetUser, setResetUser] = useState<UserRecord | null>(null);
  const [newPassword, setNewPassword] = useState("");
  const [showNewPass, setShowNewPass] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [resetError, setResetError] = useState("");

  const [deactivating, setDeactivating] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [usersRes, empRes] = await Promise.all([
        fetchAllPages<UserRecord>(usersApi.list, { include_inactive: showInactive }),
        fetchAllPages<Employee>(hrApi.listEmployees),
      ]);
      setUsers(usersRes.rows);
      setEmployees(empRes.rows);
    } finally { setLoading(false); }
  }, [showInactive]);

  useEffect(() => { load(); }, [load]);
  useEffect(() => {
    rolesApi.list().then((r) => setAllRoles(r.data)).catch(() => setAllRoles([]));
  }, []);
  // The roles the signed-in user may hand out (the API refuses the rest).
  const grantableRoles = useMemo(() => allRoles.filter((r) => canGrantRole(r, holds, isSuperadmin)), [allRoles, holds, isSuperadmin]);

  const filtered = users.filter((u) =>
    u.full_name.toLowerCase().includes(search.toLowerCase()) ||
    u.email.toLowerCase().includes(search.toLowerCase()) ||
    (u.department ?? "").toLowerCase().includes(search.toLowerCase())
  );

  // Employees not yet linked to a user
  const linkedEmpIds = new Set(users.map((u) => u.employee_id).filter(Boolean));
  const unlinkedEmployees = employees.filter((e) => !e.user_id && !linkedEmpIds.has(e.id));
  // Local, not server-backed: "unlinked" is derived from the loaded users, so a server
  // query could not express it.
  const unlinkedSource = useMemo(() => localSource(unlinkedEmployees.map((e) => ({
    id: e.id,
    label: `${e.full_name}${e.employee_number ? ` (${e.employee_number})` : ""}`,
    sublabel: e.department || undefined,
  }))), [unlinkedEmployees]);

  async function handleCreate() {
    if (!createForm.email || !createForm.name || !createForm.password) {
      setCreateError("Email, name, and password are required"); return;
    }
    setCreating(true); setCreateError("");
    try {
      await usersApi.create({
        ...createForm,
        employee_id: createForm.employee_id || undefined,
        phone: createForm.phone || undefined,
        department: createForm.department || undefined,
        position: createForm.position || undefined,
      });
      setShowCreate(false);
      setCreateForm(emptyCreate);
      await load();
    } catch (err: unknown) {
      setCreateError(apiDetail(err, "Failed to create user"));
    } finally { setCreating(false); }
  }

  function openEdit(u: UserRecord) {
    setEditUser(u);
    setEditForm({ name: u.full_name, phone: u.phone ?? "", department: u.department ?? "", position: u.position ?? "", wa_signature_name: u.wa_signature_name ?? "", portal_type: u.portal_type, is_system_admin: u.is_system_admin, is_active: u.is_active });
    setEditError("");
  }

  async function handleEdit() {
    if (!editUser) return;
    setSaving(true); setEditError("");
    try {
      await usersApi.update(editUser.id, editForm);
      setEditUser(null);
      await load();
    } catch (err: unknown) {
      setEditError(apiDetail(err, "Update failed"));
    } finally { setSaving(false); }
  }

  async function handleDeactivate(u: UserRecord) {
    if (!confirm(`Deactivate ${u.full_name}? They won't be able to log in.`)) return;
    setDeactivating(u.id);
    try { await usersApi.deactivate(u.id); await load(); }
    finally { setDeactivating(null); }
  }

  async function handleReset() {
    if (!resetUser || !newPassword) return;
    if (newPassword.length < 6) { setResetError("Password must be at least 6 characters"); return; }
    setResetting(true); setResetError("");
    try {
      await usersApi.resetPassword(resetUser.id, newPassword);
      setResetUser(null); setNewPassword("");
    } catch (err: unknown) {
      setResetError(apiDetail(err, "Reset failed"));
    } finally { setResetting(false); }
  }

  function rowActions(u: UserRecord) {
    if (u.is_superadmin) return null;
    return (
      <div className="flex items-center justify-end gap-0.5">
        <IconButton label="Edit user" icon={Edit3} size="sm" onClick={() => openEdit(u)} />
        <IconButton label="Reset password" icon={Key} size="sm"
          onClick={() => { setResetUser(u); setNewPassword(""); setResetError(""); }} />
        {u.is_active ? (
          <IconButton label="Deactivate" icon={UserCheck} size="sm" className="text-tone-red"
            onClick={() => handleDeactivate(u)} disabled={deactivating === u.id} />
        ) : (
          <IconButton label="Reactivate" icon={ShieldOff} size="sm"
            onClick={async () => { await usersApi.update(u.id, { is_active: true }); await load(); }} />
        )}
      </div>
    );
  }

  const columns: ColumnDef<UserRecord>[] = [
    {
      id: "user", header: "User", accessorFn: (u) => u.full_name,
      cell: ({ row: { original: u } }) => (
        <div className="flex min-w-0 items-center gap-2.5 py-1">
          <span aria-hidden className="flex size-7 shrink-0 items-center justify-center rounded-full bg-brand/10 text-xs font-semibold text-brand-ink">
            {u.full_name.charAt(0).toUpperCase()}
          </span>
          <div className="min-w-0">
            <div className="flex items-center gap-1.5">
              <span className="truncate font-medium text-fg">{u.full_name}</span>
              {u.is_superadmin && <Pill tone="violet">Super</Pill>}
              {u.is_system_admin && !u.is_superadmin && <Pill tone="blue">Admin</Pill>}
            </div>
            <p className="truncate text-xs text-fg-muted">{u.email}</p>
          </div>
        </div>
      ),
    },
    {
      id: "portal", header: "Role / portal", accessorFn: (u) => u.portal_type,
      cell: ({ row: { original: u } }) => (
        <div className="flex flex-wrap items-center gap-1.5">
          <Pill tone={PORTAL_TONES[u.portal_type] ?? "neutral"}>{PORTAL_LABELS[u.portal_type] ?? u.portal_type}</Pill>
          {u.roles.length > 0 && (
            <span className="truncate text-xs text-fg-muted">{u.roles.slice(0, 2).join(", ")}{u.roles.length > 2 ? "..." : ""}</span>
          )}
        </div>
      ),
    },
    {
      id: "department", header: "Department", accessorFn: (u) => u.department ?? "", meta: { hideOnMobile: true },
      cell: ({ row: { original: u } }) => u.department
        ? <span className="text-fg-muted">{u.department}{u.position ? ` · ${u.position}` : ""}</span>
        : <span className="text-fg-subtle">-</span>,
    },
    {
      id: "employee", header: "Employee", meta: { hideOnMobile: true },
      accessorFn: (u) => employees.find((e) => e.id === u.employee_id)?.full_name ?? "",
      cell: ({ row: { original: u } }) => {
        const emp = employees.find((e) => e.id === u.employee_id);
        return emp ? <span className="text-fg">{emp.full_name}</span> : <span className="text-fg-subtle">Not linked</span>;
      },
    },
    {
      id: "last_login", header: "Last login", accessorFn: (u) => u.last_login_at ?? "",
      meta: { numeric: true, hideOnMobile: true }, cell: ({ row: { original: u } }) => timeAgo(u.last_login_at),
    },
    {
      id: "status", header: "Status", accessorFn: (u) => (u.is_active ? "active" : "inactive"),
      cell: ({ row: { original: u } }) => <StatusPill status={u.is_active ? "active" : "inactive"} label={u.is_active ? "Active" : "Deactivated"} />,
    },
    { id: "actions", header: "", enableSorting: false, meta: { align: "right" }, cell: ({ row: { original: u } }) => rowActions(u) },
  ];

  return (
    <>
      <ListFrame>
        <PageHeader flush title="Users" back={{ href: "/settings?area=integrations#users", label: "Settings" }}
          summary={`${filtered.length} user${filtered.length !== 1 ? "s" : ""}. Every employee needs a user account to use the ERP.`}
          actions={
            <>
              {isAdmin && (
                <Button asChild><Link href="/settings/roles"><ShieldCheck aria-hidden /> Roles</Link></Button>
              )}
              <Button variant="primary" onClick={() => { setShowCreate(true); setCreateError(""); }}>
                <Plus aria-hidden /> Add user
              </Button>
            </>
          } />
        <FilterChips chips={[{ id: "inactive", label: "Show deactivated", active: showInactive }]}
          onToggle={() => setShowInactive((v) => !v)} onClear={() => setShowInactive(false)}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)}
            placeholder="Search by name, email, department" aria-label="Search users" className="w-64" />
        </FilterChips>
        <DataTable label="Users" columns={columns} data={filtered} getRowId={(u) => u.id} loading={loading}
          empty={{ icon: Users, title: "No users found", description: "Create a user account for each team member." }}
          renderDetails={(u) => {
            const emp = employees.find((e) => e.id === u.employee_id);
            return (
              <div className="flex flex-wrap items-start gap-6">
                <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                  { label: "Email", value: u.email },
                  { label: "Phone", value: u.phone || "-" },
                  { label: "Department", value: u.department ? `${u.department}${u.position ? ` · ${u.position}` : ""}` : "-" },
                  { label: "Roles", value: u.roles.length ? u.roles.join(", ") : "-" },
                  { label: "Employee", value: emp ? `${emp.full_name}${emp.employee_number ? ` (${emp.employee_number})` : ""}` : "Not linked" },
                  { label: "WhatsApp signature name", value: u.wa_signature_name || "Automatic" },
                  { label: "Last login", value: timeAgo(u.last_login_at) },
                  { label: "Created", value: new Date(u.created_at).toLocaleDateString() },
                ]} />
                {/* two-factor sign-in: the Owner or a system admin; only the superadmin resets the superadmin */}
                {isAdmin && (!u.is_superadmin || isSuperadmin) && (
                  <UserTwoFactorAdmin userId={u.id} name={u.full_name} />
                )}
                {!u.is_superadmin && (
                  <div className="flex flex-wrap gap-2">
                    <Button size="sm" onClick={() => openEdit(u)}><Edit3 aria-hidden /> Edit user</Button>
                    <Button size="sm" onClick={() => { setResetUser(u); setNewPassword(""); setResetError(""); }}><Key aria-hidden /> Reset password</Button>
                  </div>
                )}
              </div>
            );
          }} />
      </ListFrame>

      {/* Create */}
      <Dialog open={showCreate} onOpenChange={setShowCreate} title="New user account" size="md"
        footer={<>
          <Button onClick={() => setShowCreate(false)}>Cancel</Button>
          <Button variant="primary" onClick={handleCreate} loading={creating}>Create user</Button>
        </>}>
        <div className="space-y-4">
          <FieldGrid cols={2}>
            <Field label="Full name" required>
              <Input value={createForm.name} onChange={(e) => setCreateForm((f) => ({ ...f, name: e.target.value }))} placeholder="John Doe" />
            </Field>
            <Field label="Email" required>
              <Input type="email" value={createForm.email} onChange={(e) => setCreateForm((f) => ({ ...f, email: e.target.value }))} placeholder="john@example.com" />
            </Field>
            <Field label="Password" required className="md:col-span-2">
              <PasswordInput value={createForm.password} onChange={(v) => setCreateForm((f) => ({ ...f, password: v }))}
                show={showCreatePass} onToggle={() => setShowCreatePass(!showCreatePass)} placeholder="Min 6 characters" />
            </Field>
            <Field label="Portal type">
              <Select value={createForm.portal_type} onChange={(e) => setCreateForm((f) => ({ ...f, portal_type: e.target.value }))}
                options={[{ value: "internal", label: "Internal (staff)" }, { value: "partner", label: "Partner" }, { value: "vendor", label: "Vendor" }]} />
            </Field>
            <Field label="Phone">
              <Input value={createForm.phone} onChange={(e) => setCreateForm((f) => ({ ...f, phone: e.target.value }))} placeholder="+1 ..." />
            </Field>
            <Field label="Department">
              <Select value={createForm.department} onChange={(e) => setCreateForm((f) => ({ ...f, department: e.target.value }))}
                placeholder="Select" options={DEPARTMENT_OPTIONS} />
            </Field>
            <Field label="Position">
              <Input value={createForm.position} onChange={(e) => setCreateForm((f) => ({ ...f, position: e.target.value }))} placeholder="Software Engineer" />
            </Field>
            {unlinkedEmployees.length > 0 && (
              <Field label="Link to employee (optional)" hint="Links the user account to the employee's HR record." className="md:col-span-2">
                <Combobox value={createForm.employee_id} source={unlinkedSource}
                  placeholder="No employee link" emptyLabel="No unlinked employees"
                  onChange={(id) => setCreateForm((f) => ({ ...f, employee_id: id }))} />
              </Field>
            )}
          </FieldGrid>
          {grantableRoles.length > 0 && (
            <fieldset className="space-y-2">
              <legend className="text-sm font-medium text-fg">Roles</legend>
              <p className="text-xs text-fg-muted">Each role gives its permissions. Only roles you may hand out are listed.</p>
              <div className="grid gap-2 md:grid-cols-2">
                {grantableRoles.map((r) => (
                  <Checkbox key={r.id} label={r.name} hint={r.description ?? undefined}
                    checked={createForm.role_names.includes(r.name)}
                    onChange={(e) => setCreateForm((f) => ({
                      ...f,
                      role_names: e.target.checked ? [...f.role_names, r.name] : f.role_names.filter((n) => n !== r.name),
                    }))} />
                ))}
              </div>
            </fieldset>
          )}
          {isAdmin && (
            <Checkbox checked={createForm.is_system_admin}
              onChange={(e) => setCreateForm((f) => ({ ...f, is_system_admin: e.target.checked }))}
              label="System administrator" hint="Full access to all settings." />
          )}
          <FormError>{createError}</FormError>
        </div>
      </Dialog>

      {/* Edit */}
      <Dialog open={!!editUser} onOpenChange={(o) => { if (!o) setEditUser(null); }} title="Edit user"
        description={editUser ? `${editUser.full_name} · ${editUser.email}` : undefined} size="lg"
        footer={<>
          <Button onClick={() => setEditUser(null)}>Cancel</Button>
          <Button variant="primary" onClick={handleEdit} loading={saving}>Save changes</Button>
        </>}>
        {editUser && (
          <div className="space-y-4">
            <FieldGrid cols={2}>
              <Field label="Full name" className="md:col-span-2">
                <Input value={editForm.name} onChange={(e) => setEditForm((f) => ({ ...f, name: e.target.value }))} />
              </Field>
              <Field label="Phone">
                <Input value={editForm.phone} onChange={(e) => setEditForm((f) => ({ ...f, phone: e.target.value }))} placeholder="+1 ..." />
              </Field>
              <Field label="Portal type">
                <Select value={editForm.portal_type} onChange={(e) => setEditForm((f) => ({ ...f, portal_type: e.target.value }))}
                  options={[{ value: "internal", label: "Internal" }, { value: "partner", label: "Partner" }, { value: "vendor", label: "Vendor" }]} />
              </Field>
              <Field label="Department">
                <Select value={editForm.department} onChange={(e) => setEditForm((f) => ({ ...f, department: e.target.value }))}
                  placeholder="Select" options={DEPARTMENT_OPTIONS} />
              </Field>
              <Field label="Position">
                <Input value={editForm.position} onChange={(e) => setEditForm((f) => ({ ...f, position: e.target.value }))} />
              </Field>
              <Field label="WhatsApp signature name" className="md:col-span-2"
                hint={<>Shown as &ldquo;name, company&rdquo; under their WhatsApp messages. Leave empty for the automatic name.</>}>
                <Input value={editForm.wa_signature_name} maxLength={60}
                  onChange={(e) => setEditForm((f) => ({ ...f, wa_signature_name: e.target.value }))}
                  placeholder="Automatic: first name + initials (e.g. Robert J. S.)" />
              </Field>
            </FieldGrid>
            <div className="flex flex-wrap gap-6">
              <Checkbox checked={editForm.is_system_admin} label="System admin"
                onChange={(e) => setEditForm((f) => ({ ...f, is_system_admin: e.target.checked }))} />
              <Checkbox checked={editForm.is_active} label="Active"
                onChange={(e) => setEditForm((f) => ({ ...f, is_active: e.target.checked }))} />
            </div>
            <FormError>{editError}</FormError>
            {isAdmin && (
              <div className="-mx-5 border-t border-line px-5 pt-4">
                <UserRolesEditor userId={editUser.id} roles={editUser.roles} allRoles={allRoles}
                  holds={holds} isSuperadmin={isSuperadmin}
                  onChanged={(roles) => {
                    setEditUser((u) => (u ? { ...u, roles } : u));
                    setUsers((list) => list.map((x) => (x.id === editUser.id ? { ...x, roles } : x)));
                  }} />
              </div>
            )}
            {editUser.portal_type === "internal" && holds("nary:manage") && (
              <div className="-mx-5 border-t border-line pt-1">
                <MyWhatsAppChats userId={editUser.id} title={`${editUser.full_name}'s WhatsApp chats (saved at once)`} />
              </div>
            )}
          </div>
        )}
      </Dialog>

      {/* Reset password */}
      <Dialog open={!!resetUser} onOpenChange={(o) => { if (!o) setResetUser(null); }} title="Reset password" size="sm"
        description={resetUser ? `${resetUser.full_name} · ${resetUser.email}` : undefined}
        footer={<>
          <Button onClick={() => setResetUser(null)}>Cancel</Button>
          <Button variant="primary" onClick={handleReset} loading={resetting} disabled={!newPassword}><Key aria-hidden /> Reset password</Button>
        </>}>
        <div className="space-y-4">
          <Field label="New password">
            <PasswordInput value={newPassword} onChange={setNewPassword} show={showNewPass}
              onToggle={() => setShowNewPass(!showNewPass)} placeholder="Min 6 characters" />
          </Field>
          <p className="rounded-lg bg-tone-orange-soft px-3 py-2 text-xs text-tone-orange">
            The user will be required to change their password on next login.
          </p>
          <FormError>{resetError}</FormError>
        </div>
      </Dialog>
    </>
  );
}
