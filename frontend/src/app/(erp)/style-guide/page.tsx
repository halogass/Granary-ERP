"use client";

import { useMemo, useState } from "react";
import { Download, Inbox, MoreHorizontal, Pencil, Plus, Search, Trash2 } from "lucide-react";
import {
  Button, IconButton, Input, Textarea, Select, Checkbox, Switch, Field, FieldGrid, Combobox,
  Card, CardHeader, CardBody, CardFooter, KeyValues, Pill, StatusPill, CountBadge, STATUS_TONES,
  Tabs, TabsList, TabsTrigger, TabsContent, SavedViewTabs, useSavedViews, FilterChips, DataTable, Pager,
  PageHeader, ListFrame, Toolbar, ToolbarSpacer, Dialog, Sheet, DropdownMenu, MenuItem, MenuSeparator, Tooltip,
  toast, EmptyState, Skeleton, Kbd, Stat, StatGroup, type ColumnDef, type StatusFamily, type Tone,
} from "@/components/ui";
import { cn } from "@/lib/utils";

/* Sample data: the deals from the chosen design (B4). */
interface Deal { id: string; name: string; org: string; stage: string; value: number; next: string; touched: string; owner: string }
const DEALS: Deal[] = [
  { id: "d1", name: "Warehouse ERP rollout", org: "Acme Ltd", stage: "proposal", value: 42_000, next: "Send proposal", touched: "2 h", owner: "Owner" },
  { id: "d2", name: "Laptops for 3 sites", org: "Globex Corp", stage: "discovery", value: 18_000, next: "Meeting 14:00", touched: "1 d", owner: "Owner" },
  { id: "d3", name: "Support plan renewal", org: "Northwind Traders", stage: "contract", value: 31_000, next: "Revised quote", touched: "3 d", owner: "Sales" },
  { id: "d4", name: "Monitor pilot", org: "Contoso", stage: "qualifying", value: 9_500, next: "Call back", touched: "4 d", owner: "Sales" },
  { id: "d5", name: "Onboarding package", org: "Initech", stage: "new", value: 4_000, next: "Discovery", touched: "5 d", owner: "Owner" },
  { id: "d6", name: "Consulting hours", org: "Umbrella Corp", stage: "qualifying", value: 12_000, next: "Site survey", touched: "6 d", owner: "Sales" },
  { id: "d7", name: "Software licences", org: "Stark Industries", stage: "proposal", value: 21_000, next: "Proposal review", touched: "1 w", owner: "Owner" },
  { id: "d8", name: "25 seat licence", org: "Wayne Enterprises", stage: "won", value: 7_500, next: "Handover", touched: "2 w", owner: "Sales" },
];

const usd = (n: number) => `USD ${new Intl.NumberFormat("en-US", { minimumFractionDigits: 2 }).format(n)}`;

type DealFilters = { stage?: string; mine?: boolean };

const TOKENS: { name: string; cls: string }[] = [
  { name: "canvas", cls: "bg-canvas" }, { name: "surface", cls: "bg-surface" }, { name: "surface-2", cls: "bg-surface-2" },
  { name: "line", cls: "bg-line" }, { name: "fg", cls: "bg-fg" }, { name: "fg-muted", cls: "bg-fg-muted" },
  { name: "brand", cls: "bg-brand" }, { name: "nav-active", cls: "bg-nav-active" },
];
const TONES: Tone[] = ["neutral", "blue", "violet", "orange", "red"];

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="space-y-3">
      <h2 className="text-sm font-semibold text-fg">{title}</h2>
      {children}
    </section>
  );
}

