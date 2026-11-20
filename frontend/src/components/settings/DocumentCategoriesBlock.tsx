"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { Button, Card, CardBody, CardFooter, CardHeader, Checkbox, Skeleton, Switch, toast } from "@/components/ui";
import { usersApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { docLibApi, itemsOf, type DocCategory } from "@/lib/documentLibrary";
import { statusLabel } from "@/components/ui/status";

const BASE_ROLES = ["executive", "system_admin", "finance_admin", "sales_bizdev", "marketing", "team_lead",
  "project_manager", "hr_admin", "manufacturing_tech"];
const PORTAL_ROLES = new Set(["partner_portal", "vendor_portal", "superadmin"]);

/** Settings, Knowledge & documents: which roles see each library category (none ticked = all staff). */
export default function DocumentCategoriesBlock() {
  const [rows, setRows] = useState<DocCategory[] | null>(null);
  const [saved, setSaved] = useState<string>("");
  const [roles, setRoles] = useState<string[]>(BASE_ROLES);
  const [saving, setSaving] = useState(false);
  const [failed, setFailed] = useState(false);

  const load = useCallback(async () => {
    try {
      const r = await docLibApi.categories();
      const list = itemsOf(r.data).sort((a, b) => (a.sort_order ?? 0) - (b.sort_order ?? 0))
        .map((c) => ({ ...c, visible_roles: c.visible_roles ?? [] }));
      setRows(list);
      setSaved(JSON.stringify(list));
      setFailed(false);
    } catch {
      setRows([]);
      setFailed(true);
    }
  }, []);

  useEffect(() => { void load(); }, [load]);

  // Every role in use, so a new role shows up without a code change.
  useEffect(() => {
    usersApi.list({ include_inactive: false }).then((r) => {
      const seen = new Set(BASE_ROLES);
      for (const u of itemsOf<{ roles?: string[] }>(r.data)) for (const role of u.roles ?? []) if (!PORTAL_ROLES.has(role)) seen.add(role);
      setRoles(Array.from(seen));
    }).catch(() => { /* the base list stays */ });
  }, []);

  const dirty = useMemo(() => rows !== null && JSON.stringify(rows) !== saved, [rows, saved]);

  const setRoleOn = (catId: string, role: string, on: boolean) =>
    setRows((rs) => rs && rs.map((c) => c.id !== catId ? c
      : { ...c, visible_roles: on ? [...(c.visible_roles ?? []).filter((x) => x !== role), role] : (c.visible_roles ?? []).filter((x) => x !== role) }));
  const setAll = (catId: string, all: boolean) =>
    setRows((rs) => rs && rs.map((c) => (c.id !== catId ? c : { ...c, visible_roles: all ? [] : ["executive"] })));

  async function save() {
    if (!rows) return;
    setSaving(true);
    try {
      await docLibApi.saveCategories(rows.map((c) => ({ id: c.id, key: c.key, visible_roles: c.visible_roles ?? [] })));
      setSaved(JSON.stringify(rows));
      toast.success("Category access saved");
    } catch (e) {
      toast.error(extractApiError(e, "Category access could not be saved."));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Card>
      <CardHeader title="Who sees each category"
        description="Directors and the system admin always see everything. Partners and customers only get what is marked for them." />
      <CardBody className="space-y-4">
        {rows === null ? <Skeleton className="h-32 w-full" />
          : failed ? <p className="text-[13px] text-fg-muted">The categories could not be loaded. Only directors and the system admin may change them.</p>
          : rows.map((c) => {
            const everyone = (c.visible_roles ?? []).length === 0;
            return (
              <fieldset key={c.id} className="rounded-lg border border-line p-3">
                <legend className="px-1 text-sm font-medium text-fg">{c.name}</legend>
                <Switch label="All staff" checked={everyone} onCheckedChange={(v) => setAll(c.id, v)} />
                {!everyone && (
                  <div className="mt-3 grid grid-cols-1 gap-x-4 gap-y-2 sm:grid-cols-2 lg:grid-cols-3">
                    {roles.map((r) => (
                      <Checkbox key={r} label={statusLabel(r)} checked={(c.visible_roles ?? []).includes(r)}
                        onChange={(e) => setRoleOn(c.id, r, e.target.checked)} />
                    ))}
                  </div>
                )}
              </fieldset>
            );
          })}
      </CardBody>
      {rows && rows.length > 0 && (
        <CardFooter>
          <Button onClick={() => { setRows(JSON.parse(saved)); }} disabled={!dirty || saving}>Discard</Button>
          <Button variant="primary" loading={saving} disabled={!dirty} onClick={save}>Save category access</Button>
        </CardFooter>
      )}
    </Card>
  );
}
