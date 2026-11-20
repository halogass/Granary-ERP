"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { Edit3, Plus, Search, ShieldCheck } from "lucide-react";
import { rolesApi, type PermissionRecord, type RoleRecord } from "@/lib/api";
import { usePermissions } from "@/hooks/usePermissions";
import {
  Button, Checkbox, DataTable, Dialog, Field, IconButton, Input, ListFrame, PageHeader, Pill, type ColumnDef,
} from "@/components/ui";

function apiDetail(err: unknown, fallback: string) {
  return (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail ?? fallback;
}

/** The module part of a permission code ("crm:view_own" -> "crm"). */
function moduleOf(code: string) {
  const i = code.indexOf(":");
  return i < 0 ? code : code.slice(0, i);
}

interface Editing {
  /** null: a new role. */
  role: RoleRecord | null;
  name: string;
  description: string;
  codes: Set<string>;
}

/**
 * Settings > Roles: the built-in roles (read only) and custom roles, which system admins create and edit.
 * As the API rules: everyone but the superadmin only adds permissions they hold themselves.
 */
export default function RolesPage() {
  const { isAdmin, isSuperadmin, holds, loaded } = usePermissions();
  const [roles, setRoles] = useState<RoleRecord[]>([]);
  const [perms, setPerms] = useState<PermissionRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState("");
  const [editing, setEditing] = useState<Editing | null>(null);
  const [filter, setFilter] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  const load = useCallback(async () => {
    setLoading(true); setLoadError("");
    try {
      const [r, p] = await Promise.all([rolesApi.list(), rolesApi.permissions()]);
      setRoles([...r.data].sort((a, b) => Number(!!b.is_system_role) - Number(!!a.is_system_role) || a.name.localeCompare(b.name)));
      setPerms([...p.data].sort((a, b) => a.code.localeCompare(b.code)));
    } catch (err) {
      setLoadError(apiDetail(err, "Could not load the roles"));
    } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const groups = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const out = new Map<string, PermissionRecord[]>();
    for (const p of perms) {
      if (q && !p.code.toLowerCase().includes(q) && !p.name.toLowerCase().includes(q)) continue;
      const m = moduleOf(p.code);
      out.set(m, [...(out.get(m) ?? []), p]);
    }
    return Array.from(out.entries());
  }, [perms, filter]);

  function openNew() {
    setEditing({ role: null, name: "", description: "", codes: new Set() });
    setError(""); setFilter("");
  }

  function openEdit(role: RoleRecord) {
    setEditing({ role, name: role.name, description: role.description ?? "", codes: new Set(role.permissions) });
    setError(""); setFilter("");
  }

  /** A code may be ticked by the superadmin, or by someone holding it; one already on the role stays. */
  function canTick(code: string) {
    return isSuperadmin || holds(code) || !!editing?.role?.permissions.includes(code);
  }

  function toggle(code: string, on: boolean) {
    setEditing((e) => {
      if (!e) return e;
      const codes = new Set(e.codes);
      if (on) codes.add(code); else codes.delete(code);
      return { ...e, codes };
    });
  }

  async function save() {
    if (!editing) return;
    const name = editing.name.trim();
    if (!editing.role && !/^[a-z0-9_]+$/.test(name)) {
      setError("Use lowercase letters, digits and underscores for the name, e.g. sales_lead");
      return;
    }
    setSaving(true); setError("");
    const permission_codes = Array.from(editing.codes).sort();
    try {
      if (editing.role) {
        await rolesApi.update(editing.role.id, { description: editing.description.trim(), permission_codes });
      } else {
        await rolesApi.create({ name, description: editing.description.trim() || undefined, permission_codes });
      }
      setEditing(null);
      await load();
    } catch (err) {
      setError(apiDetail(err, "Could not save the role"));
    } finally { setSaving(false); }
  }

  const columns: ColumnDef<RoleRecord>[] = [
    {
      id: "name", header: "Role", accessorFn: (r) => r.name,
      cell: ({ row: { original: r } }) => (
        <div className="min-w-0 py-1">
          <div className="flex items-center gap-1.5">
            <span className="truncate font-medium text-fg">{r.name}</span>
            {r.is_system_role ? <Pill tone="neutral">Built-in</Pill> : <Pill tone="violet">Custom</Pill>}
          </div>
          {r.description && <p className="truncate text-xs text-fg-muted">{r.description}</p>}
        </div>
      ),
    },
    {
      id: "permissions", header: "Permissions", accessorFn: (r) => r.permissions.length, meta: { numeric: true },
      cell: ({ row: { original: r } }) => <span className="tabular">{r.permissions.length}</span>,
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right" },
      cell: ({ row: { original: r } }) => (isAdmin && !r.is_system_role
        ? <IconButton label={`Edit role ${r.name}`} icon={Edit3} size="sm" onClick={() => openEdit(r)} />
        : null),
    },
  ];

  return (
    <>
      <ListFrame>
        <PageHeader flush title="Roles" back={{ href: "/settings/users", label: "Users" }}
          summary="A role is a set of permissions given to users. Built-in roles come with Granary; custom roles are yours to shape."
          actions={loaded && isAdmin ? (
            <Button variant="primary" onClick={openNew}><Plus aria-hidden /> New role</Button>
          ) : undefined} />
        {loadError && <p role="alert" className="rounded-lg bg-tone-red-soft px-3 py-2 text-sm text-tone-red">{loadError}</p>}
        <DataTable label="Roles" columns={columns} data={roles} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: ShieldCheck, title: "No roles", description: "Create a custom role to group permissions." }}
          renderDetails={(r) => (
            <div className="flex flex-wrap gap-1.5">
              {r.permissions.length === 0
                ? <span className="text-sm text-fg-subtle">No permissions</span>
                : [...r.permissions].sort().map((c) => <Pill key={c} tone="neutral">{c}</Pill>)}
            </div>
          )} />
      </ListFrame>

      <Dialog open={!!editing} onOpenChange={(o) => { if (!o) setEditing(null); }} size="xl"
        title={editing?.role ? `Edit role ${editing.role.name}` : "New role"}
        description={isSuperadmin ? undefined : "You can add only the permissions you hold yourself."}
        dismissable={!saving}
        footer={<>
          <Button onClick={() => setEditing(null)}>Cancel</Button>
          <Button variant="primary" onClick={save} loading={saving}>{editing?.role ? "Save role" : "Create role"}</Button>
        </>}>
        {editing && (
          <div className="space-y-4">
            {!editing.role && (
              <Field label="Name" required hint="Lowercase letters, digits and underscores; it cannot be changed later.">
                <Input value={editing.name} placeholder="sales_lead"
                  onChange={(e) => setEditing((x) => (x ? { ...x, name: e.target.value } : x))} />
              </Field>
            )}
            <Field label="Description">
              <Input value={editing.description} placeholder="What people with this role do"
                onChange={(e) => setEditing((x) => (x ? { ...x, description: e.target.value } : x))} />
            </Field>
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="text-sm font-medium text-fg">Permissions ({editing.codes.size} selected)</span>
              <Input type="search" leading={Search} value={filter} onChange={(e) => setFilter(e.target.value)}
                placeholder="Filter permissions" aria-label="Filter permissions" className="w-56" />
            </div>
            <div className="space-y-4">
              {groups.map(([mod, list]) => (
                <fieldset key={mod} className="space-y-1.5">
                  <legend className="text-xs font-semibold uppercase tracking-wide text-fg-muted">{mod}</legend>
                  <div className="grid gap-1.5 md:grid-cols-2">
                    {list.map((p) => (
                      <Checkbox key={p.code} label={p.code} hint={p.name}
                        checked={editing.codes.has(p.code)} disabled={!canTick(p.code)}
                        onChange={(e) => toggle(p.code, e.target.checked)} />
                    ))}
                  </div>
                </fieldset>
              ))}
            </div>
            {error && <p role="alert" className="rounded-lg bg-tone-red-soft px-3 py-2 text-sm text-tone-red">{error}</p>}
          </div>
        )}
      </Dialog>
    </>
  );
}
