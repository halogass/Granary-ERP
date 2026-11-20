"use client";
import { useEffect, useMemo, useState, useCallback } from "react";
import {
  DndContext, DragOverlay, closestCorners, MouseSensor, TouchSensor, useSensor, useSensors,
  useDroppable, type DragStartEvent, type DragEndEvent,
} from "@dnd-kit/core";
import { SortableContext, useSortable, verticalListSortingStrategy } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { ArrowDown, ArrowUp, Plus, Layers, X, Settings2, Search } from "lucide-react";
import { peopleApi, collabApi } from "@/lib/api";
import { Issue, IssueAdapter, PRIORITY_META, Member, Epic, StatusColumn } from "@/lib/issues";
import IssueDetailDrawer from "@/components/pm/IssueDetailDrawer";
import { Button, Dialog, IconButton, Input, Select, Skeleton, StatusPill, Textarea } from "@/components/ui";
import { cn } from "@/lib/utils";
import { MoveToMenu } from "@/components/dnd/MoveToMenu";

// Epic and column colors are user data (stored per epic/column and editable), not page styling.
const EPIC_COLORS = ["#8b5cf6", "#3b82f6", "#06b6d4", "#22c55e", "#f59e0b", "#ef4444", "#ec4899", "#64748b"];
interface BoardCol { id: string; key: string; label: string; color: string; sort_order: number }

function Avatar({ member }: { member?: Member | null }) {
  if (!member) return <div className="size-5 rounded-full border border-dashed border-line-strong" title="Unassigned" />;
  if (member.avatar_url)
    // eslint-disable-next-line @next/next/no-img-element
    return <img src={member.avatar_url} alt={member.name} title={member.name} className="size-5 rounded-full object-cover" />;
  return (
    <div title={member.name} className="flex size-5 items-center justify-center rounded-full bg-surface-2 text-[9px] font-semibold text-fg-muted">
      {(member.name ?? "?").charAt(0).toUpperCase()}
    </div>
  );
}

function IssueCard({ issue, members, usesStoryPoints, epic, onClick, dragging, actions }: {
  issue: Issue; members: Member[]; usesStoryPoints: boolean; epic?: Epic | null; onClick?: () => void; dragging?: boolean;
  /** Extra controls next to the assignee ("Move to"). */
  actions?: React.ReactNode;
}) {
  const assignee = members.find((m) => m.id === issue.assigneeId) || null;
  const pr = issue.priority ? PRIORITY_META[issue.priority] : null;
  return (
    <div onClick={onClick}
      className={cn("cursor-pointer rounded-lg border border-line bg-surface p-3 shadow-card transition-colors hover:border-line-strong",
        dragging && "ring-2 ring-brand/30")}>
      {epic && (
        <span className="mb-1.5 inline-flex items-center gap-1 text-[11px] font-medium">
          <span className="size-2 shrink-0 rounded-full" style={{ background: epic.color }} aria-hidden />
          <Layers className="size-3 text-fg-subtle" aria-hidden /><span className="text-fg-muted">{epic.title}</span>
        </span>
      )}
      <p className="mb-2 line-clamp-3 text-sm font-medium leading-snug text-fg">{issue.title}</p>
      {issue.labels.length > 0 && (
        <div className="mb-2 flex flex-wrap gap-1">
          {issue.labels.slice(0, 4).map((l) => (
            <span key={l} className="rounded bg-surface-2 px-1.5 py-0.5 text-[11px] text-fg-muted">{l}</span>
          ))}
        </div>
      )}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-1.5">
          {pr && <StatusPill family="priority" status={issue.priority} label={pr.label} className="h-5 px-2 text-[11px]" />}
          {usesStoryPoints && issue.storyPoints != null && (
            <span className="flex size-5 items-center justify-center rounded-full bg-surface-2 text-[11px] font-semibold text-fg-muted tabular">{issue.storyPoints}</span>
          )}
          {!usesStoryPoints && issue.estimate != null && (
            <span className="text-[11px] text-fg-muted tabular">{issue.estimate} h</span>
          )}
        </div>
        <div className="flex items-center gap-1">
          {actions}
          <Avatar member={assignee} />
        </div>
      </div>
    </div>
  );
}

