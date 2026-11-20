"use client";

import { useCallback, useEffect, useState } from "react";
import { Users } from "lucide-react";
import { gitlabApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Button, Card, CardHeader, EmptyState, Input } from "@/components/ui";
import { toast } from "@/components/ui/Toast";

interface Person {
  user_id: string;
  name?: string | null;
  email?: string | null;
  gitlab_user_id?: number | null;
  gitlab_username?: string | null;
  matched_by?: string | null;
}

/** 17 Nov 2026: who is who between the ERP and one GitLab server (assignees and comment authors in the issue sync). */
export default function GitLabPeople({ configId, serverName }: { configId: string; serverName: string }) {
  const [people, setPeople] = useState<Person[]>([]);
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await gitlabApi.listPeople(configId);
      setPeople(res.data ?? []);
    } catch { setPeople([]); } finally { setLoading(false); }
  }, [configId]);

  useEffect(() => { load(); }, [load]);

  async function save(p: Person) {
    const value = (draft[p.user_id] ?? p.gitlab_username ?? "").trim();
    setSaving(p.user_id);
    try {
      await gitlabApi.setPerson(configId, p.user_id, value);
      toast.success(value ? `${p.name ?? "User"} matched to @${value.replace(/^@/, "")}` : "Match cleared");
      setDraft((d) => { const n = { ...d }; delete n[p.user_id]; return n; });
      await load();
    } catch (e) { toast.error(extractApiError(e)); } finally { setSaving(null); }
  }

  const matched = (p: Person) => !!p.gitlab_user_id;

  return (
    <Card>
      <CardHeader title={`People on ${serverName}`}
        description="Assignees and comment authors are matched by e-mail where GitLab shows it; set the rest by GitLab username." />
      {loading ? <p className="px-4 py-3 text-sm text-fg-muted">Loading people...</p> : people.length === 0 ? (
        <EmptyState compact icon={Users} title="No active staff" />
      ) : (
        <ul className="divide-y divide-line">
          {people.map((p) => {
            const value = draft[p.user_id] ?? p.gitlab_username ?? "";
            const dirty = draft[p.user_id] !== undefined && draft[p.user_id] !== (p.gitlab_username ?? "");
            return (
              <li key={p.user_id} className="flex flex-wrap items-center gap-3 px-4 py-2">
                <div className="min-w-0 flex-1">
                  <p className="text-sm font-medium text-fg">{p.name ?? "-"}</p>
                  <p className="truncate text-xs text-fg-muted">{p.email}</p>
                </div>
                <span className="w-28 text-xs text-fg-muted">
                  {matched(p) ? (p.matched_by === "manual" ? "Set by hand" : "Matched by e-mail") : "Not matched"}
                </span>
                <Input className="w-48 font-mono" value={value} placeholder="GitLab username" aria-label={`GitLab username for ${p.name ?? "user"}`}
                  autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
                  onChange={(e) => setDraft((d) => ({ ...d, [p.user_id]: e.target.value }))} />
                <Button size="sm" disabled={!dirty} loading={saving === p.user_id} onClick={() => save(p)}>Save</Button>
              </li>
            );
          })}
        </ul>
      )}
    </Card>
  );
}
