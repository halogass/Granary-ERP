"use client";
import { useEffect, useState, useCallback, useMemo } from "react";
import { Trash2, Clock, Activity as ActivityIcon, MessageSquare, Paperclip, Send } from "lucide-react";
import AttachmentPanel from "@/components/ui/AttachmentPanel";
import { collabApi } from "@/lib/api";
import {
  Combobox, Field, IconButton, Input, Select, Sheet, Tabs, TabsContent, TabsList, TabsTrigger, Textarea,
} from "@/components/ui";
import { localSource } from "@/lib/entitySources";
import { Issue, IssueAdapter, PRIORITY_META, Member, Epic, StatusColumn } from "@/lib/issues";

interface Comment { id: string; body: string; author?: { id: string; name: string; avatar_url?: string | null } | null; created_at?: string; edited_at?: string | null }
interface ActivityRow { id: string; verb: string; field?: string | null; old_value?: string | null; new_value?: string | null; actor?: { name: string } | null; created_at?: string }

const PRIORITIES = ["critical", "high", "medium", "low"];

function timeAgo(iso?: string | null) {
  if (!iso) return "";
  const d = new Date(iso); const s = (Date.now() - d.getTime()) / 1000;
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return d.toLocaleDateString("en-GB", { day: "numeric", month: "short" });
}