function DealList() {
  const builtins = useMemo(() => [
    { id: "open", label: "All open", filters: {} as DealFilters },
    { id: "mine", label: "Mine", filters: { mine: true } as DealFilters },
    { id: "proposal", label: "At proposal", filters: { stage: "proposal" } as DealFilters },
  ], []);
  const views = useSavedViews<DealFilters>("style-guide.deals", builtins);
  const [chips, setChips] = useState<Record<string, boolean>>({});
  const filters: DealFilters = { ...views.active?.filters, ...(chips.mine ? { mine: true } : {}) };
  const rows = DEALS.filter((d) => (!filters.mine || d.owner === "Owner") && (!filters.stage || d.stage === filters.stage));
  const total = rows.reduce((n, d) => n + d.value, 0);

  const columns = useMemo<ColumnDef<Deal>[]>(() => [
    { accessorKey: "name", header: "Deal", cell: (c) => <span className="font-medium">{c.getValue<string>()}</span> },
    { accessorKey: "org", header: "Organization" },
    { accessorKey: "stage", header: "Stage", cell: (c) => <StatusPill family="deal" status={c.getValue<string>()} /> },
    { accessorKey: "value", header: "Value", meta: { numeric: true }, cell: (c) => usd(c.getValue<number>()) },
    { accessorKey: "next", header: "Next", enableSorting: false, meta: { hideOnMobile: true } },
    { accessorKey: "touched", header: "Last touch", enableSorting: false, meta: { numeric: true, hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string>()}</span> },
  ], []);

  return (
    <ListFrame>
      <PageHeader flush title="Deals" summary={`${rows.length} open, ${usd(total)}`}
        actions={<><Input type="search" leading={Search} placeholder="Filter deals" aria-label="Filter deals" className="w-48" />
          <Button variant="primary"><Plus aria-hidden /> New deal</Button></>} />
      <SavedViewTabs state={views} currentFilters={filters} counts={{ proposal: 2 }} alertIds={["proposal"]} />
      <FilterChips
        chips={[{ id: "mine", label: "Owner: me", active: !!chips.mine }, { id: "overdue", label: "Next step overdue", active: !!chips.overdue, count: 3 }]}
        onToggle={(id) => setChips((c) => ({ ...c, [id]: !c[id] }))} onClear={() => setChips({})} />
      <DataTable label="Deals" columns={columns} data={rows} getRowId={(d) => d.id} maxHeight="none"
        renderDetails={(d) => (
          <div className="flex flex-wrap gap-6">
            <KeyValues className="min-w-[260px] flex-1" items={[
              { label: "Value", value: usd(d.value), numeric: true }, { label: "Owner", value: d.owner },
              { label: "Next step", value: d.next }, { label: "Last touch", value: d.touched },
            ]} />
            <div className="flex min-w-[260px] flex-1 flex-col items-start gap-2">
              <p className="text-[13px] text-fg-muted">Draft proposal with figures is ready to review.</p>
              <Button size="sm" variant="secondary">Open deal</Button>
            </div>
          </div>
        )}
        footer={<Pager page={1} pageSize={20} total={rows.length} onPage={() => undefined} />} />
    </ListFrame>
  );
}

