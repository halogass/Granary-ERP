"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { UserPlus, UserMinus } from "lucide-react";
import { channelApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import Avatar from "@/components/common/Avatar";
import { Button, Dialog, Field, IconButton, Pill, Select, Skeleton } from "@/components/ui";

interface Member {
  user_id: string;
  name: string;
  avatar_url?: string | null;
  role?: string | null;
}

/**
 * A channel's members. Who may add people follows the API: the owners of a private channel, any member of
 * another channel. Only owners remove members; an owner is never removed here.
 */
export default function ChannelMembersDialog({ channelId, channelName, channelType, myId, users, onClose }: {
  channelId: string;
  channelName: string;
  /** Announcement channels keep everyone: no remove buttons there. */
  channelType?: string;
  myId: string;
  /** The messaging people directory (active internal users). */
  users: { id: string; name: string }[];
  onClose: () => void;
}) {
  const [members, setMembers] = useState<Member[] | null>(null);
  const [pick, setPick] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");

  const load = useCallback(async () => {
    try {
      const r = await channelApi.get(channelId);
      setMembers((r.data?.members ?? []) as Member[]);
    } catch (e) {
      setMembers([]);
      setError(extractApiError(e, "Could not load the members"));
    }
  }, [channelId]);

  useEffect(() => { load(); }, [load]);

  const isOwner = !!members?.some((m) => m.user_id === myId && m.role === "owner");
  const isMember = !!members?.some((m) => m.user_id === myId);
  const canInvite = channelType === "private" ? isOwner : isMember;
  const candidates = useMemo(() => {
    const inside = new Set((members ?? []).map((m) => m.user_id));
    return users.filter((u) => !inside.has(u.id))
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((u) => ({ value: u.id, label: u.name }));
  }, [members, users]);

  async function invite() {
    if (!pick) return;
    setBusy("invite"); setError("");
    try {
      await channelApi.invite(channelId, pick);
      setPick("");
      await load();
    } catch (e) {
      setError(extractApiError(e, "Could not add the member"));
    } finally { setBusy(null); }
  }

  async function remove(m: Member) {
    if (!confirm(`Remove ${m.name} from #${channelName}?`)) return;
    setBusy(m.user_id); setError("");
    try {
      await channelApi.removeMember(channelId, m.user_id);
      await load();
    } catch (e) {
      setError(extractApiError(e, "Could not remove the member"));
    } finally { setBusy(null); }
  }

  return (
    <Dialog open onOpenChange={(o) => { if (!o) onClose(); }} title={`Members of #${channelName}`} size="md"
      description={members ? `${members.length} member${members.length === 1 ? "" : "s"}` : undefined}
      footer={<Button onClick={onClose}>Close</Button>}>
      <div className="space-y-4">
        {canInvite && (
          <div className="flex items-end gap-2">
            <Field label="Add a member" className="flex-1">
              <Select value={pick} onChange={(e) => setPick(e.target.value)} placeholder="Choose a person"
                options={candidates} disabled={candidates.length === 0} />
            </Field>
            <Button variant="primary" onClick={invite} loading={busy === "invite"} disabled={!pick}>
              <UserPlus aria-hidden /> Add
            </Button>
          </div>
        )}
        {error && <p role="alert" className="rounded-lg bg-tone-red-soft px-3 py-2 text-sm text-tone-red">{error}</p>}
        {members === null ? (
          <div className="space-y-2">{[0, 1, 2].map((i) => <Skeleton key={i} className="h-9 w-full" />)}</div>
        ) : (
          <ul className="divide-y divide-line-soft" aria-label="Channel members">
            {members.map((m) => (
              <li key={m.user_id} className="flex items-center gap-3 py-2">
                <Avatar name={m.name} />
                <span className="min-w-0 flex-1 truncate text-sm text-fg">{m.name}{m.user_id === myId ? " (you)" : ""}</span>
                {m.role === "owner" && <Pill tone="violet">Owner</Pill>}
                {isOwner && m.role !== "owner" && channelType !== "announcement" && (
                  <IconButton icon={UserMinus} label={`Remove ${m.name}`} size="sm" className="text-tone-red"
                    onClick={() => remove(m)} disabled={busy === m.user_id} />
                )}
              </li>
            ))}
          </ul>
        )}
      </div>
    </Dialog>
  );
}
