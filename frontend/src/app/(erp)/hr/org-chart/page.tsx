"use client";
import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useMemo,
  useCallback,
} from "react";
import { useSession } from "next-auth/react";
import { Users, GitBranch, Plus, Trash2, RefreshCw, AlertCircle, ArrowLeftRight } from "lucide-react";
import { orgChartApi } from "@/lib/api";
import { Button, Dialog, EmptyState, Field, FieldGrid, IconButton, Input, PageHeader, Pill, Select } from "@/components/ui";
import { cn } from "@/lib/utils";

// Types

interface OrgEmployee {
  id: string;
  full_name: string;
  position: string | null;
  department: string | null;
  reports_to_id: string | null;
  status: string;
}

interface CoordLink {
  id: string;
  from_department: string;
  to_department: string;
  notes?: string | null;
}

interface SvgLine {
  x1: number; y1: number;
  x2: number; y2: number;
  type: "report" | "coord";
}

// Department config

// Default departments
// R&D = product & internal tool engineers
// Op Tech = project-delivery engineers (custom AI/web services), sits under Operations
// Operations = PM, Admin, Legal
// Finance = own department
// HR = own department
// Marketing & Sales = BD, Sales, Marketing, Partner
const DEPT_GROUPS = [
  { key: "management",    label: "Management" },
  { key: "finance",       label: "Finance" },
  { key: "hr",            label: "HR" },
  { key: "sales",         label: "Marketing & Sales" },
  { key: "rnd",           label: "R&D" },
  { key: "operations",    label: "Operations" },
  { key: "optech",        label: "Op Tech" },
  { key: "manufacturing", label: "Manufacturing" },
  { key: "other",         label: "Other" },
];

const CANONICAL_DEPT_LABELS = [
  "Management", "Finance", "HR", "Marketing & Sales", "R&D", "Operations", "Op Tech", "Manufacturing",
];

function deptKey(dept: string | null | undefined): string {
  const d = (dept || "").toLowerCase().trim();
  if (!d) return "operations";
  if (d.includes("management") || d.includes("owner") || d.includes("director") || d.includes("founder") || d.includes("executive"))
    return "management";
  if (d.includes("marketing") || d.includes("sales") || d.includes("business") || d.includes("partnership") || d.includes("partner"))
    return "sales";
  if (d === "r&d" || d.includes("r & d") || d.includes("rnd") || d.includes("research") || d.includes("product") || d.includes("design"))
    return "rnd";
  if (d.includes("op tech") || d.includes("optech") || d.includes("operational tech"))
    return "optech";
  if (d.includes("manufactur") || d.includes("production") || d.includes("assembly") || d.includes("procurement") || d === "qc")
    return "manufacturing";
  if (d === "finance" || d.includes("financ") || d.includes("accounting") || d.includes("treasury"))
    return "finance";
  if (d === "hr" || d === "human resources" || d.includes("human resource") || d.includes("hrd"))
    return "hr";
  // Operations: project management, admin, legal, delivery (fallback)
  return "operations";
}

function getDeptConfig(key: string) {
  return DEPT_GROUPS.find((d) => d.key === key) ?? DEPT_GROUPS[DEPT_GROUPS.length - 1];
}

function getInitials(name: string): string {
  return name.split(" ").filter(Boolean).slice(0, 2).map((w) => w[0]).join("").toUpperCase();
}

// Mini Employee Card

function EmpCard({
  emp,
  lead,
  nodeRef,
}: {
  emp: OrgEmployee;
  lead?: boolean;
  nodeRef?: (el: HTMLDivElement | null) => void;
}) {
  return (
    <div
      ref={nodeRef}
      className="flex min-w-[160px] max-w-full items-center gap-2 rounded-lg border border-line bg-surface px-3 py-2.5 shadow-card"
    >
      <div className={cn("flex size-8 shrink-0 items-center justify-center rounded-full text-xs font-semibold",
        lead ? "bg-brand text-brand-on" : "bg-surface-2 text-fg-muted")}>
        {getInitials(emp.full_name)}
      </div>
      <div className="min-w-0">
        <div className="truncate text-[13px] font-medium leading-tight text-fg" title={emp.full_name}>
          {emp.full_name}
        </div>
        <div className="mt-0.5 truncate text-xs text-fg-muted" title={emp.position ?? "-"}>
          {emp.position || "-"}
        </div>
      </div>
    </div>
  );
}