function SortableCard(props: {
  issue: Issue; members: Member[]; usesStoryPoints: boolean; epic?: Epic | null; onOpen: (i: Issue) => void;
  columns: StatusColumn[]; onMove: (id: string, toCol: string) => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id: props.issue.id });
  const style = { transform: CSS.Transform.toString(transform), transition, opacity: isDragging ? 0.4 : 1 };
  return (
    // touch-manipulation, not touch-none: the touch sensor waits for a press-and-hold, so a swipe
    // that starts on a card still scrolls the board between columns.
    <div ref={setNodeRef} style={style} {...attributes} {...listeners} className="mb-2 touch-manipulation">
      <IssueCard issue={props.issue} members={props.members} usesStoryPoints={props.usesStoryPoints}
        epic={props.epic} onClick={() => props.onOpen(props.issue)}
        actions={<MoveToMenu itemLabel={props.issue.title} current={props.issue.status}
          options={props.columns.map((c) => ({ value: c.key, label: c.label }))}
          onMove={(to) => props.onMove(props.issue.id, to)} />} />
    </div>
  );
}

function Column({ colKey, children }: { colKey: string; children: React.ReactNode }) {
  const { setNodeRef, isOver } = useDroppable({ id: colKey });
  return (
    <div ref={setNodeRef} className={cn("min-h-[60vh] flex-1 rounded-lg transition-colors", isOver && "bg-brand/5")}>
      {children}
    </div>
  );
}

