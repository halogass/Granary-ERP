"use client";

import { useState } from "react";
import { Plus, X } from "lucide-react";
import { usersApi, type RoleRecord } from "@/lib/api";
import { Button, IconButton, Pill, Select } from "@/components/ui";

/** The role the seed gives the superadmin account: only the superadmin hands it out. */
export const SUPERADMIN_ROLE = "superadmin";

/**
 * Whether the signed-in user may hand out `role`, as the API decides (users.rs `ensure_grantable`): the
 * superadmin may grant anything; everyone else only roles whose permissions they all hold, never the
 * superadmin role. The API checks again; this only keeps choices that would fail out of the list.
 */
export function canGrantRole(role: RoleRecord, holds: (code: string) => boolean, isSuperadmin: boolean): boolean {
  if (isSuperadmin) return true;
  if (role.slug === SUPERADMIN_ROLE || role.name === SUPERADMIN_ROLE) return false;
  return role.permissions.every(holds);
}

function apiDetail(err: unknown, fallback: string) {
  return (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail ?? fallback;
}

/**
 * A user's roles with add and remove, saved at once (POST/DELETE /users/{id}/roles/{role}). For system
 * admins only: the page renders it only for them, as the API requires.
 */
export default function UserRolesEditor({ userId, roles, allRoles, holds, isSuperadmin, onChanged }: {
  userId: string;
  roles: string[];
  allRoles: RoleRecord[];
  holds: (code: string) => boolean;
  isSuperadmin: boolean;
  onChanged: (roles: string[]) => void;
}) {
  const [pick, setPick] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const addable = allRoles
    .filter((r) => !roles.includes(r.name) && canGrantRole(r, holds, isSuperadmin))
    .map((r) => ({ value: r.name, label: r.description ? `${r.name} (${r.description})` : r.name }));

  async function add() {
    if (!pick) return;
    setBusy(true); setError("");
    try {
      await usersApi.assignRole(userId, pick);
      onChanged([...roles, pick]);
      setPick("");
    } catch (err) {
      setError(apiDetail(err, "Could not assign the role"));
    } finally { setBusy(false); }
  }

  async function remove(name: string) {
    setBusy(true); setError("");
    try {
      await usersApi.removeRole(userId, name);
      onChanged(roles.filter((r) => r !== name));
    } catch (err) {
      setError(apiDetail(err, "Could not remove the role"));
    } finally { setBusy(false); }
  }

  return (
    <section aria-label="Roles" className="space-y-2">
      <div>
        <h3 className="text-sm font-semibold text-fg">Roles</h3>
        <p className="text-xs text-fg-muted">A role gives its permissions. Changes are saved at once.</p>
      </div>
      <div className="flex flex-wrap gap-1.5">
        {roles.length === 0 && <span className="text-sm text-fg-subtle">No roles</span>}
        {roles.map((name) => {
          const role = allRoles.find((r) => r.name === name);
          const removable = !role || canGrantRole(role, holds, isSuperadmin);
          return (
            <span key={name} className="inline-flex items-center gap-0.5">
              <Pill tone="blue">{name}</Pill>
              {removable && (
                <IconButton label={`Remove role ${name}`} icon={X} size="sm" disabled={busy} onClick={() => remove(name)} />
              )}
            </span>
          );
        })}
      </div>
      {addable.length > 0 && (
        <div className="flex items-center gap-2">
          <div className="min-w-0 flex-1">
            <Select aria-label="Role to add" value={pick} onChange={(e) => setPick(e.target.value)}
              placeholder="Choose a role" options={addable} />
          </div>
          <Button size="sm" onClick={add} disabled={!pick} loading={busy}><Plus aria-hidden /> Add role</Button>
        </div>
      )}
      {error && <p role="alert" className="rounded-lg bg-tone-red-soft px-3 py-2 text-sm text-tone-red">{error}</p>}
    </section>
  );
}