// Department Group Box

interface DeptGroupProps {
  deptKey: string;
  employees: OrgEmployee[];
  boxRef: (el: HTMLDivElement | null) => void;
  nodeRefs: React.MutableRefObject<Map<string, HTMLDivElement>>;
}

function DeptGroup({ deptKey: key, employees, boxRef, nodeRefs }: DeptGroupProps) {
  const cfg = getDeptConfig(key);

  // Build mini-tree within this department using reports_to_id
  const deptEmpMap = useMemo(() => {
    const m = new Map<string, OrgEmployee>();
    employees.forEach((e) => m.set(e.id, e));
    return m;
  }, [employees]);

  // Find roots within this dept (reports_to_id is null or points outside this dept)
  const internalRoots = useMemo(
    () => employees.filter((e) => !e.reports_to_id || !deptEmpMap.has(e.reports_to_id)),
    [employees, deptEmpMap]
  );

  // Check if any employee has internal reports_to
  const hasInternalHierarchy = useMemo(
    () => employees.some((e) => e.reports_to_id && deptEmpMap.has(e.reports_to_id)),
    [employees, deptEmpMap]
  );

  function renderEmpNode(emp: OrgEmployee, depth: number): React.ReactNode {
    const children = employees.filter(
      (e) => e.reports_to_id === emp.id
    );
    return (
      <div key={emp.id} className="flex flex-col items-center">
        <EmpCard
          emp={emp}
          lead={key === "management"}
          nodeRef={(el) => {
            if (el) nodeRefs.current.set(emp.id, el);
            else nodeRefs.current.delete(emp.id);
          }}
        />
        {children.length > 0 && (
          // Reports wrap onto a new line rather than overflowing: a non-wrapping row was wider
          // than the department box, which clipped the cards (names cut off at both edges).
          <div className="mt-3 flex max-w-full flex-wrap items-start justify-center gap-3">
            {children.map((c) => (
              <div key={c.id} className="flex flex-col items-center">
                <div className="mb-1 h-3 w-px bg-line-strong" />
                {renderEmpNode(c, depth + 1)}
              </div>
            ))}
          </div>
        )}
      </div>
    );
  }

  return (
    <div ref={boxRef} className="overflow-hidden rounded-lg border border-line bg-surface shadow-card">
      <div className={cn("flex items-center justify-between border-b border-line bg-surface-2 px-4 py-2",
        key === "management" && "shadow-[inset_3px_0_0_rgb(var(--brand))]")}>
        <span className="text-[13px] font-semibold text-fg">{cfg.label}</span>
        <span className="text-xs text-fg-muted tabular">
          {employees.length} member{employees.length !== 1 ? "s" : ""}
        </span>
      </div>

      <div className="p-3">
        {employees.length === 0 ? (
          <p className="py-2 text-center text-xs italic text-fg-subtle">No members</p>
        ) : hasInternalHierarchy ? (
          <div className="flex flex-wrap justify-center gap-3">
            {internalRoots.map((root) => renderEmpNode(root, 0))}
          </div>
        ) : (
          <div className="flex flex-wrap gap-2">
            {employees.map((emp) => (
              <EmpCard
                key={emp.id}
                emp={emp}
                lead={key === "management"}
                nodeRef={(el) => {
                  if (el) nodeRefs.current.set(emp.id, el);
                  else nodeRefs.current.delete(emp.id);
                }}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

// Manage Links Modal

interface ManageLinksModalProps {
  links: CoordLink[];
  onAdd: (from: string, to: string, notes: string) => Promise<void>;
  onDelete: (id: string) => Promise<void>;
  onClose: () => void;
}

function ManageLinksModal({ links, onAdd, onDelete, onClose }: ManageLinksModalProps) {
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [notes, setNotes] = useState("");
  const [adding, setAdding] = useState(false);
  const [err, setErr] = useState("");

  const handleAdd = async () => {
    if (!from || !to) { setErr("Select both departments."); return; }
    if (from === to) { setErr("Departments must be different."); return; }
    setErr(""); setAdding(true);
    try {
      await onAdd(from, to, notes);
      setFrom(""); setTo(""); setNotes("");
    } catch (e: unknown) {
      const msg = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      setErr(msg || "Failed to add link.");
    } finally { setAdding(false); }
  };

  const deptOptions = CANONICAL_DEPT_LABELS.map((d) => ({ value: d, label: d }));

  return (
    <Dialog open onOpenChange={(o) => { if (!o) onClose(); }} title="Manage coordinative links">
      <h3 className="mb-2 text-sm font-semibold text-fg">Existing links</h3>
      {links.length === 0 ? (
        <p className="mb-4 text-sm text-fg-muted">No coordinative links yet.</p>
      ) : (
        <div className="mb-5 space-y-2">
          {links.map((l) => (
            <div key={l.id} className="flex items-center justify-between gap-2 rounded-lg bg-surface-2 px-3 py-2">
              <div className="flex min-w-0 flex-wrap items-center gap-2 text-sm">
                <Pill>{l.from_department}</Pill>
                <ArrowLeftRight className="size-3.5 text-fg-subtle" aria-label="coordinates with" />
                <Pill>{l.to_department}</Pill>
                {l.notes && <span className="text-xs text-fg-muted">{l.notes}</span>}
              </div>
              <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete link ${l.from_department} to ${l.to_department}`} onClick={() => onDelete(l.id)} />
            </div>
          ))}
        </div>
      )}

      <h3 className="mb-2 text-sm font-semibold text-fg">Add a link</h3>
      <div className="space-y-3">
        <FieldGrid cols={2}>
          <Field label="From department">
            <Select value={from} onChange={(e) => setFrom(e.target.value)} placeholder="Select..." options={deptOptions} />
          </Field>
          <Field label="To department">
            <Select value={to} onChange={(e) => setTo(e.target.value)} placeholder="Select..." options={deptOptions} />
          </Field>
        </FieldGrid>
        <Field label="Notes (optional)">
          <Input type="text" value={notes} onChange={(e) => setNotes(e.target.value)} placeholder="e.g. shared project delivery" />
        </Field>
        {err && (
          <div role="alert" className="flex items-center gap-1.5 text-xs text-tone-red">
            <AlertCircle className="size-3.5" aria-hidden /> {err}
          </div>
        )}
        <Button variant="primary" onClick={handleAdd} loading={adding}>
          {!adding && <Plus aria-hidden />}
          {adding ? "Adding..." : "Add link"}
        </Button>
      </div>
    </Dialog>
  );
}

// Main Page

export default function OrgChartPage() {
  const { data: session } = useSession();
  const isSuperadmin = (session?.user as { is_superadmin?: boolean } | undefined)?.is_superadmin ?? false;

  const [employees, setEmployees] = useState<OrgEmployee[]>([]);
  const [coordLinks, setCoordLinks] = useState<CoordLink[]>([]);
  const [loading, setLoading] = useState(true);
  const [svgLines, setSvgLines] = useState<SvgLine[]>([]);
  const [svgDims, setSvgDims] = useState({ w: 0, h: 0 });
  const [showModal, setShowModal] = useState(false);

  const containerRef = useRef<HTMLDivElement>(null);
  const nodeRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  const deptBoxRefs = useRef<Map<string, HTMLDivElement>>(new Map());

  // Group employees by dept key
  const deptGroups = useMemo(() => {
    const groups = new Map<string, OrgEmployee[]>();
    DEPT_GROUPS.forEach((g) => groups.set(g.key, []));
    employees.forEach((e) => {
      const k = deptKey(e.department);
      if (!groups.has(k)) groups.set(k, []);
      groups.get(k)!.push(e);
    });
    // Remove empty groups (except show all defined ones that have members)
    return groups;
  }, [employees]);

  const nonEmptyDepts = useMemo(
    () => DEPT_GROUPS.filter((g) => (deptGroups.get(g.key)?.length ?? 0) > 0),
    [deptGroups]
  );

  const managementDept = useMemo(
    () => nonEmptyDepts.find((d) => d.key === "management"),
    [nonEmptyDepts]
  );
  const childDepts = useMemo(
    () => nonEmptyDepts.filter((d) => d.key !== "management"),
    [nonEmptyDepts]
  );

  useEffect(() => {
    setLoading(true);
    orgChartApi
      .get()
      .then((res) => {
        setEmployees(res.data.employees ?? []);
        setCoordLinks(res.data.coordinative_links ?? []);
      })
      .finally(() => setLoading(false));
  }, []);

  const recomputeLines = useCallback(() => {
    const container = containerRef.current;
    if (!container) return;

    const cRect = container.getBoundingClientRect();
    const scrollL = container.scrollLeft;
    const scrollT = container.scrollTop;
    const lines: SvgLine[] = [];

    function toLocal(rect: DOMRect, cx: number, cy: number) {
      return {
        x: rect.left + cx - cRect.left + scrollL,
        y: rect.top + cy - cRect.top + scrollT,
      };
    }

    // Lines from the management box bottom to each child dept box top
    const mgmtEl = deptBoxRefs.current.get("management");
    if (mgmtEl) {
      const mRect = mgmtEl.getBoundingClientRect();
      const mBottom = toLocal(mRect, mRect.width / 2, mRect.height);

      childDepts.forEach((d) => {
        const childEl = deptBoxRefs.current.get(d.key);
        if (!childEl) return;
        const cRect2 = childEl.getBoundingClientRect();
        const cTop = toLocal(cRect2, cRect2.width / 2, 0);
        lines.push({ x1: mBottom.x, y1: mBottom.y, x2: cTop.x, y2: cTop.y, type: "report" });
      });
    }

    // Coordinative lines between dept boxes
    for (const link of coordLinks) {
      const fromKey = deptKey(link.from_department);
      const toKey = deptKey(link.to_department);
      const fromEl = deptBoxRefs.current.get(fromKey);
      const toEl = deptBoxRefs.current.get(toKey);
      if (!fromEl || !toEl) continue;
      const fRect = fromEl.getBoundingClientRect();
      const tRect = toEl.getBoundingClientRect();
      const f = toLocal(fRect, fRect.width / 2, fRect.height / 2);
      const t = toLocal(tRect, tRect.width / 2, tRect.height / 2);
      lines.push({ x1: f.x, y1: f.y, x2: t.x, y2: t.y, type: "coord" });
    }

    setSvgDims({ w: container.scrollWidth, h: container.scrollHeight });
    setSvgLines(lines);
  }, [coordLinks, childDepts]);

  useLayoutEffect(() => {
    recomputeLines();
  }, [recomputeLines]);

  useEffect(() => {
    window.addEventListener("resize", recomputeLines);
    return () => window.removeEventListener("resize", recomputeLines);
  }, [recomputeLines]);

  const handleAddLink = async (from: string, to: string, notes: string) => {
    await orgChartApi.addCoordLink({ from_department: from, to_department: to, notes: notes || undefined });
    const res = await orgChartApi.get();
    setCoordLinks(res.data.coordinative_links ?? []);
  };

  const handleDeleteLink = async (id: string) => {
    await orgChartApi.deleteCoordLink(id);
    setCoordLinks((prev) => prev.filter((l) => l.id !== id));
  };

  const legend = (
    <>
      <span className="flex items-center gap-1.5">
        <svg width="24" height="10" aria-hidden><line x1="0" y1="5" x2="24" y2="5" stroke="rgb(var(--line-strong))" strokeWidth="1.5" /></svg>
        Reporting
      </span>
      <span className="flex items-center gap-1.5">
        <svg width="24" height="10" aria-hidden><line x1="0" y1="5" x2="24" y2="5" stroke="rgb(var(--brand))" strokeWidth="1.5" strokeDasharray="5 3" /></svg>
        Coordinative
      </span>
    </>
  );

  return (
    <div className="flex h-full flex-col">
      <PageHeader title="Org chart"
        summary={`${employees.length} active employee${employees.length !== 1 ? "s" : ""}, ${nonEmptyDepts.length} department${nonEmptyDepts.length !== 1 ? "s" : ""}`}
        actions={<>
          <div className="mr-2 hidden items-center gap-4 text-xs text-fg-muted sm:flex">{legend}</div>
          <IconButton variant="ghost" icon={RefreshCw} label="Refresh lines" onClick={recomputeLines} />
          {isSuperadmin && (
            <Button variant="primary" onClick={() => setShowModal(true)}>
              <GitBranch aria-hidden /><span className="max-sm:sr-only">Manage links</span>
            </Button>
          )}
        </>} />
      <div className="mb-2 flex items-center gap-4 text-xs text-fg-muted sm:hidden">{legend}</div>

      <div
        ref={containerRef}
        className="relative flex-1 overflow-auto rounded-lg border border-line bg-canvas"
        onScroll={recomputeLines}
      >
        {loading ? (
          <div className="flex h-64 flex-col items-center justify-center gap-3 text-fg-muted" aria-live="polite">
            <div className="size-8 animate-spin rounded-full border-2 border-brand border-t-transparent" aria-hidden />
            <span className="text-sm">Loading org chart...</span>
          </div>
        ) : employees.length === 0 ? (
          <EmptyState icon={Users} title="No employees found" description="Add employees in HR, Employees to populate the org chart." />
        ) : (
          <>
            <svg
              className="pointer-events-none absolute left-0 top-0"
              width={svgDims.w}
              height={svgDims.h}
              style={{ minWidth: "100%", minHeight: "100%" }}
              aria-hidden
            >
              {svgLines.map((line, i) =>
                line.type === "report" ? (
                  <path
                    key={i}
                    d={`M ${line.x1} ${line.y1} C ${line.x1} ${(line.y1 + line.y2) / 2} ${line.x2} ${(line.y1 + line.y2) / 2} ${line.x2} ${line.y2}`}
                    stroke="rgb(var(--line-strong))"
                    strokeWidth={1.5}
                    fill="none"
                  />
                ) : (
                  <path
                    key={i}
                    d={`M ${line.x1} ${line.y1} C ${line.x1} ${line.y1 - 60} ${line.x2} ${line.y2 - 60} ${line.x2} ${line.y2}`}
                    stroke="rgb(var(--brand))"
                    strokeWidth={1.5}
                    strokeDasharray="7 4"
                    fill="none"
                    opacity={0.65}
                  />
                )
              )}
            </svg>

            {/* Phones: departments stack in one column; desktop lays them out side by side */}
            <div className="relative z-10 p-3 md:min-w-max md:p-8">
              {managementDept && (
                <div className="mb-6 flex justify-center md:mb-10">
                  <div className="w-full max-w-2xl">
                    <DeptGroup
                      deptKey="management"
                      employees={deptGroups.get("management") ?? []}
                      boxRef={(el) => {
                        if (el) deptBoxRefs.current.set("management", el);
                        else deptBoxRefs.current.delete("management");
                      }}
                      nodeRefs={nodeRefs}
                    />
                  </div>
                </div>
              )}

              {childDepts.length > 0 && (
                <div className="flex flex-col gap-4 md:flex-row md:flex-wrap md:justify-center md:gap-5">
                  {childDepts.map((dept) => (
                    <div key={dept.key} className="w-full flex-shrink-0 md:w-[280px]">
                      <DeptGroup
                        deptKey={dept.key}
                        employees={deptGroups.get(dept.key) ?? []}
                        boxRef={(el) => {
                          if (el) deptBoxRefs.current.set(dept.key, el);
                          else deptBoxRefs.current.delete(dept.key);
                        }}
                        nodeRefs={nodeRefs}
                      />
                    </div>
                  ))}
                </div>
              )}
            </div>
          </>
        )}
      </div>

      {showModal && (
        <ManageLinksModal
          links={coordLinks}
          onAdd={handleAddLink}
          onDelete={handleDeleteLink}
          onClose={() => setShowModal(false)}
        />
      )}
    </div>
  );
}
