"use client";

import { useState } from "react";
import { Search, Hash, Plus, Globe, Lock, Megaphone, MessagesSquare } from "lucide-react";
import { channelApi } from "@/lib/api";
import Avatar from "@/components/common/Avatar";
import { Button, CountBadge, Dialog, Field, IconButton, Input } from "@/components/ui";
import { cn } from "@/lib/utils";
import { mentionsToPlainText } from "@/lib/messageMentions";

export interface ChannelItem {
  id: string;
  name: string;
  slug: string;
  type: "public" | "private" | "announcement";
  is_member: boolean;
  unread_count: number;
  created_by_id?: string | null;
}

export interface ConversationItem {
  thread_id: string;
  other_user_id: string;
  other_user_name: string;
  other_user_avatar?: string;
  last_message?: string;
  last_at?: string;
  unread_count: number;
  is_online: boolean;
}

interface ActiveRoom {
  type: "dm" | "channel";
  id: string;
  name: string;
}

interface Props {
  channels: ChannelItem[];
  conversations: ConversationItem[];
  online: string[];
  activeRoom: ActiveRoom | null;
  myName: string;
  search: string;
  onSearchChange: (v: string) => void;
  onSelectChannel: (ch: ChannelItem) => void;
  onSelectDm: (userId: string, userName: string) => void;
  onChannelsChange: () => void;
  users: { id: string; name: string }[];
  myId: string;
  isSuperadmin?: boolean;
}