export default function KanbanBoard({ adapter, scopeId, onCountChange, reloadKey }: {
  adapter: IssueAdapter; scopeId: string; onCountChange?: (n: number) => void; reloadKey?: number;
}) {
  const [issues, setIssues] = useState<Issue[]>([]);
  const [members, setMembers] = useState<Member[]>([]);
  const [loading, setLoading] = useState(true);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [openId, setOpenId] = useState<string | null>(null);
  const [adding, setAdding] = useState<string | null>(null);   // column key in add-mode
  const [newTitle, setNewTitle] = useState("");
  const [epics, setEpics] = useState<Epic[]>([]);
  const [epicFilter, setEpicFilter] = useState<string>("");   // "" = all, "none" = no epic, else epic id
  const [showEpics, setShowEpics] = useState(false);
  const [customCols, setCustomCols] = useState<BoardCol[]>([]);
  const [showCols, setShowCols] = useState(false);
  const [query, setQuery] = useState("");
  const [assigneeFilter, setAssigneeFilter] = useState("");
  const [priorityFilter, setPriorityFilter] = useState("");

  // Mouse: small drag threshold. Touch: press-and-hold ~220ms before dragging so a
  // tap/scroll on a tablet isn't hijacked (and the OS long-press menu doesn't fire).
  const sensors = useSensors(
    useSensor(MouseSensor, { activationConstraint: { distance: 5 } }),
    useSensor(TouchSensor, { activationConstraint: { delay: 220, tolerance: 8 } }),
  );

  // Resolved columns: custom (if any) override the adapter defaults.
  const columns: StatusColumn[] = customCols.length > 0
    ? customCols.map((c) => ({ key: c.key, label: c.label, accent: c.color }))
    : adapter.statuses;

  const loadEpics = useCallback(async () => {
    try {
      const r = await collabApi.listEpics(adapter.scopeType, scopeId);
      setEpics(r.data ?? []);
    } catch { setEpics([]); }
  }, [adapter.scopeType, scopeId]);

  const loadCols = useCallback(async () => {
    try {
      const r = await collabApi.listColumns(adapter.scopeType, scopeId);
      setCustomCols(r.data ?? []);
    } catch { setCustomCols([]); }
  }, [adapter.scopeType, scopeId]);

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      const data = await adapter.list(scopeId);
      setIssues(data);
      onCountChange?.(data.length);
    } catch { setIssues([]); } finally { setLoading(false); }
  }, [adapter, scopeId, onCountChange]);

  useEffect(() => { reload(); }, [reload, reloadKey]);
  useEffect(() => { loadEpics(); }, [loadEpics, reloadKey]);
  useEffect(() => { loadCols(); }, [loadCols, reloadKey]);
  useEffect(() => {
    // The staff directory (any internal user may read it), not the admin users list (403 for staff).
    peopleApi.directory().then((r) => {
      setMembers((r.data ?? []).map((u) => ({ id: String(u.id), name: u.name || "User", avatar_url: u.avatar_url ?? null })));
    }).catch(() => {});
  }, []);

  const epicById = useMemo(() => new Map(epics.map((e) => [e.id, e] as [string, Epic])), [epics]);

  const byColumn = useMemo(() => {
    const map: Record<string, Issue[]> = {};
    for (const col of columns) map[col.key] = [];
    const q = query.trim().toLowerCase();
    const visible = issues.filter((i) => {
      if (epicFilter === "none" && i.epicId) return false;
      if (epicFilter && epicFilter !== "none" && i.epicId !== epicFilter) return false;
      if (assigneeFilter === "none" && i.assigneeId) return false;
      if (assigneeFilter && assigneeFilter !== "none" && i.assigneeId !== assigneeFilter) return false;
      if (priorityFilter && i.priority !== priorityFilter) return false;
      if (q && !(i.title.toLowerCase().includes(q) || i.labels.some((l) => l.toLowerCase().includes(q)))) return false;
      return true;
    });
    for (const it of visible) (map[it.status] ?? (map[it.status] = [])).push(it);
    for (const k of Object.keys(map)) map[k].sort((a, b) => a.rank - b.rank);
    return map;
  }, [issues, columns, epicFilter, assigneeFilter, priorityFilter, query]);

  const columnOf = (id: string): string | null => {
    if (columns.some((c) => c.key === id)) return id;              // dropped on empty column
    return issues.find((i) => i.id === id)?.status ?? null;
  };

  async function onDragEnd(e: DragEndEvent) {
    setActiveId(null);
    const { active, over } = e;
    if (!over) return;
    const toCol = columnOf(String(over.id));
    if (!toCol) return;
    await moveIssue(String(active.id), toCol, String(over.id) === toCol ? null : String(over.id));
  }

  /** Move an issue into a column, before `beforeId` or at the end: by dragging, or with "Move to". */
  async function moveIssue(id: string, toCol: string, beforeId: string | null = null) {
    const fromCol = issues.find((i) => i.id === id)?.status;
    if (!fromCol) return;

    // Build the target column ordering after the move.
    const moved = issues.find((i) => i.id === id)!;
    const target = (byColumn[toCol] ?? []).filter((i) => i.id !== id);
    let insertAt = target.length;
    if (beforeId) {
      const idx = target.findIndex((i) => i.id === beforeId);
      if (idx >= 0) insertAt = idx;
    }
    target.splice(insertAt, 0, { ...moved, status: toCol });

    // Optimistic local update with sequential ranks for the target column.
    const reranked = new Map<string, number>();
    target.forEach((i, idx) => reranked.set(i.id, idx));
    setIssues((prev) => prev.map((i) => {
      if (i.id === moved.id) return { ...i, status: toCol, rank: reranked.get(i.id) ?? i.rank };
      if (reranked.has(i.id)) return { ...i, rank: reranked.get(i.id)! };
      return i;
    }));

    // Persist: the moved item (status + rank) + any target item whose rank changed.
    try {
      await adapter.update(moved.id, { status: toCol, [adapter.rankField]: reranked.get(moved.id) ?? 0 });
      await Promise.all(target
        .filter((i) => i.id !== moved.id && i.rank !== reranked.get(i.id))
        .map((i) => adapter.update(i.id, { [adapter.rankField]: reranked.get(i.id) ?? 0 })));
    } catch { reload(); }   // revert to server truth on failure
  }

  async function quickAdd(colKey: string) {
    const title = newTitle.trim();
    if (!title) { setAdding(null); return; }
    setNewTitle("");
    setAdding(null);
    try {
      const created = await adapter.create(scopeId, { title, status: colKey });
      if (created) setIssues((prev) => [...prev, { ...created, status: colKey }]);
      else reload();
      onCountChange?.(issues.length + 1);
    } catch { /* ignore */ }
  }

  const active = activeId ? issues.find((i) => i.id === activeId) : null;
  const openIssue = openId ? issues.find((i) => i.id === openId) : null;

  if (loading) return (
    <div className="flex gap-3" aria-label="Loading board">
      {[0, 1, 2].map((i) => <Skeleton key={i} className="h-64 min-w-[280px] flex-1" />)}
    </div>
  );

  return (
    <>
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
          value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search" aria-label="Search issues" className="w-full sm:w-44" />
        <Select aria-label="Assignee" value={assigneeFilter} onChange={(e) => setAssigneeFilter(e.target.value)} className="w-40"
          options={[{ value: "", label: "Anyone" }, { value: "none", label: "Unassigned" }, ...members.map((m) => ({ value: m.id, label: m.name ?? "" }))]} />
        <Select aria-label="Priority" value={priorityFilter} onChange={(e) => setPriorityFilter(e.target.value)} className="w-36"
          options={[{ value: "", label: "Any priority" }, ...["critical", "high", "medium", "low"].map((p) => ({ value: p, label: PRIORITY_META[p]?.label ?? p }))]} />
        <Select aria-label="Epic" value={epicFilter} onChange={(e) => setEpicFilter(e.target.value)} className="w-36"
          options={[{ value: "", label: "All epics" }, { value: "none", label: "No epic" }, ...epics.map((ep) => ({ value: ep.id, label: ep.title }))]} />
        {(query || assigneeFilter || priorityFilter || epicFilter) && (
          <Button size="sm" variant="ghost" onClick={() => { setQuery(""); setAssigneeFilter(""); setPriorityFilter(""); setEpicFilter(""); }}>Clear</Button>
        )}
        <Button size="sm" variant="ghost" onClick={() => setShowEpics(true)}><Layers aria-hidden /> Epics</Button>
        <Button size="sm" variant="ghost" className="ml-auto" onClick={() => setShowCols(true)}><Settings2 aria-hidden /> Columns</Button>
      </div>
      <DndContext sensors={sensors} collisionDetection={closestCorners}
        onDragStart={(e: DragStartEvent) => setActiveId(String(e.active.id))}
        onDragEnd={onDragEnd}>
        {/* Phones: one column at a time (85% wide, the next one peeking), snapping as you swipe */}
        <div className="flex snap-x snap-mandatory gap-3 overflow-x-auto pb-2 md:snap-none">
          {columns.map((col) => (
            <div key={col.key} className="w-[85%] shrink-0 snap-start rounded-lg border border-line bg-surface-2 p-2.5 md:w-auto md:min-w-[280px] md:flex-1 md:shrink">
              <div className="mb-2.5 flex items-center justify-between px-1">
                <div className="flex items-center gap-2">
                  <span className="size-2 rounded-full" style={{ background: col.accent }} aria-hidden />
                  <span className="text-[13px] font-semibold text-fg">{col.label}</span>
                  <span className="text-xs text-fg-muted tabular">{byColumn[col.key]?.length ?? 0}</span>
                </div>
              </div>
              <Column colKey={col.key}>
                <SortableContext items={(byColumn[col.key] ?? []).map((i) => i.id)} strategy={verticalListSortingStrategy}>
                  {(byColumn[col.key] ?? []).map((issue) => (
                    <SortableCard key={issue.id} issue={issue} members={members}
                      usesStoryPoints={adapter.usesStoryPoints}
                      epic={issue.epicId ? epicById.get(issue.epicId) : null}
                      onOpen={(i) => setOpenId(i.id)} columns={columns} onMove={(id, to) => { void moveIssue(id, to); }} />
                  ))}
                </SortableContext>
              </Column>
              {adding === col.key ? (
                <Textarea autoFocus value={newTitle} onChange={(e) => setNewTitle(e.target.value)} aria-label={`New issue in ${col.label}`}
                  onBlur={() => quickAdd(col.key)}
                  onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); quickAdd(col.key); } if (e.key === "Escape") { setAdding(null); setNewTitle(""); } }}
                  placeholder="What needs doing?" rows={2}
                  className="mt-1 min-h-0 resize-none" />
              ) : (
                <Button size="sm" variant="ghost" className="mt-1 w-full justify-start text-fg-muted" onClick={() => { setAdding(col.key); setNewTitle(""); }}>
                  <Plus aria-hidden /> Add
                </Button>
              )}
            </div>
          ))}
        </div>
        <DragOverlay>
          {active ? <IssueCard issue={active} members={members} usesStoryPoints={adapter.usesStoryPoints} dragging /> : null}
        </DragOverlay>
      </DndContext>

      {openIssue && (
        <IssueDetailDrawer
          adapter={adapter}
          issue={openIssue}
          members={members}
          epics={epics}
          statuses={columns}
          onClose={() => setOpenId(null)}
          onChanged={(updated) => setIssues((prev) => prev.map((i) => i.id === updated.id ? updated : i))}
          onDeleted={(id) => { setIssues((prev) => prev.filter((i) => i.id !== id)); setOpenId(null); }}
        />
      )}

      {showEpics && (
        <EpicManager
          adapter={adapter}
          scopeId={scopeId}
          epics={epics}
          onClose={() => setShowEpics(false)}
          onChanged={loadEpics}
        />
      )}

      {showCols && (
        <ColumnManager
          adapter={adapter}
          scopeId={scopeId}
          customCols={customCols}
          onClose={() => setShowCols(false)}
          onChanged={loadCols}
        />
      )}
    </>
  );
}