export default function IssueDetailDrawer({ adapter, issue, members, epics = [], statuses, onClose, onChanged, onDeleted }: {
  adapter: IssueAdapter; issue: Issue; members: Member[]; epics?: Epic[]; statuses?: StatusColumn[];
  onClose: () => void; onChanged: (i: Issue) => void; onDeleted: (id: string) => void;
}) {
  const cols = statuses && statuses.length > 0 ? statuses : adapter.statuses;
  const [draft, setDraft] = useState<Issue>(issue);
  const [tab, setTab] = useState<"comments" | "activity" | "files">("comments");
  const [comments, setComments] = useState<Comment[]>([]);
  const [activity, setActivity] = useState<ActivityRow[]>([]);
  const [newComment, setNewComment] = useState("");
  const [mentionIds, setMentionIds] = useState<string[]>([]);
  const [mentionQuery, setMentionQuery] = useState<string | null>(null);
  const [desc, setDesc] = useState(issue.description ?? "");
  const [labelsText, setLabelsText] = useState(issue.labels.join(", "));
  const [posting, setPosting] = useState(false);

  useEffect(() => { setDraft(issue); setDesc(issue.description ?? ""); setLabelsText(issue.labels.join(", ")); }, [issue]);

  const loadCollab = useCallback(async () => {
    try { const [c, a] = await Promise.all([
      collabApi.listComments(adapter.entityType, issue.id),
      collabApi.listActivity(adapter.entityType, issue.id),
    ]); setComments(c.data ?? []); setActivity(a.data ?? []); } catch { /* ignore */ }
  }, [adapter.entityType, issue.id]);
  useEffect(() => { loadCollab(); }, [loadCollab]);

  // Persist a patch and merge it locally.
  async function save(patch: Record<string, unknown>, merge: Partial<Issue>) {
    const next = { ...draft, ...merge };
    setDraft(next);
    onChanged(next);
    try { await adapter.update(issue.id, patch); await loadCollab(); } catch { /* keep optimistic */ }
  }

  // @-mention: detect the token the caret is in.
  function onCommentChange(v: string, caret: number) {
    setNewComment(v);
    const upto = v.slice(0, caret);
    const m = upto.match(/@([\w.]*)$/);
    setMentionQuery(m ? m[1] : null);
  }
  function pickMention(member: Member) {
    const first = (member.name ?? "user").split(" ")[0];
    setNewComment((v) => v.replace(/@([\w.]*)$/, `@${first} `));
    setMentionIds((ids) => ids.includes(member.id) ? ids : [...ids, member.id]);
    setMentionQuery(null);
  }
  const mentionMatches = mentionQuery !== null
    ? members.filter((m) => (m.name ?? "").toLowerCase().includes(mentionQuery.toLowerCase())).slice(0, 6)
    : [];

  async function addComment() {
    const body = newComment.trim();
    if (!body || posting) return;
    setPosting(true);
    try {
      const res = await collabApi.addComment(adapter.entityType, issue.id, body, mentionIds);
      setComments((p) => [...p, res.data]);
      setNewComment("");
      setMentionIds([]);
      setMentionQuery(null);
      loadCollab();
    } catch { /* ignore */ } finally { setPosting(false); }
  }

  async function removeComment(id: string) {
    try { await collabApi.deleteComment(id); setComments((p) => p.filter((c) => c.id !== id)); } catch { /* ignore */ }
  }

  async function del() {
    try { await adapter.remove(issue.id); onDeleted(issue.id); } catch { /* ignore */ }
  }

  const assignee = members.find((m) => m.id === draft.assigneeId) || null;
  const memberSource = useMemo(
    () => localSource(members.map((m) => ({ id: m.id, label: m.name ?? "" }))),
    [members],
  );

  function activityText(a: ActivityRow): string {
    if (a.verb === "comment") return "commented";
    if (a.verb === "status") return `changed status from ${a.old_value ?? "?"} to ${a.new_value ?? "?"}`;
    if (a.verb === "assignee") return "changed the assignee";
    if (a.verb === "priority") return `set priority to ${a.new_value ?? "?"}`;
    return `updated ${a.field ?? "an issue"}`;
  }

  const fieldsGrid = "grid grid-cols-2 gap-3";

  return (
    <Sheet open onOpenChange={(o) => { if (!o) onClose(); }} title={draft.title || "Issue"}
      footer={tab === "comments" ? (
        <div className="relative flex w-full items-end gap-2">
          {mentionMatches.length > 0 && (
            <div role="listbox" aria-label="Mention someone"
              className="absolute bottom-full left-0 z-10 mb-1 w-56 overflow-hidden rounded-lg border border-line bg-surface-raised shadow-pop">
              {mentionMatches.map((m) => (
                <button key={m.id} type="button" role="option" aria-selected={false}
                  onMouseDown={(e) => { e.preventDefault(); pickMention(m); }}
                  className="flex w-full items-center gap-2 px-3 py-2 text-left hover:bg-surface-2">
                  <span className="flex size-6 items-center justify-center rounded-full bg-surface-2 text-[10px] font-semibold text-fg-muted">{(m.name ?? "?").charAt(0).toUpperCase()}</span>
                  <span className="text-sm text-fg">{m.name}</span>
                </button>
              ))}
            </div>
          )}
          <Textarea value={newComment} aria-label="Write a comment"
            onChange={(e) => onCommentChange(e.target.value, e.target.selectionStart ?? e.target.value.length)} rows={1}
            onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey && mentionMatches.length === 0) { e.preventDefault(); addComment(); } }}
            placeholder="Write a comment, use @ to mention"
            className="max-h-28 min-h-0 flex-1 resize-none" />
          <IconButton variant="primary" icon={Send} label="Send comment" onClick={addComment} disabled={!newComment.trim() || posting} />
        </div>
      ) : undefined}>
      <div className="space-y-4">
        <div className="flex items-start gap-2">
          <Field label="Title" className="flex-1">
            <Input
              value={draft.title}
              onChange={(e) => setDraft({ ...draft, title: e.target.value })}
              onBlur={() => { if (draft.title.trim() && draft.title !== issue.title) save({ title: draft.title.trim() }, { title: draft.title.trim() }); }}
              className="font-medium"
            />
          </Field>
          <IconButton variant="danger" icon={Trash2} label="Delete issue" onClick={del} className="mt-6" />
        </div>

        <div className={fieldsGrid}>
          <Field label="Status">
            <Select value={draft.status} onChange={(e) => save({ status: e.target.value }, { status: e.target.value })}
              options={cols.map((st) => ({ value: st.key, label: st.label }))} />
          </Field>
          <Field label="Priority">
            <Select value={draft.priority ?? ""} onChange={(e) => save({ priority: e.target.value }, { priority: e.target.value })}
              placeholder="-" options={PRIORITIES.map((pr) => ({ value: pr, label: PRIORITY_META[pr]?.label ?? pr }))} />
          </Field>
          <Field label="Assignee">
            <Combobox value={draft.assigneeId ?? ""} source={memberSource}
              placeholder="Unassigned" emptyLabel="No members"
              onChange={(id) => save({ [adapter.assigneeField]: id || null }, { assigneeId: id || null })} />
          </Field>
          <Field label="Epic">
            <Select value={draft.epicId ?? ""} onChange={(e) => save({ epic_id: e.target.value || null }, { epicId: e.target.value || null })}
              placeholder="None" options={epics.map((ep) => ({ value: ep.id, label: ep.title }))} />
          </Field>
          {adapter.usesStoryPoints ? (
            <Field label="Story points">
              <Input numeric type="number" min={0} defaultValue={draft.storyPoints ?? ""} onBlur={(e) => { const n = e.target.value ? parseInt(e.target.value) : null; save({ story_points: n }, { storyPoints: n }); }} />
            </Field>
          ) : (
            <Field label="Estimate (h)">
              <Input numeric type="number" min={0} step="0.5" defaultValue={draft.estimate ?? ""} onBlur={(e) => { const n = e.target.value ? parseFloat(e.target.value) : null; save({ estimated_hours: n }, { estimate: n }); }} />
            </Field>
          )}
          {!adapter.usesStoryPoints && (
            <Field label="Due date">
              <Input type="date" defaultValue={draft.dueDate ?? ""} onBlur={(e) => save({ due_date: e.target.value || null }, { dueDate: e.target.value || null })} />
            </Field>
          )}
          <Field label="Labels" className="col-span-2">
            <Input value={labelsText} onChange={(e) => setLabelsText(e.target.value)}
              onBlur={() => { const arr = labelsText.split(",").map((x) => x.trim()).filter(Boolean); save({ [adapter.labelsField]: arr }, { labels: arr }); }}
              placeholder="comma, separated" />
          </Field>
        </div>

        <Field label="Description">
          <Textarea value={desc} onChange={(e) => setDesc(e.target.value)}
            onBlur={() => { if (desc !== (issue.description ?? "")) save({ description: desc }, { description: desc }); }}
            rows={4} placeholder="Add a description..." className="resize-y" />
        </Field>

        <Tabs value={tab} onValueChange={(v) => setTab(v as typeof tab)}>
          <TabsList>
            <TabsTrigger value="comments"><MessageSquare className="size-3.5" aria-hidden /> Comments</TabsTrigger>
            <TabsTrigger value="activity"><ActivityIcon className="size-3.5" aria-hidden /> Activity</TabsTrigger>
            <TabsTrigger value="files"><Paperclip className="size-3.5" aria-hidden /> Files</TabsTrigger>
          </TabsList>
          <TabsContent value="comments">
            <div className="space-y-3">
              {comments.length === 0 && <p className="text-xs text-fg-muted">No comments yet.</p>}
              {comments.map((c) => (
                <div key={c.id} className="group flex gap-2.5">
                  <div aria-hidden className="flex size-7 shrink-0 items-center justify-center rounded-full bg-surface-2 text-[10px] font-semibold text-fg-muted">
                    {(c.author?.name ?? "?").charAt(0).toUpperCase()}
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="text-xs font-semibold text-fg">{c.author?.name ?? "Unknown"}</span>
                      <span className="text-[11px] text-fg-subtle">{timeAgo(c.created_at)}{c.edited_at ? ", edited" : ""}</span>
                      <button type="button" onClick={() => removeComment(c.id)}
                        className="ml-auto min-h-11 px-2 text-[11px] text-fg-subtle hover:text-tone-red md:min-h-0 md:px-0 [@media(hover:hover)]:opacity-0 [@media(hover:hover)]:group-hover:opacity-100 [@media(hover:hover)]:focus-within:opacity-100 [@media(hover:hover)]:focus-visible:opacity-100">Delete</button>
                    </div>
                    <p className="whitespace-pre-wrap break-words text-sm text-fg">{c.body}</p>
                  </div>
                </div>
              ))}
            </div>
          </TabsContent>
          <TabsContent value="activity">
            <div className="space-y-3">
              {activity.length === 0 && <p className="text-xs text-fg-muted">No activity yet.</p>}
              {activity.map((a) => (
                <div key={a.id} className="flex items-start gap-2 text-xs text-fg-muted">
                  <Clock className="mt-0.5 size-3.5 shrink-0 text-fg-subtle" aria-hidden />
                  <p><span className="font-semibold text-fg">{a.actor?.name ?? "Someone"}</span> {activityText(a)} <span className="text-fg-subtle">{timeAgo(a.created_at)}</span></p>
                </div>
              ))}
            </div>
          </TabsContent>
          <TabsContent value="files">
            <AttachmentPanel entityType={adapter.entityType} entityId={issue.id} />
          </TabsContent>
        </Tabs>
      </div>
    </Sheet>
  );
}