function timeAgo(date?: string) {
  if (!date) return "";
  const diff = Date.now() - new Date(date).getTime();
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return "now";
  if (mins < 60) return `${mins}m`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h`;
  return new Date(date).toLocaleDateString("en-GB", { day: "numeric", month: "short" });
}

const rowClass = (active: boolean) => cn(
  "mb-0.5 flex w-full items-center gap-2.5 rounded-lg px-3 text-left transition-colors",
  "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-[rgb(var(--focus)/0.7)]",
  active ? "bg-brand/[0.07] shadow-[inset_2px_0_0_rgb(var(--brand))]" : "hover:bg-surface-2",
);

// Create channel dialog
function CreateChannelDialog({ onClose, onCreate, isSuperadmin }: { onClose: () => void; onCreate: () => void; isSuperadmin?: boolean }) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [type, setType] = useState<"public" | "private" | "announcement">("public");
  const [loading, setLoading] = useState(false);

  const handleCreate = async () => {
    if (!name.trim()) return;
    setLoading(true);
    try {
      await channelApi.create({ name: name.trim(), description: description.trim(), type });
      onCreate();
      onClose();
    } catch {
      // ignore
    } finally {
      setLoading(false);
    }
  };

  return (
    <Dialog open onOpenChange={(o) => { if (!o) onClose(); }} title="Create a channel"
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" onClick={handleCreate} disabled={!name.trim()} loading={loading}>Create channel</Button>
      </>}>
      <div className="space-y-4">
        <Field label="Channel name">
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. general" />
        </Field>
        <Field label="Description" hint="Optional.">
          <Input value={description} onChange={(e) => setDescription(e.target.value)} placeholder="What's this channel about?" />
        </Field>
        <fieldset>
          <legend className="mb-1.5 text-[13px] font-medium text-fg">Visibility</legend>
          <div className="flex flex-col gap-2" role="radiogroup">
            {([
              ["public", Globe, "Anyone can join", "All employees are added automatically"],
              ["private", Lock, "Invite only", "Members must be invited"],
              ...(isSuperadmin ? [["announcement", Megaphone, "Announcement", "Everyone joins and cannot leave"]] : []),
            ] as [string, React.ElementType, string, string][]).map(([val, Icon, label, sub]) => (
              <button
                key={val}
                type="button"
                role="radio"
                aria-checked={type === val}
                onClick={() => setType(val as typeof type)}
                className={cn("flex items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors",
                  type === val ? "border-brand bg-brand/[0.07] text-fg" : "border-line text-fg hover:bg-surface-2")}
              >
                <Icon className={cn("size-4 shrink-0", type === val ? "text-brand-ink" : "text-fg-muted")} aria-hidden />
                <div>
                  <div className="text-sm font-medium">{label}</div>
                  <div className="text-xs text-fg-muted">{sub}</div>
                </div>
              </button>
            ))}
          </div>
        </fieldset>
      </div>
    </Dialog>
  );
}


// Main sidebar
export default function Sidebar({
  channels, conversations, online, activeRoom, myName, search, onSearchChange,
  onSelectChannel, onSelectDm, onChannelsChange, users, myId, isSuperadmin,
}: Props) {
  const [showCreateChannel, setShowCreateChannel] = useState(false);

  const filteredChannels = channels.filter((c) =>
    (c.name ?? "").toLowerCase().includes(search.toLowerCase())
  );

  const convMap = new Map(conversations.map((c) => [c.other_user_id, c]));
  const filtered = users.filter(
    (u) => u.id !== myId && (u.name ?? "").toLowerCase().includes(search.toLowerCase())
  );

  // Users with existing conversations, most recent message first
  const withConv = filtered
    .filter((u) => convMap.has(u.id))
    .map((u) => ({ ...u, conv: convMap.get(u.id)! }))
    .sort((a, b) => {
      const ta = a.conv.last_at ? new Date(a.conv.last_at).getTime() : 0;
      const tb = b.conv.last_at ? new Date(b.conv.last_at).getTime() : 0;
      return tb - ta;
    });

  // Everyone else, sorted A-Z
  const withoutConv = filtered
    .filter((u) => !convMap.has(u.id))
    .map((u) => ({ ...u, conv: null }))
    .sort((a, b) => a.name.localeCompare(b.name));

  return (
    <div className="flex w-full shrink-0 flex-col border-r border-line bg-surface md:w-72">
      {showCreateChannel && (
        <CreateChannelDialog
          onClose={() => setShowCreateChannel(false)}
          onCreate={onChannelsChange}
          isSuperadmin={isSuperadmin}
        />
      )}

      {/* Header + search */}
      <div className="border-b border-line px-4 py-3">
        <div className="mb-3 flex items-center gap-2">
          <MessagesSquare className="size-4 text-fg-muted" aria-hidden />
          <h2 className="text-base font-semibold text-fg">Messaging</h2>
        </div>
        <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
          value={search} onChange={(e) => onSearchChange(e.target.value)}
          placeholder="Search channels and people" aria-label="Search channels and people" />
      </div>

      <nav className="flex-1 space-y-4 overflow-y-auto px-2 py-3" aria-label="Conversations">

        {/* Channels */}
        <div>
          <div className="mb-1 flex items-center justify-between pl-3 pr-1">
            <p className="text-xs font-medium text-fg-muted">Channels</p>
            <IconButton size="sm" icon={Plus} label="New channel" onClick={() => setShowCreateChannel(true)} />
          </div>

          {/* Announcement channels first, pinned */}
          {filteredChannels.filter((c) => c.type === "announcement" && c.is_member).map((ch) => {
            const isActive = activeRoom?.type === "channel" && activeRoom.id === ch.id;
            return (
              <button key={ch.id} type="button" onClick={() => onSelectChannel(ch)} aria-current={isActive || undefined}
                className={cn(rowClass(isActive), "min-h-11 md:min-h-9")}>
                <Megaphone className="size-4 shrink-0 text-tone-orange" aria-hidden />
                <span className={cn("flex-1 truncate text-sm text-fg", isActive && "font-medium")}>{ch.name}</span>
                <CountBadge count={ch.unread_count} className="shrink-0" />
              </button>
            );
          })}

          {/* Regular member channels */}
          {filteredChannels.filter((c) => c.type !== "announcement" && c.is_member).map((ch) => {
            const isActive = activeRoom?.type === "channel" && activeRoom.id === ch.id;
            return (
              <button key={ch.id} type="button" onClick={() => onSelectChannel(ch)} aria-current={isActive || undefined}
                className={cn(rowClass(isActive), "min-h-11 md:min-h-9")}>
                <Hash className={cn("size-4 shrink-0", isActive ? "text-brand-ink" : "text-fg-subtle")} aria-hidden />
                <span className={cn("flex-1 truncate text-sm", isActive ? "font-medium text-fg" : "text-fg")}>{ch.name}</span>
                <CountBadge count={ch.unread_count} className="shrink-0" />
              </button>
            );
          })}

          {/* Discover public channels */}
          {filteredChannels.filter((c) => !c.is_member && c.type === "public").length > 0 && (
            <div className="mt-2">
              <p className="px-3 py-1 text-xs text-fg-subtle">Browse</p>
              {filteredChannels.filter((c) => !c.is_member && c.type === "public").map((ch) => (
                <button key={ch.id} type="button" onClick={() => onSelectChannel(ch)}
                  className={cn(rowClass(false), "min-h-11 md:min-h-9")}>
                  <Hash className="size-4 shrink-0 text-fg-subtle" aria-hidden />
                  <span className="flex-1 truncate text-sm text-fg-muted">{ch.name}</span>
                  <span className="text-xs text-fg-subtle">Join</span>
                </button>
              ))}
            </div>
          )}
        </div>

        {/* Direct messages */}
        <div>
          <p className="mb-1 px-3 text-xs font-medium text-fg-muted">Direct messages</p>

          {withConv.length === 0 && withoutConv.length === 0 && (
            <p className="px-4 py-3 text-center text-xs text-fg-muted">No people found</p>
          )}

          {/* Users with message history, by recency */}
          {withConv.map(({ id, name, conv }) => {
            const isActive = activeRoom?.type === "dm" && activeRoom.id === id;
            const isOnline = online.includes(id);
            return (
              <button key={id} type="button" onClick={() => onSelectDm(id, name)} aria-current={isActive || undefined}
                className={cn(rowClass(isActive), "gap-3 py-2")}>
                <Avatar name={name} size="sm" online={isOnline} />
                <div className="min-w-0 flex-1">
                  <div className="flex items-center justify-between">
                    <span className={cn("truncate text-sm text-fg", (isActive || (conv.unread_count ?? 0) > 0) ? "font-semibold" : "font-medium")}>
                      {name}
                    </span>
                    {conv.last_at && (
                      <span className="ml-2 shrink-0 text-xs text-fg-muted tabular">{timeAgo(conv.last_at)}</span>
                    )}
                  </div>
                  <div className="mt-0.5 flex items-center justify-between">
                    <p className="truncate pr-2 text-xs text-fg-muted">{mentionsToPlainText(conv.last_message)}</p>
                    <CountBadge count={conv.unread_count} className="shrink-0" />
                  </div>
                </div>
              </button>
            );
          })}

          {/* Everyone else, A-Z */}
          {withoutConv.length > 0 && (
            <>
              <p className={cn("px-3 text-xs text-fg-subtle", withConv.length > 0 ? "mb-1 mt-3" : "mb-1")}>People</p>
              {withoutConv.map(({ id, name }) => {
                const isActive = activeRoom?.type === "dm" && activeRoom.id === id;
                const isOnline = online.includes(id);
                return (
                  <button key={id} type="button" onClick={() => onSelectDm(id, name)} aria-current={isActive || undefined}
                    className={cn(rowClass(isActive), "min-h-11 gap-3 md:min-h-10")}>
                    <Avatar name={name} size="sm" online={isOnline} />
                    <span className={cn("truncate text-sm", isActive ? "font-semibold text-fg" : "text-fg-muted")}>{name}</span>
                  </button>
                );
              })}
            </>
          )}
        </div>
      </nav>

      {/* My status */}
      <div className="flex items-center gap-3 border-t border-line px-4 py-3">
        <Avatar name={myName} size="sm" online={true} />
        <div className="min-w-0">
          <p className="truncate text-xs font-medium text-fg">{myName}</p>
          <p className="text-xs text-fg-muted">Active</p>
        </div>
      </div>
    </div>
  );
}