function EpicManager({ adapter, scopeId, epics, onClose, onChanged }: {
  adapter: IssueAdapter; scopeId: string; epics: Epic[]; onClose: () => void; onChanged: () => void;
}) {
  const [title, setTitle] = useState("");
  const [color, setColor] = useState(EPIC_COLORS[0]);
  const [busy, setBusy] = useState(false);

  async function create() {
    if (!title.trim() || busy) return;
    setBusy(true);
    try {
      await collabApi.createEpic({ scope_type: adapter.scopeType, scope_id: scopeId, title: title.trim(), color });
      setTitle("");
      onChanged();
    } catch { /* ignore */ } finally { setBusy(false); }
  }
  async function rename(ep: Epic, t: string) {
    if (!t.trim() || t === ep.title) return;
    try { await collabApi.updateEpic(ep.id, { title: t.trim() }); onChanged(); } catch { /* ignore */ }
  }
  async function recolor(ep: Epic, c: string) {
    try { await collabApi.updateEpic(ep.id, { color: c }); onChanged(); } catch { /* ignore */ }
  }
  async function remove(ep: Epic) {
    if (!confirm(`Delete epic "${ep.title}"? Issues keep their data but lose the epic.`)) return;
    try { await collabApi.deleteEpic(ep.id); onChanged(); } catch { /* ignore */ }
  }

  return (
    <Dialog open onOpenChange={(o) => { if (!o) onClose(); }} title="Epics"
      footer={
        <div className="flex w-full items-center gap-2">
          <input type="color" value={color} onChange={(e) => setColor(e.target.value)} aria-label="New epic color"
            className="size-9 shrink-0 cursor-pointer rounded-lg border border-line-strong bg-surface" />
          <Input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="New epic title" aria-label="New epic title"
            onKeyDown={(e) => { if (e.key === "Enter") create(); }} className="flex-1" />
          <Button variant="primary" onClick={create} disabled={busy || !title.trim()}>Add epic</Button>
        </div>
      }>
      <div className="space-y-2">
        {epics.length === 0 && <p className="text-xs text-fg-muted">No epics yet.</p>}
        {epics.map((ep) => (
          <div key={ep.id} className="flex items-center gap-2">
            <input type="color" value={ep.color} onChange={(e) => recolor(ep, e.target.value)} aria-label={`Color of ${ep.title}`}
              className="size-8 shrink-0 cursor-pointer rounded-lg border border-line-strong bg-surface" />
            <Input defaultValue={ep.title} onBlur={(e) => rename(ep, e.target.value)} aria-label="Epic title" className="flex-1" />
            <IconButton variant="ghost" icon={X} label={`Delete epic ${ep.title}`} onClick={() => remove(ep)} />
          </div>
        ))}
      </div>
    </Dialog>
  );
}