function Gallery() {
  const [dialog, setDialog] = useState(false);
  const [sheet, setSheet] = useState(false);
  const [on, setOn] = useState(true);
  const [org, setOrg] = useState("");
  return (
    <div className="space-y-10">
      <Section title="Tokens">
        <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
          {TOKENS.map((t) => (
            <div key={t.name} className="flex items-center gap-2 text-[13px] text-fg">
              <span className={cn("size-8 rounded-lg border border-line", t.cls)} /> {t.name}
            </div>
          ))}
        </div>
        <div className="flex flex-wrap gap-2">{TONES.map((t) => <Pill key={t} tone={t}>{t}</Pill>)}</div>
      </Section>

      <Section title="Type">
        <div className="space-y-1">
          <p className="text-lg font-semibold text-fg">Page title, 18 / 600</p>
          <p className="text-sm font-semibold text-fg">Section title, 14 / 600</p>
          <p className="text-sm text-fg">Body and table text, 14 / 400. Amounts line up: <span className="tabular">USD 1,250.00</span></p>
          <p className="text-[13px] text-fg-muted">Muted, 13: hints, summaries, metadata.</p>
          <p className="font-mono text-xs text-fg-subtle">nary.reply_wait_seconds (IBM Plex Mono for keys and ids)</p>
        </div>
      </Section>

      <Section title="Buttons">
        <Toolbar>
          <Button variant="primary">Save</Button>
          <Button>Cancel</Button>
          <Button variant="ghost">Skip</Button>
          <Button variant="danger"><Trash2 aria-hidden /> Delete</Button>
          <Button variant="primary" loading>Sending</Button>
          <Button size="sm">Small</Button>
          <Button size="lg" variant="primary">Large</Button>
          <IconButton label="Edit" icon={Pencil} />
          <IconButton label="Download" icon={Download} variant="secondary" />
          <IconButton label="Inbox" icon={Inbox} badge={4} />
        </Toolbar>
      </Section>

      <Section title="Form controls">
        <Card>
          <CardHeader title="Billing" description="How this customer is invoiced." />
          <CardBody>
            <FieldGrid cols={2}>
              <Field label="Legal name" required hint="As on the tax registration."><Input placeholder="Acme Ltd" /></Field>
              <Field label="Payment terms" hint="Days after the invoice date.">
                <Select options={[{ value: "14", label: "14 days" }, { value: "30", label: "30 days" }]} defaultValue="30" />
              </Field>
              <Field label="Credit limit" error="Enter an amount above zero."><Input numeric trailing="USD" defaultValue="0" /></Field>
              <Field label="Organization" hint="Searches as you type.">
                <Combobox value={org} onChange={(id) => setOrg(id)} placeholder="Choose an organization"
                  source={{ fetch: async (q) => DEALS.filter((d) => d.org.toLowerCase().includes(q.toLowerCase())).map((d) => ({ id: d.id, label: d.org })) }} />
              </Field>
              <Field label="Notes" className="md:col-span-2"><Textarea placeholder="Anything finance should know" /></Field>
            </FieldGrid>
            <div className="mt-4 flex flex-wrap gap-6">
              <Checkbox label="Send invoices by email" hint="To the billing contact." defaultChecked />
              <Switch label="Reminders before the due date" checked={on} onCheckedChange={setOn} />
            </div>
          </CardBody>
          <CardFooter><Button>Discard</Button><Button variant="primary">Save changes</Button></CardFooter>
        </Card>
      </Section>

      <Section title="Status pills (one map per family)">
        <div className="space-y-2">
          {(["deal", "invoice", "quotation", "approval"] as StatusFamily[]).map((f) => (
            <div key={f} className="flex flex-wrap items-center gap-1.5">
              <span className="w-20 text-xs text-fg-muted">{f}</span>
              {Object.keys(STATUS_TONES[f]).map((s) => <StatusPill key={s} family={f} status={s} />)}
            </div>
          ))}
          <div className="flex items-center gap-2 text-[13px] text-fg">Needs you <CountBadge count={5} /> <CountBadge count={12} tone="muted" /></div>
        </div>
      </Section>

      <Section title="List page: header, saved views, chips, table with rows that unfold">
        <DealList />
      </Section>

      <Section title="Tabs">
        <Tabs defaultValue="overview">
          <TabsList><TabsTrigger value="overview">Overview</TabsTrigger><TabsTrigger value="activity">Activity</TabsTrigger><TabsTrigger value="files">Files</TabsTrigger></TabsList>
          <TabsContent value="overview"><p className="text-sm text-fg-muted">Tab content.</p></TabsContent>
          <TabsContent value="activity"><p className="text-sm text-fg-muted">Activity.</p></TabsContent>
          <TabsContent value="files"><p className="text-sm text-fg-muted">Files.</p></TabsContent>
        </Tabs>
      </Section>

      <Section title="Stats">
        <StatGroup>
          <Stat label="Open pipeline" value="USD 182,000" note="14 deals" />
          <Stat label="Overdue invoices" value="3" note="USD 8,600 past due" alert />
          <Stat label="Nary cards" value="5" note="waiting for you" alert />
          <Stat label="Paid this month" value="USD 41,200" note="+8% vs last month" />
        </StatGroup>
      </Section>

      <Section title="Overlays">
        <Toolbar>
          <Button onClick={() => setDialog(true)}>Open dialog</Button>
          <Button onClick={() => setSheet(true)}>Open sheet</Button>
          <DropdownMenu trigger={<IconButton label="More actions" icon={MoreHorizontal} variant="secondary" />}>
            <MenuItem icon={Pencil}>Edit</MenuItem>
            <MenuItem icon={Download}>Download PDF</MenuItem>
            <MenuSeparator />
            <MenuItem icon={Trash2} danger>Delete</MenuItem>
          </DropdownMenu>
          <Tooltip content="Shown on hover and focus"><Button variant="ghost">Tooltip</Button></Tooltip>
          <Button onClick={() => toast.success("Invoice sent")}>Toast</Button>
          <ToolbarSpacer />
          <span className="flex items-center gap-1 text-xs text-fg-muted"><Kbd>Ctrl</Kbd><Kbd>K</Kbd> opens the palette</span>
        </Toolbar>
        <Dialog open={dialog} onOpenChange={setDialog} title="Send invoice INV-2026-0142" description="To finance@globex.example with the PDF attached."
          footer={<><Button onClick={() => setDialog(false)}>Cancel</Button><Button variant="primary" onClick={() => setDialog(false)}>Send</Button></>}>
          <Field label="Message"><Textarea defaultValue="Please find the invoice attached." /></Field>
        </Dialog>
        <Sheet open={sheet} onOpenChange={setSheet} title="Filters" footer={<Button variant="primary" onClick={() => setSheet(false)}>Apply</Button>}>
          <Field label="Owner"><Select options={[{ value: "", label: "Anyone" }, { value: "me", label: "Me" }]} /></Field>
        </Sheet>
      </Section>

      <Section title="Empty and loading">
        <div className="grid gap-3 md:grid-cols-2">
          <Card><EmptyState icon={Inbox} title="No invoices yet" description="Create one from an accepted quotation, or start a blank invoice." action={<Button variant="primary">New invoice</Button>} /></Card>
          <Card className="space-y-2 p-4"><Skeleton className="h-4 w-1/3" /><Skeleton className="h-3.5 w-2/3" /><Skeleton className="h-3.5 w-1/2" /></Card>
        </div>
        <DataTable label="Loading example" columns={[{ accessorKey: "a", header: "Name" }, { accessorKey: "b", header: "Amount", meta: { numeric: true } }]}
          data={[] as { a: string; b: number }[]} loading maxHeight="none" className="rounded-lg border border-line" />
      </Section>
    </div>
  );
}

/** Every shared component in light and dark (internal page, not in the menu). */
export default function StyleGuidePage() {
  const [show, setShow] = useState<"both" | "light" | "dark">("both");
  return (
    <div className="mx-auto max-w-6xl space-y-4">
      <PageHeader title="Style guide" summary="The shared components, as phase 2 pages use them."
        actions={
          <div role="group" aria-label="Themes shown" className="flex rounded-lg border border-line bg-surface p-0.5">
            {(["both", "light", "dark"] as const).map((v) => (
              <button key={v} type="button" aria-pressed={show === v} onClick={() => setShow(v)}
                className={cn("h-8 rounded-md px-3 text-[13px] capitalize", show === v ? "bg-surface-2 font-semibold text-fg" : "text-fg-muted hover:text-fg")}>
                {v}
              </button>
            ))}
          </div>
        } />
      {(show === "both" || show === "light") && (
        <div className="light rounded-lg border border-line bg-canvas p-4 text-fg md:p-6"><Gallery /></div>
      )}
      {(show === "both" || show === "dark") && (
        <div className="dark rounded-lg border border-line bg-canvas p-4 text-fg md:p-6"><Gallery /></div>
      )}
    </div>
  );
}
