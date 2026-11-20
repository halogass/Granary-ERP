"use client";

import { useState, useEffect, useCallback, useMemo } from "react";
import { Plus, Users, Trash2, Pencil, Clock } from "lucide-react";
import api from "@/lib/api";
import {
  Button, DataTable, Dialog, Field, FieldGrid, IconButton, Input, ListFrame, PageHeader, Pill, Select, Textarea,
  toast, type ColumnDef, type Tone,
} from "@/components/ui";
import { stack } from "@/lib/stackTable";
import { cn } from "@/lib/utils";

interface Team { id: string; name: string; email: string | null; description: string | null; }
interface SLAPolicy { id: string; team_id: string; name: string; priority: string; response_hours: number; resolution_hours: number; }

const PRIORITY_ORDER = ["critical", "high", "normal", "low"];
const PRIORITY_TONES: Record<string, Tone> = { critical: "red", high: "orange", normal: "neutral", low: "neutral" };

export default function TeamsPage() {
  const [teams, setTeams] = useState<Team[]>([]);
  const [policies, setPolicies] = useState<SLAPolicy[]>([]);
  const [loading, setLoading] = useState(true);
  const [activeTeamId, setActiveTeamId] = useState<string | null>(null);

  const [showTeamModal, setShowTeamModal] = useState(false);
  const [editingTeam, setEditingTeam] = useState<Team | null>(null);
  const [teamForm, setTeamForm] = useState({ name: "", email: "", description: "" });
  const [teamSaving, setTeamSaving] = useState(false);

  const [showSlaModal, setShowSlaModal] = useState(false);
  const [slaForm, setSlaForm] = useState({ name: "", priority: "normal", response_hours: "4", resolution_hours: "24" });
  const [slaSaving, setSlaSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [tRes, pRes] = await Promise.all([
        api.get("/helpdesk/teams"),
        api.get("/helpdesk/sla-policies"),
      ]);
      setTeams(tRes.data ?? []);
      setPolicies(pRes.data ?? []);
    } catch { setTeams([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const activeTeam = teams.find(t => t.id === activeTeamId) ?? null;
  const policiesFor = (teamId: string) => policies.filter(p => p.team_id === teamId)
    .sort((a, b) => PRIORITY_ORDER.indexOf(a.priority) - PRIORITY_ORDER.indexOf(b.priority));

  async function saveTeam() {
    if (!teamForm.name.trim()) return toast.error("Name required");
    setTeamSaving(true);
    try {
      if (editingTeam) {
        await api.patch(`/helpdesk/teams/${editingTeam.id}`, { name: teamForm.name.trim(), email: teamForm.email || null, description: teamForm.description || null });
        toast.success("Team updated");
      } else {
        await api.post("/helpdesk/teams", { name: teamForm.name.trim(), email: teamForm.email || null, description: teamForm.description || null });
        toast.success("Team created");
      }
      setShowTeamModal(false);
      load();
    } catch { toast.error("Failed"); } finally { setTeamSaving(false); }
  }

  async function deleteTeam(id: string) {
    try { await api.delete(`/helpdesk/teams/${id}`); toast.success("Team deactivated"); load(); }
    catch { toast.error("Failed"); }
  }

  async function saveSla() {
    if (!activeTeam) return;
    if (!slaForm.name.trim()) return toast.error("Name required");
    setSlaSaving(true);
    try {
      await api.post("/helpdesk/sla-policies", { team_id: activeTeam.id, name: slaForm.name.trim(), priority: slaForm.priority, response_hours: parseFloat(slaForm.response_hours), resolution_hours: parseFloat(slaForm.resolution_hours) });
      toast.success("SLA policy created");
      setShowSlaModal(false);
      load();
    } catch { toast.error("Failed"); } finally { setSlaSaving(false); }
  }

  async function deleteSla(id: string) {
    try { await api.delete(`/helpdesk/sla-policies/${id}`); toast.success("Deleted"); load(); }
    catch { toast.error("Failed"); }
  }

  function openEdit(t: Team) {
    setEditingTeam(t);
    setTeamForm({ name: t.name, email: t.email ?? "", description: t.description ?? "" });
    setShowTeamModal(true);
  }

  const columns = useMemo<ColumnDef<Team>[]>(() => [
    {
      accessorKey: "name", header: "Team",
      cell: (c) => (
        <div className="min-w-0">
          <p className="truncate font-medium text-fg">{c.row.original.name}</p>
          {c.row.original.description && <p className="max-w-md truncate text-xs text-fg-muted">{c.row.original.description}</p>}
        </div>
      ),
    },
    { accessorKey: "email", header: "Inbox email", meta: { hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{c.getValue<string>() ?? "-"}</span> },
    { id: "slas", header: "SLA policies", meta: { numeric: true }, cell: (c) => policies.filter((p) => p.team_id === c.row.original.id).length },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => (
        <div className="flex items-center justify-end gap-1">
          <IconButton size="sm" icon={Pencil} label={`Edit ${c.row.original.name}`} onClick={() => openEdit(c.row.original)} />
          <IconButton size="sm" icon={Trash2} label={`Delete ${c.row.original.name}`} onClick={() => deleteTeam(c.row.original.id)} />
        </div>
      ),
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [policies]);

  return (
    <>
      <ListFrame>
        <PageHeader flush title="Teams" summary="Support teams and their SLA policies"
          actions={
            <Button variant="primary" onClick={() => { setEditingTeam(null); setTeamForm({ name: "", email: "", description: "" }); setShowTeamModal(true); }}>
              <Plus aria-hidden /> New team
            </Button>
          } />
        <DataTable label="Teams" columns={columns} data={teams} getRowId={(r) => r.id} loading={loading}
          expandedId={activeTeamId} onExpandedChange={setActiveTeamId}
          empty={{ icon: Users, title: "No teams yet", description: "Create a support team, then set its SLA policies." }}
          renderDetails={(team) => {
            const teamPolicies = policiesFor(team.id);
            return (
              <div className="space-y-3">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div>
                    <h3 className="text-sm font-semibold text-fg">SLA policies</h3>
                    <p className="text-xs text-fg-muted">Response and resolution time targets per priority level</p>
                  </div>
                  <Button size="sm" variant="primary" onClick={() => { setActiveTeamId(team.id); setSlaForm({ name: "", priority: "normal", response_hours: "4", resolution_hours: "24" }); setShowSlaModal(true); }}>
                    <Plus aria-hidden /> Add SLA
                  </Button>
                </div>
                {teamPolicies.length === 0 ? (
                  <p className="flex items-center gap-2 text-xs text-fg-muted"><Clock className="size-3.5" aria-hidden /> No SLA policies for this team yet.</p>
                ) : (
                  <table className={cn("w-full max-w-2xl overflow-hidden rounded-lg border border-line bg-surface text-[13px]", stack.table, "max-md:border-0 max-md:bg-transparent")}>
                    <thead className={cn("bg-surface-2 text-left text-[11px] uppercase tracking-[0.04em] text-fg-muted", stack.thead)}>
                      <tr>
                        <th scope="col" className="px-3 py-2 font-medium">Priority</th>
                        <th scope="col" className="px-3 py-2 font-medium">Name</th>
                        <th scope="col" className="px-3 py-2 text-right font-medium">First response</th>
                        <th scope="col" className="px-3 py-2 text-right font-medium">Resolution</th>
                        <th scope="col" className="w-px px-3 py-2"><span className="sr-only">Actions</span></th>
                      </tr>
                    </thead>
                    <tbody className={cn("divide-y divide-line-soft", stack.tbody, "max-md:p-0")}>
                      {teamPolicies.map(p => (
                        <tr key={p.id} className={stack.tr}>
                          <td data-label="Priority" className={cn("px-3 py-1.5", stack.td)}><Pill tone={PRIORITY_TONES[p.priority] ?? "neutral"} className="capitalize">{p.priority}</Pill></td>
                          <td data-label="Name" className={cn("px-3 py-1.5 text-fg", stack.td)}>{p.name}</td>
                          <td data-label="First response" className={cn("px-3 py-1.5 text-right font-medium text-fg tabular", stack.td)}>{p.response_hours}h</td>
                          <td data-label="Resolution" className={cn("px-3 py-1.5 text-right font-medium text-fg tabular", stack.td)}>{p.resolution_hours}h</td>
                          <td className={cn("px-3 py-1.5", stack.actions)}><IconButton size="sm" icon={Trash2} label={`Delete SLA ${p.name}`} onClick={() => deleteSla(p.id)} /></td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>
            );
          }} />
      </ListFrame>

      <Dialog open={showTeamModal} onOpenChange={setShowTeamModal} title={editingTeam ? "Edit team" : "New team"}
        footer={<>
          <Button onClick={() => setShowTeamModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={saveTeam} loading={teamSaving}>{teamSaving ? "Saving..." : editingTeam ? "Save" : "Create"}</Button>
        </>}>
        <div className="space-y-4">
          <Field label="Team name" required>
            <Input value={teamForm.name} onChange={e => setTeamForm({ ...teamForm, name: e.target.value })} placeholder="e.g. Technical Support" />
          </Field>
          <Field label="Inbox email">
            <Input type="email" value={teamForm.email} onChange={e => setTeamForm({ ...teamForm, email: e.target.value })} placeholder="support@example.com" />
          </Field>
          <Field label="Description">
            <Textarea value={teamForm.description} onChange={e => setTeamForm({ ...teamForm, description: e.target.value })} rows={2} />
          </Field>
        </div>
      </Dialog>

      <Dialog open={showSlaModal} onOpenChange={setShowSlaModal} title="Add SLA policy" description={activeTeam?.name}
        footer={<>
          <Button onClick={() => setShowSlaModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={saveSla} loading={slaSaving}>{slaSaving ? "Saving..." : "Create policy"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Name" required>
            <Input value={slaForm.name} onChange={e => setSlaForm({ ...slaForm, name: e.target.value })} placeholder="Standard" />
          </Field>
          <Field label="Priority">
            <Select value={slaForm.priority} onChange={e => setSlaForm({ ...slaForm, priority: e.target.value })}
              options={PRIORITY_ORDER.map(p => ({ value: p, label: p.charAt(0).toUpperCase() + p.slice(1) }))} />
          </Field>
          <Field label="First response (h)" required>
            <Input numeric type="number" min="0.5" step="0.5" value={slaForm.response_hours} onChange={e => setSlaForm({ ...slaForm, response_hours: e.target.value })} />
          </Field>
          <Field label="Resolution (h)" required>
            <Input numeric type="number" min="1" step="1" value={slaForm.resolution_hours} onChange={e => setSlaForm({ ...slaForm, resolution_hours: e.target.value })} />
          </Field>
        </FieldGrid>
      </Dialog>
    </>
  );
}