function ColumnManager({ adapter, scopeId, customCols, onClose, onChanged }: {
  adapter: IssueAdapter; scopeId: string; customCols: BoardCol[]; onClose: () => void; onChanged: () => void;
}) {
  const [label, setLabel] = useState("");
  const [color, setColor] = useState(EPIC_COLORS[7]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const cols = [...customCols].sort((a, b) => a.sort_order - b.sort_order);

  async function customize() {
    setBusy(true);
    try {
      await collabApi.seedColumns({ scope_type: adapter.scopeType, scope_id: scopeId,
        columns: adapter.statuses.map((s) => ({ key: s.key, label: s.label, color: s.accent })) });
      onChanged();
    } catch { /* ignore */ } finally { setBusy(false); }
  }
  async function add() {
    if (!label.trim()) return;
    setBusy(true); setErr("");
    try {
      await collabApi.createColumn({ scope_type: adapter.scopeType, scope_id: scopeId,
        key: label.trim().toLowerCase().replace(/\s+/g, "_"), label: label.trim(), color, sort_order: cols.length });
      setLabel("");
      onChanged();
    } catch { /* ignore */ } finally { setBusy(false); }
  }
  async function rename(c: BoardCol, v: string) { if (v.trim() && v !== c.label) { try { await collabApi.updateColumn(c.id, { label: v.trim() }); onChanged(); } catch {} } }
  async function recolor(c: BoardCol, v: string) { try { await collabApi.updateColumn(c.id, { color: v }); onChanged(); } catch {} }
  async function move(i: number, dir: -1 | 1) {
    const j = i + dir;
    if (j < 0 || j >= cols.length) return;
    const a = cols[i], b = cols[j];
    try { await Promise.all([collabApi.updateColumn(a.id, { sort_order: b.sort_order }), collabApi.updateColumn(b.id, { sort_order: a.sort_order })]); onChanged(); } catch {}
  }
  async function remove(c: BoardCol) {
    setErr("");
    try { await collabApi.deleteColumn(c.id); onChanged(); }
    catch (e: any) { setErr(e?.response?.data?.detail ?? "Could not delete column"); }
  }

  return (
    <Dialog open onOpenChange={(o) => { if (!o) onClose(); }} title="Board columns"
      footer={customCols.length > 0 ? (
        <div className="flex w-full items-center gap-2">
          <input type="color" value={color} onChange={(e) => setColor(e.target.value)} aria-label="New column color"
            className="size-9 shrink-0 cursor-pointer rounded-lg border border-line-strong bg-surface" />
          <Input value={label} onChange={(e) => setLabel(e.target.value)} placeholder="New column name" aria-label="New column name"
            onKeyDown={(e) => { if (e.key === "Enter") add(); }} className="flex-1" />
          <Button variant="primary" onClick={add} disabled={busy || !label.trim()}>Add column</Button>
        </div>
      ) : undefined}>
      {customCols.length === 0 ? (
        <div className="space-y-3 py-2 text-center">
          <p className="text-sm text-fg-muted">This board uses the default columns. Customize to add, rename, reorder or recolor them.</p>
          <Button variant="primary" onClick={customize} loading={busy}>{busy ? "Setting up..." : "Customize columns"}</Button>
        </div>
      ) : (
        <div className="space-y-2">
          {cols.map((c, i) => (
            <div key={c.id} className="flex items-center gap-1.5">
              <input type="color" value={c.color} onChange={(e) => recolor(c, e.target.value)} aria-label={`Color of ${c.label}`}
                className="size-8 shrink-0 cursor-pointer rounded-lg border border-line-strong bg-surface" />
              <Input defaultValue={c.label} onBlur={(e) => rename(c, e.target.value)} aria-label="Column name" className="flex-1" />
              <IconButton size="sm" variant="ghost" icon={ArrowUp} label={`Move ${c.label} up`} onClick={() => move(i, -1)} disabled={i === 0} />
              <IconButton size="sm" variant="ghost" icon={ArrowDown} label={`Move ${c.label} down`} onClick={() => move(i, 1)} disabled={i === cols.length - 1} />
              <IconButton size="sm" variant="ghost" icon={X} label={`Delete column ${c.label}`} onClick={() => remove(c)} />
            </div>
          ))}
          {err && <p role="alert" className="text-xs text-tone-red">{err}</p>}
        </div>
      )}
    </Dialog>
  );
}
