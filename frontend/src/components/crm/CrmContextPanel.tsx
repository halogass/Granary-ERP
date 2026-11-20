"use client";

import { useCurrencySettings } from "@/hooks/useCurrencySettings";
import { useCallback, useEffect, useState } from "react";
import Link from "next/link";
import {
  User, Building2, Plus, Pencil, Trash2, X, Phone, Mail,
  MessageCircle, StickyNote, Search, ChevronRight, Briefcase, Target, FileText,
} from "lucide-react";
import { crmApi, crmInboxApi, salesApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { formatCurrency } from "@/lib/currency";
import {
  Button, Checkbox, Dialog, Field, FieldGrid, IconButton, Input, Pill, Select, Skeleton, StatusPill, Textarea, statusLabel,
} from "@/components/ui";

import { useCrmStages } from "@/hooks/useCrmStages";
import { REASON_STAGES, canMoveLead, stageStyle } from "@/lib/crmStages";

// Personal/free email providers: never auto-suggest an org from these domains.
const FREE_EMAIL_DOMAINS = new Set([
  "gmail.com", "googlemail.com", "yahoo.com", "yahoo.co.jp", "outlook.com", "hotmail.com",
  "live.com", "icloud.com", "me.com", "aol.com", "proton.me", "protonmail.com",
  "gmx.com", "mail.com", "yandex.com", "zoho.com",
]);

function domainFromEmail(email?: string | null): string | null {
  if (!email || !email.includes("@")) return null;
  return email.split("@").pop()!.trim().toLowerCase() || null;
}

// "acme.co.jp" -> "Acme"; "sub.acme.com" -> "Acme"
function orgNameFromDomain(domain: string): string {
  const label = domain.replace(/^www\./, "").split(".")[0] || domain;
  return label.charAt(0).toUpperCase() + label.slice(1);
}

interface Contact { id: string; first_name: string; last_name?: string | null; full_name?: string; position?: string | null; email?: string | null; phone?: string | null; whatsapp?: string | null; organization_id?: string | null; }
interface Org { id: string; name: string; industry?: string | null; website?: string | null; city?: string | null; }
interface Lead { id: string; title?: string | null; status: string; source?: string | null; estimated_value?: number | null; currency?: string | null; description?: string | null; }
interface Deal { id: string; title: string; stage: string; value?: number | null; currency?: string | null; probability?: number | null; expected_close_date?: string | null; notes?: string | null; }
interface Quotation { id: string; number?: string | null; full_number?: string | null; status: string; total?: number | null; currency?: string | null; issue_date?: string | null; }
interface Activity { id: string; activity_type: string; direction?: string | null; subject?: string | null; content?: string | null; created_at?: string | null; created_by_name?: string | null; }

export interface CrmContextPanelProps {
  channel: "whatsapp" | "email";
  /** The chat's real number (digits); null when WhatsApp hides it. */
  phone?: string | null;
  /** The WhatsApp chat id: links a chat whose number is hidden ("...@lid"). */
  chatId?: string | null;
  threadId?: string | null;
  contactId?: string | null;
  orgId?: string | null;
  displayName?: string | null;
  email?: string | null;
  /** Refresh the parent conversation/thread list after a link/create. */
  onChange?: () => void;
  /** Override how a contact link is persisted (e.g. personal email inbox links a message,
   *  not a CRM thread). When provided, it is used instead of the default channel persistence. */
  onLinkContact?: (contactId: string) => Promise<void>;
  /** Mobile drawer open state + close handler (ignored on desktop unless `collapsible`). */
  open?: boolean;
  onClose?: () => void;
  /** When true, the panel is collapsible on desktop too (hidden when !open), not a permanent column. */
  collapsible?: boolean;
}

type ModalState =
  | { type: "contact"; editing: Contact | null }
  | { type: "lead"; editing: Lead | null }
  | { type: "deal"; editing: Deal | null }
  | { type: "note" }
  | null;

function fmtDate(iso?: string | null) {
  if (!iso) return "";
  return new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });
}

export default function CrmContextPanel(props: CrmContextPanelProps) {
  const { channel, phone, chatId, threadId, displayName, email, onChange, open, onClose, collapsible } = props;

  const [contactId, setContactId] = useState<string | null>(props.contactId ?? null);
  const [contact, setContact] = useState<Contact | null>(null);
  const [org, setOrg] = useState<Org | null>(null);
  const [leads, setLeads] = useState<Lead[]>([]);
  const [deals, setDeals] = useState<Deal[]>([]);
  const [quotations, setQuotations] = useState<Quotation[]>([]);
  const [activities, setActivities] = useState<Activity[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  // Link-existing search (unmatched state)
  const [linkSearch, setLinkSearch] = useState("");
  const [linkResults, setLinkResults] = useState<Contact[]>([]);

  const [modal, setModal] = useState<ModalState>(null);

  // Reset resolved contact whenever the conversation identity changes.
  useEffect(() => {
    setContactId(props.contactId ?? null);
    setError("");
  }, [props.contactId, phone, chatId, threadId]);

  const load = useCallback(async (id: string) => {
    setLoading(true); setError("");
    try {
      const cRes = await crmApi.getContact(id);
      const c: Contact = cRes.data;
      setContact(c);
      const orgId = c.organization_id ?? props.orgId ?? null;
      const [oRes, lRes, dRes, qRes, aRes] = await Promise.all([
        orgId ? crmApi.getOrg(orgId).catch(() => null) : Promise.resolve(null),
        crmApi.listLeads({ contact_id: id, organization_id: orgId ?? undefined, limit: 50 }).catch(() => null),
        crmApi.listDeals({ contact_id: id, organization_id: orgId ?? undefined, limit: 50 }).catch(() => null),
        salesApi.listQuotations({ contact_id: id, organization_id: orgId ?? undefined, limit: 50 }).catch(() => null),
        crmApi.listActivities({ entity_type: "contact", entity_id: id, limit: 30 }).catch(() => null),
      ]);
      setOrg(oRes?.data ?? null);
      setLeads(lRes?.data ?? []);
      setDeals(dRes?.data ?? []);
      setQuotations(qRes?.data?.items ?? qRes?.data ?? []);
      setActivities(aRes?.data ?? []);
    } catch (err) {
      setError(extractApiError(err, "Failed to load CRM context."));
    } finally {
      setLoading(false);
    }
  }, [props.orgId]);

  useEffect(() => {
    if (contactId) load(contactId);
    else { setContact(null); setOrg(null); setLeads([]); setDeals([]); setQuotations([]); setActivities([]); }
  }, [contactId, load]);

  // Debounced contact search for the unmatched "link existing" box.
  useEffect(() => {
    if (contactId || !linkSearch.trim()) { setLinkResults([]); return; }
    const t = setTimeout(async () => {
      try {
        const r = await crmApi.listContacts({ q: linkSearch, limit: 8 });
        setLinkResults(r.data.items ?? r.data ?? []);
      } catch { setLinkResults([]); }
    }, 300);
    return () => clearTimeout(t);
  }, [linkSearch, contactId]);

  async function persistLink(newContactId: string) {
    if (props.onLinkContact) {
      await props.onLinkContact(newContactId);
    } else if (channel === "whatsapp" && (phone || chatId)) {
      await crmInboxApi.waLink({ phone: phone || null, chat_id: chatId || null, contact_id: newContactId });
    } else if (channel === "email" && threadId) {
      await crmInboxApi.updateThread(threadId, { linked_contact_id: newContactId });
    }
    setContactId(newContactId);
    onChange?.();
  }

  async function linkExisting(c: Contact) {
    try { await persistLink(c.id); } catch (err) { setError(extractApiError(err, "Failed to link contact.")); }
  }

  const refresh = () => { if (contactId) load(contactId); };

    const matched = !!contactId;

  // `collapsible`: when set, the panel is fully toggle-controlled on desktop too (hidden when !open),
  // instead of being a permanently-visible right column.
  const visibility = collapsible
    ? (open ? "translate-x-0 lg:translate-x-0" : "translate-x-full lg:hidden")
    : `lg:translate-x-0 ${open ? "translate-x-0" : "translate-x-full lg:translate-x-0"}`;

  return (
    <>
      {/* Mobile backdrop */}
      {open && <div className="fixed inset-0 z-40 bg-black/40 lg:hidden" onClick={onClose} />}
      <aside aria-label="CRM context"
        className={`fixed inset-y-0 right-0 z-50 flex w-[340px] max-w-[88vw] shrink-0 flex-col border-l border-line bg-surface
          transition-transform lg:static lg:z-auto ${visibility}`}
      >
        <header className="flex h-14 shrink-0 items-center justify-between border-b border-line px-4">
          <span className="text-sm font-semibold text-fg">CRM context</span>
          <IconButton icon={X} label="Close CRM context" onClick={onClose} className={collapsible ? "" : "lg:hidden"} />
        </header>

        <div className="flex-1 overflow-y-auto">
          {error && (
            <p role="alert" className="m-3 rounded-lg bg-tone-red-soft px-3 py-2 text-xs text-tone-red">{error}</p>
          )}

          {loading && (
            <div className="space-y-3 p-3" aria-busy>
              <Skeleton className="h-24" />
              <Skeleton className="h-16" />
              <Skeleton className="h-16" />
            </div>
          )}

          {!loading && !matched && (
            <UnmatchedState
              channel={channel} phone={phone} email={email} displayName={displayName}
              linkSearch={linkSearch} setLinkSearch={setLinkSearch} linkResults={linkResults}
              onLink={linkExisting} onCreate={() => setModal({ type: "contact", editing: null })}
            />
          )}

          {!loading && matched && contact && (
            <div className="space-y-3 p-3">
              <ContactCard contact={contact} org={org} onEdit={() => setModal({ type: "contact", editing: contact })} />
              {org && <OrgCard org={org} />}
              <LeadsSection leads={leads} onAdd={() => setModal({ type: "lead", editing: null })}
                onEdit={(l) => setModal({ type: "lead", editing: l })}
                onDelete={async (id) => { try { await crmApi.deleteLead(id); refresh(); } catch (e) { setError(extractApiError(e)); } }} />
              <DealsSection deals={deals} onAdd={() => setModal({ type: "deal", editing: null })}
                onEdit={(d) => setModal({ type: "deal", editing: d })}
                onDelete={async (id) => { try { await crmApi.deleteDeal(id); refresh(); } catch (e) { setError(extractApiError(e)); } }} />
              <QuotationsSection quotations={quotations} />
              <ActivitySection activities={activities} onLog={() => setModal({ type: "note" })} />
            </div>
          )}
        </div>
      </aside>

      {modal?.type === "contact" && (
        <ContactFormModal
          editing={modal.editing}
          prefill={{ displayName, phone: phone ? `+${phone.replace(/^\+/, "")}` : null, email, channel }}
          initialOrg={org ? { id: org.id, name: org.name } : null}
          onClose={() => setModal(null)}
          onSaved={async (savedId, isNew, savedOrgId) => {
            setModal(null);
            try {
              if (isNew) await persistLink(savedId);
              // Keep the email thread's org in sync with the contact's org.
              if (channel === "email" && threadId && savedOrgId) {
                await crmInboxApi.updateThread(threadId, { linked_org_id: savedOrgId });
              }
            } catch (e) { setError(extractApiError(e)); }
            onChange?.();
            refresh();
          }}
          onError={(m) => setError(m)}
        />
      )}
      {modal?.type === "lead" && contactId && (
        <LeadFormModal editing={modal.editing} contactId={contactId} orgId={contact?.organization_id ?? props.orgId ?? null}
          channel={channel} onClose={() => setModal(null)} onSaved={() => { setModal(null); refresh(); }} onError={(m) => setError(m)} />
      )}
      {modal?.type === "deal" && contactId && (
        <DealFormModal editing={modal.editing} contactId={contactId} orgId={contact?.organization_id ?? props.orgId ?? null}
          onClose={() => setModal(null)} onSaved={() => { setModal(null); refresh(); }} onError={(m) => setError(m)} />
      )}
      {modal?.type === "note" && contactId && (
        <NoteModal contactId={contactId} channel={channel} onClose={() => setModal(null)}
          onSaved={() => { setModal(null); refresh(); }} onError={(m) => setError(m)} />
      )}
    </>
  );
}

// Sections

const box = "rounded-lg border border-line bg-surface p-3";

function ContactCard({ contact, org, onEdit }: { contact: Contact; org: Org | null; onEdit: () => void }) {
  const name = `${contact.first_name} ${contact.last_name ?? ""}`.trim();
  return (
    <div className={box}>
      <div className="flex items-start gap-3">
        <div aria-hidden className="flex size-10 shrink-0 items-center justify-center rounded-full bg-tone-violet-soft text-sm font-semibold text-tone-violet">
          {name.slice(0, 2).toUpperCase()}
        </div>
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-semibold text-fg">{name}</p>
          {contact.position && <p className="truncate text-xs text-fg-muted">{contact.position}</p>}
          {org && <p className="truncate text-xs text-fg-muted">{org.name}</p>}
        </div>
        <IconButton size="sm" icon={Pencil} label="Edit contact" onClick={onEdit} />
      </div>
      <div className="mt-3 space-y-1.5 text-xs">
        {contact.phone && <Row icon={<Phone className="size-3.5" aria-hidden />} value={contact.phone} />}
        {contact.whatsapp && <Row icon={<MessageCircle className="size-3.5" aria-hidden />} value={contact.whatsapp} />}
        {contact.email && <Row icon={<Mail className="size-3.5" aria-hidden />} value={contact.email} />}
      </div>
    </div>
  );
}

function Row({ icon, value }: { icon: React.ReactNode; value: string }) {
  return <div className="flex items-center gap-2 text-fg"><span className="text-fg-subtle">{icon}</span><span className="truncate">{value}</span></div>;
}

function OrgCard({ org }: { org: Org }) {
  return (
    <Link href="/crm/organizations" className={`${box} block transition-colors hover:bg-surface-2`}>
      <div className="flex items-center gap-2">
        <Building2 className="size-4 text-fg-subtle" aria-hidden />
        <p className="flex-1 truncate text-sm font-semibold text-fg">{org.name}</p>
        <ChevronRight className="size-4 text-fg-subtle" aria-hidden />
      </div>
      <div className="mt-1 truncate text-xs text-fg-muted">
        {[org.industry, org.city].filter(Boolean).join(" · ") || (org.website ?? "Organization")}
      </div>
    </Link>
  );
}

function SectionHeader({ icon, title, count, onAdd, addLabel }: { icon: React.ReactNode; title: string; count: number; onAdd?: () => void; addLabel?: string }) {
  return (
    <div className="mb-2 flex items-center justify-between">
      <h3 className="flex items-center gap-1.5 text-xs font-semibold text-fg-muted">
        {icon}<span>{title}</span><span className="tabular text-fg-subtle">{count}</span>
      </h3>
      {onAdd && <IconButton size="sm" icon={Plus} label={addLabel ?? `Add to ${title.toLowerCase()}`} onClick={onAdd} />}
    </div>
  );
}

function LeadsSection({ leads, onAdd, onEdit, onDelete }: { leads: Lead[]; onAdd: () => void; onEdit: (l: Lead) => void; onDelete: (id: string) => void }) {
  const { reg } = useCrmStages();
  return (
    <div className={box}>
      <SectionHeader icon={<Target className="size-3.5" aria-hidden />} title="Leads" count={leads.length} onAdd={onAdd} addLabel="New lead" />
      {leads.length === 0 ? <p className="text-xs text-fg-muted">No leads yet.</p> : (
        <div className="space-y-1">
          {leads.map((l) => (
            <RecordRow key={l.id} title={l.title || "Untitled lead"}
              badge={<StatusPill family="lead" status={l.status} label={stageStyle(reg, l.status).label} />}
              value={l.estimated_value ? formatCurrency(l.estimated_value, l.currency || "USD") : null}
              onEdit={() => onEdit(l)} onDelete={() => onDelete(l.id)} />
          ))}
        </div>
      )}
    </div>
  );
}

function DealsSection({ deals, onAdd, onEdit, onDelete }: { deals: Deal[]; onAdd: () => void; onEdit: (d: Deal) => void; onDelete: (id: string) => void }) {
  const { reg } = useCrmStages();
  return (
    <div className={box}>
      <SectionHeader icon={<Briefcase className="size-3.5" aria-hidden />} title="Deals" count={deals.length} onAdd={onAdd} addLabel="New deal" />
      {deals.length === 0 ? <p className="text-xs text-fg-muted">No deals yet.</p> : (
        <div className="space-y-1">
          {deals.map((d) => (
            <RecordRow key={d.id} title={d.title}
              badge={<StatusPill family="deal" status={d.stage} label={stageStyle(reg, d.stage).label} />}
              value={d.value ? formatCurrency(d.value, d.currency || "USD") : null}
              onEdit={() => onEdit(d)} onDelete={() => onDelete(d.id)} />
          ))}
        </div>
      )}
    </div>
  );
}

function QuotationsSection({ quotations }: { quotations: Quotation[] }) {
  return (
    <div className={box}>
      <SectionHeader icon={<FileText className="size-3.5" aria-hidden />} title="Quotations" count={quotations.length} />
      {quotations.length === 0 ? <p className="text-xs text-fg-muted">No quotations yet.</p> : (
        <div className="space-y-1">
          {quotations.map((q) => (
            <Link key={q.id} href={`/sales/quotations/${q.id}`}
              className="group flex items-center gap-2 rounded-lg px-2 py-1.5 hover:bg-surface-2">
              <div className="min-w-0 flex-1">
                <p className="truncate font-mono text-xs text-fg">{q.full_number || q.number || "Quotation"}</p>
                <div className="mt-0.5 flex items-center gap-1.5">
                  <StatusPill family="quotation" status={q.status} />
                  {q.total ? <span className="text-xs text-fg-muted tabular">{formatCurrency(q.total, q.currency || "USD")}</span> : null}
                </div>
              </div>
              <ChevronRight className="size-3.5 shrink-0 text-fg-subtle" aria-hidden />
            </Link>
          ))}
        </div>
      )}
    </div>
  );
}

function RecordRow({ title, badge, value, onEdit, onDelete }: { title: string; badge: React.ReactNode; value: string | null; onEdit: () => void; onDelete: () => void }) {
  const [confirm, setConfirm] = useState(false);
  return (
    <div className="group flex items-center gap-2 rounded-lg px-2 py-1.5 hover:bg-surface-2">
      <button type="button" onClick={onEdit} className="min-w-0 flex-1 text-left" title="View or edit">
        <p className="truncate text-[13px] font-medium text-fg">{title}</p>
        <div className="mt-0.5 flex items-center gap-1.5">
          {badge}
          {value && <span className="text-xs text-fg-muted tabular">{value}</span>}
        </div>
      </button>
      {confirm ? (
        <>
          <Button size="sm" variant="danger" onClick={onDelete}>Delete</Button>
          <IconButton size="sm" icon={X} label="Keep it" onClick={() => setConfirm(false)} />
        </>
      ) : (
        <div className="flex items-center gap-0.5 opacity-100 md:opacity-0 md:group-focus-within:opacity-100 md:group-hover:opacity-100">
          <IconButton size="sm" icon={Pencil} label={`Edit ${title}`} onClick={onEdit} />
          <IconButton size="sm" icon={Trash2} label={`Delete ${title}`} onClick={() => setConfirm(true)} />
        </div>
      )}
    </div>
  );
}

function ActivitySection({ activities, onLog }: { activities: Activity[]; onLog: () => void }) {
  return (
    <div className={box}>
      <SectionHeader icon={<StickyNote className="size-3.5" aria-hidden />} title="Activity" count={activities.length} onAdd={onLog} addLabel="Log a note" />
      {activities.length === 0 ? <p className="text-xs text-fg-muted">No activity yet.</p> : (
        <div className="space-y-2">
          {activities.slice(0, 15).map((a) => (
            <div key={a.id} className="text-xs">
              <div className="flex items-center gap-1.5">
                <Pill>{statusLabel(a.activity_type)}</Pill>
                <span className="text-fg-muted tabular">{fmtDate(a.created_at)}</span>
              </div>
              {(a.subject || a.content) && <p className="mt-0.5 line-clamp-2 text-fg">{a.subject || a.content}</p>}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function UnmatchedState({ channel, phone, email, displayName, linkSearch, setLinkSearch, linkResults, onLink, onCreate }: {
  channel: "whatsapp" | "email"; phone?: string | null; email?: string | null; displayName?: string | null;
  linkSearch: string; setLinkSearch: (v: string) => void; linkResults: Contact[];
  onLink: (c: Contact) => void; onCreate: () => void;
}) {
  return (
    <div className="space-y-4 p-4">
      <div className="py-3 text-center">
        <div className="mx-auto flex size-12 items-center justify-center rounded-full bg-surface-2 text-fg-subtle">
          <User className="size-6" aria-hidden />
        </div>
        <p className="mt-2 text-sm font-medium text-fg">{displayName || "Unknown contact"}</p>
        <p className="text-xs text-fg-muted">
          {channel === "whatsapp" ? (phone ? `+${phone.replace(/^\+/, "")}` : "Number hidden by WhatsApp") : email}
        </p>
        <p className="mt-1 text-xs text-fg-muted">Not linked to a CRM contact yet.</p>
      </div>
      <Button variant="primary" className="w-full" onClick={onCreate}>
        <Plus aria-hidden /> Create contact
      </Button>
      <div>
        <Input type="search" leading={Search} value={linkSearch} onChange={(e) => setLinkSearch(e.target.value)}
          placeholder="Link existing contact" aria-label="Link existing contact" />
        {linkResults.length > 0 && (
          <div className="mt-1.5 divide-y divide-line-soft overflow-hidden rounded-lg border border-line">
            {linkResults.map((c) => (
              <button key={c.id} type="button" onClick={() => onLink(c)} className="w-full px-3 py-2 text-left text-xs hover:bg-surface-2">
                <span className="font-medium text-fg">{c.full_name || `${c.first_name} ${c.last_name ?? ""}`.trim()}</span>
                {c.email && <span className="text-fg-muted"> · {c.email}</span>}
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

// Modals

function FormDialog({ title, children, onClose, footer }: { title: string; children: React.ReactNode; onClose: () => void; footer?: React.ReactNode }) {
  return (
    <Dialog open onOpenChange={(o) => { if (!o) onClose(); }} title={title} size="sm" footer={footer}>
      {children}
    </Dialog>
  );
}

interface OrgRef { id: string; name: string }

function OrgSelect({ value, onChange, allowClear, prefillName, createExtra }: {
  value: OrgRef | null;
  onChange: (org: OrgRef | null) => void;
  allowClear?: boolean;
  prefillName?: string;
  createExtra?: Record<string, string>;
}) {
  const [query, setQuery] = useState(prefillName ?? "");
  const [results, setResults] = useState<OrgRef[]>([]);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (value || !query.trim()) { setResults([]); return; }
    const t = setTimeout(async () => {
      try { const r = await crmApi.listOrgs({ q: query, limit: 8 }); setResults(r.data.items ?? r.data ?? []); }
      catch { setResults([]); }
    }, 300);
    return () => clearTimeout(t);
  }, [query, value]);

  async function createOrSelect() {
    const name = query.trim();
    if (!name) return;
    setBusy(true);
    try {
      // Prefer an existing org with the same name (case-insensitive) over a duplicate.
      const r = await crmApi.listOrgs({ q: name, limit: 10 });
      const items: OrgRef[] = r.data.items ?? r.data ?? [];
      const exact = items.find((o) => o.name.toLowerCase() === name.toLowerCase());
      if (exact) { onChange({ id: exact.id, name: exact.name }); }
      else { const c = await crmApi.createOrg({ name, ...(createExtra ?? {}) }); onChange({ id: c.data.id, name }); }
      setOpen(false);
    } catch { /* keep the picker open */ }
    finally { setBusy(false); }
  }

  if (value) {
    return (
      <div className="flex h-11 items-center gap-2 rounded-lg border border-line-strong bg-surface px-3 text-sm text-fg md:h-9">
        <Building2 className="size-3.5 text-fg-subtle" aria-hidden />
        <span className="flex-1 truncate">{value.name}</span>
        {allowClear !== false && (
          <IconButton size="sm" icon={X} label="Clear organization" onClick={() => { onChange(null); setQuery(""); }} />
        )}
      </div>
    );
  }

  const name = query.trim();
  const hasExact = results.some((o) => o.name.toLowerCase() === name.toLowerCase());
  return (
    <div className="relative">
      <Input type="search" autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other" value={query} placeholder="Search or create organization"
        onChange={(e) => { setQuery(e.target.value); setOpen(true); }} onFocus={() => setOpen(true)} />
      {open && (results.length > 0 || (name.length > 0 && !hasExact)) && (
        <div className="absolute z-10 mt-1 w-full divide-y divide-line-soft overflow-hidden rounded-lg border border-line bg-surface-raised shadow-pop">
          {results.map((o) => (
            <button key={o.id} type="button" onClick={() => { onChange({ id: o.id, name: o.name }); setOpen(false); }}
              className="w-full truncate px-3 py-2 text-left text-sm text-fg hover:bg-surface-2">{o.name}</button>
          ))}
          {name.length > 0 && !hasExact && (
            <button type="button" onClick={createOrSelect} disabled={busy}
              className="w-full px-3 py-2 text-left text-sm text-brand-ink hover:bg-surface-2 disabled:opacity-50">
              {busy ? "Creating…" : `+ Create “${name}”`}
            </button>
          )}
        </div>
      )}
    </div>
  );
}

function ContactFormModal({ editing, prefill, initialOrg, onClose, onSaved, onError }: {
  editing: Contact | null;
  prefill: { displayName?: string | null; phone?: string | null; email?: string | null; channel: "whatsapp" | "email" };
  initialOrg: OrgRef | null;
  onClose: () => void; onSaved: (id: string, isNew: boolean, orgId: string | null) => void; onError: (m: string) => void;
}) {
  const initName = editing ? `${editing.first_name} ${editing.last_name ?? ""}`.trim() : (prefill.displayName ?? "");
  const initEmail = editing?.email ?? (prefill.channel === "email" ? prefill.email ?? "" : "");
  const [form, setForm] = useState({
    full_name: initName,
    position: editing?.position ?? "",
    email: initEmail,
    phone: editing?.phone ?? (prefill.channel === "whatsapp" ? prefill.phone ?? "" : ""),
    whatsapp: editing?.whatsapp ?? (prefill.channel === "whatsapp" ? prefill.phone ?? "" : ""),
  });
  const [saving, setSaving] = useState(false);
  const [org, setOrg] = useState<OrgRef | null>(initialOrg);
  const [orgPrefillName, setOrgPrefillName] = useState<string>("");
  const [orgCreateExtra, setOrgCreateExtra] = useState<Record<string, string> | undefined>(undefined);

  // Email channel, new contact: auto-match the org by the sender's company domain.
  useEffect(() => {
    if (editing || initialOrg || prefill.channel !== "email") return;
    const domain = domainFromEmail(initEmail);
    if (!domain || FREE_EMAIL_DOMAINS.has(domain)) return;
    (async () => {
      try {
        const r = await crmApi.listOrgs({ domain, limit: 1 });
        const items: OrgRef[] = r.data.items ?? r.data ?? [];
        if (items[0]) { setOrg({ id: items[0].id, name: items[0].name }); }
        else { setOrgPrefillName(orgNameFromDomain(domain)); setOrgCreateExtra({ website: `https://${domain}` }); }
      } catch { /* ignore */ }
    })();
  }, [editing, initialOrg, prefill.channel, initEmail]);

  async function save() {
    if (!form.full_name.trim()) { onError("Name is required."); return; }
    setSaving(true);
    const [first, ...rest] = form.full_name.trim().split(" ");
    const payload = {
      first_name: first, last_name: rest.join(" ") || undefined,
      position: form.position || undefined, email: form.email || undefined,
      phone: form.phone || undefined, whatsapp: form.whatsapp || undefined,
      organization_id: org?.id ?? null,
    };
    try {
      if (editing) { await crmApi.updateContact(editing.id, payload); onSaved(editing.id, false, org?.id ?? null); }
      else { const r = await crmApi.createContact(payload); onSaved(r.data.id, true, org?.id ?? null); }
    } catch (err) { onError(extractApiError(err, "Failed to save contact.")); }
    finally { setSaving(false); }
  }

  return (
    <FormDialog title={editing ? "Edit contact" : "New contact"} onClose={onClose}
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" loading={saving} onClick={save}>{editing ? "Save changes" : "Create contact"}</Button>
      </>}>
      <FieldGrid cols={1}>
        <Field label="Full name" required><Input value={form.full_name} onChange={(e) => setForm({ ...form, full_name: e.target.value })} /></Field>
        <Field label="Position"><Input value={form.position} onChange={(e) => setForm({ ...form, position: e.target.value })} /></Field>
        <Field label="Organization">
          <OrgSelect value={org} onChange={setOrg} allowClear prefillName={orgPrefillName} createExtra={orgCreateExtra} />
        </Field>
        <Field label="Email"><Input type="email" value={form.email} onChange={(e) => setForm({ ...form, email: e.target.value })} /></Field>
        <Field label="Phone"><Input value={form.phone} onChange={(e) => setForm({ ...form, phone: e.target.value })} /></Field>
        <Field label="WhatsApp"><Input value={form.whatsapp} onChange={(e) => setForm({ ...form, whatsapp: e.target.value })} /></Field>
      </FieldGrid>
    </FormDialog>
  );
}

function ActivityNotes({ entityType, entityId, onError }: { entityType: "lead" | "deal"; entityId: string; onError: (m: string) => void }) {
  const [items, setItems] = useState<Activity[]>([]);
  const [loading, setLoading] = useState(true);
  const [newNote, setNewNote] = useState("");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editText, setEditText] = useState("");
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try { const r = await crmApi.listActivities({ entity_type: entityType, entity_id: entityId, limit: 50 }); setItems(r.data ?? []); }
    catch { setItems([]); } finally { setLoading(false); }
  }, [entityType, entityId]);
  useEffect(() => { load(); }, [load]);

  async function add() {
    const content = newNote.trim();
    if (!content) return;
    setBusy(true);
    try {
      await crmApi.createActivity({ entity_type: entityType, entity_id: entityId, [`${entityType}_id`]: entityId, activity_type: "note", content });
      setNewNote(""); await load();
    } catch (e) { onError(extractApiError(e, "Failed to add note.")); } finally { setBusy(false); }
  }
  async function saveEdit(id: string) {
    setBusy(true);
    try { await crmApi.updateActivity(id, { content: editText.trim() }); setEditingId(null); await load(); }
    catch (e) { onError(extractApiError(e, "Failed to update note.")); } finally { setBusy(false); }
  }
  async function remove(id: string) {
    setBusy(true);
    try { await crmApi.deleteActivity(id); setItems((p) => p.filter((a) => a.id !== id)); }
    catch (e) { onError(extractApiError(e)); } finally { setBusy(false); }
  }

  return (
    <div className="mt-4 border-t border-line pt-3">
      <p className="mb-2 text-sm font-semibold text-fg">Activity and notes</p>
      <div className="mb-2 flex gap-2">
        <Input value={newNote} onChange={(e) => setNewNote(e.target.value)} aria-label="New note"
          onKeyDown={(e) => { if (e.key === "Enter") add(); }} placeholder="Add a note" />
        <Button variant="primary" onClick={add} disabled={busy || !newNote.trim()}>Add</Button>
      </div>
      {loading ? <p className="text-xs text-fg-muted">Loading…</p> : items.length === 0 ? <p className="text-xs text-fg-muted">No notes yet.</p> : (
        <div className="max-h-44 space-y-2 overflow-y-auto">
          {items.map((a) => (
            <div key={a.id} className="group text-xs">
              <div className="flex items-center gap-1.5">
                <Pill>{statusLabel(a.activity_type)}</Pill>
                <span className="text-fg-muted tabular">{fmtDate(a.created_at)}</span>
                {a.activity_type === "note" && editingId !== a.id && (
                  <div className="ml-auto flex items-center gap-0.5 opacity-100 md:opacity-0 md:group-focus-within:opacity-100 md:group-hover:opacity-100">
                    <IconButton size="sm" icon={Pencil} label="Edit note" onClick={() => { setEditingId(a.id); setEditText(a.content ?? ""); }} />
                    <IconButton size="sm" icon={Trash2} label="Delete note" onClick={() => remove(a.id)} />
                  </div>
                )}
              </div>
              {editingId === a.id ? (
                <div className="mt-1 flex gap-1.5">
                  <Input value={editText} onChange={(e) => setEditText(e.target.value)} aria-label="Edit note" />
                  <Button size="sm" variant="primary" onClick={() => saveEdit(a.id)} disabled={busy}>Save</Button>
                  <IconButton size="sm" icon={X} label="Cancel editing" onClick={() => setEditingId(null)} />
                </div>
              ) : (
                (a.subject || a.content) && <p className="mt-0.5 whitespace-pre-wrap text-fg">{a.content || a.subject}</p>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function LeadFormModal({ editing, contactId, orgId, channel, onClose, onSaved, onError }: {
  editing: Lead | null; contactId: string; orgId: string | null; channel: "whatsapp" | "email";
  onClose: () => void; onSaved: () => void; onError: (m: string) => void;
}) {
  const { functional } = useCurrencySettings();
  const { reg } = useCrmStages();
  const [form, setForm] = useState({
    title: editing?.title ?? "",
    status: editing?.status ?? "new",
    source: editing?.source ?? channel,
    estimated_value: editing?.estimated_value != null ? String(editing.estimated_value) : "",
    description: editing?.description ?? "",
    disqualification_reason: "",
  });
  const disqualifying = !!editing && form.status === "disqualified" && editing.status !== "disqualified";
  const [saving, setSaving] = useState(false);

  async function save() {
    if (!form.title.trim()) { onError("Lead title is required."); return; }
    setSaving(true);
    const payload = {
      title: form.title.trim(), source: form.source,
      estimated_value: form.estimated_value ? Number(form.estimated_value) : undefined,
      description: form.description || undefined,
      currency: functional, contact_id: contactId, organization_id: orgId ?? undefined,
    };
    try {
      if (editing) await crmApi.updateLead(editing.id, { title: form.title.trim(), status: form.status, source: form.source, estimated_value: form.estimated_value ? Number(form.estimated_value) : undefined, description: form.description || undefined,
                                                         ...(disqualifying ? { disqualification_reason: form.disqualification_reason } : {}) });
      else await crmApi.createLead(payload);
      onSaved();
    } catch (err) { onError(extractApiError(err, "Failed to save lead.")); }
    finally { setSaving(false); }
  }

  return (
    <FormDialog title={editing ? "Edit lead" : "New lead"} onClose={onClose}
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" loading={saving} onClick={save}>{editing ? "Save changes" : "Create lead"}</Button>
      </>}>
      <FieldGrid cols={1}>
        <Field label="Title" required><Input value={form.title} onChange={(e) => setForm({ ...form, title: e.target.value })} /></Field>
        {editing && (
          <Field label="Stage"><Select value={form.status} onChange={(e) => setForm({ ...form, status: e.target.value })}>
            {reg.lead_statuses.filter((s) => s.key === editing.status || canMoveLead(reg, editing.status, s.key))
              .map((s) => <option key={s.key} value={s.key}>{s.label}</option>)}
          </Select></Field>
        )}
        {disqualifying && (
          <Field label="Disqualification reason"><Select value={form.disqualification_reason}
            onChange={(e) => setForm({ ...form, disqualification_reason: e.target.value })} placeholder="Select a reason"
            options={reg.disqualify_reasons.map((r) => ({ value: r.key, label: r.label }))} /></Field>
        )}
        <Field label="Source"><Select value={form.source} onChange={(e) => setForm({ ...form, source: e.target.value })}
          options={reg.lead_sources.map((s) => ({ value: s.key, label: s.label }))} /></Field>
        <Field label="Estimated value"><Input numeric type="number" trailing={functional} value={form.estimated_value} onChange={(e) => setForm({ ...form, estimated_value: e.target.value })} /></Field>
        <Field label="Notes"><Textarea rows={3} value={form.description} onChange={(e) => setForm({ ...form, description: e.target.value })} placeholder="Notes about this lead" /></Field>
      </FieldGrid>
      {editing && <ActivityNotes entityType="lead" entityId={editing.id} onError={onError} />}
    </FormDialog>
  );
}

function DealFormModal({ editing, contactId, orgId, onClose, onSaved, onError }: {
  editing: Deal | null; contactId: string; orgId: string | null;
  onClose: () => void; onSaved: () => void; onError: (m: string) => void;
}) {
  const { functional } = useCurrencySettings();
  const { reg } = useCrmStages();
  const [form, setForm] = useState({
    title: editing?.title ?? "",
    stage: editing?.stage ?? "new",
    value: editing?.value != null ? String(editing.value) : "",
    probability: editing?.probability != null ? String(editing.probability) : "",   // "" = the stage default
    expected_close_date: editing?.expected_close_date ?? "",
    notes: editing?.notes ?? "",
  });
  const [reasons, setReasons] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);
  const reasonList = REASON_STAGES[form.stage] && form.stage !== editing?.stage ? reg[REASON_STAGES[form.stage]] : null;

  async function save() {
    if (!form.title.trim()) { onError("Deal title is required."); return; }
    setSaving(true);
    const base = {
      title: form.title.trim(), stage: form.stage,
      value: form.value ? Number(form.value) : undefined,
      probability: form.probability === "" ? undefined : Number(form.probability),
      expected_close_date: form.expected_close_date || undefined,
      notes: form.notes || undefined,
      ...(reasonList ? { loss_reasons: reasons } : {}),
    };
    try {
      if (editing) await crmApi.updateDeal(editing.id, base);
      else await crmApi.createDeal({ ...base, currency: functional, contact_id: contactId, organization_id: orgId ?? undefined });
      onSaved();
    } catch (err) { onError(extractApiError(err, "Failed to save deal.")); }
    finally { setSaving(false); }
  }

  return (
    <FormDialog title={editing ? "Edit deal" : "New deal"} onClose={onClose}
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" loading={saving} onClick={save}>{editing ? "Save changes" : "Create deal"}</Button>
      </>}>
      <FieldGrid cols={1}>
        <Field label="Title" required><Input value={form.title} onChange={(e) => setForm({ ...form, title: e.target.value })} /></Field>
        <Field label="Stage"><Select value={form.stage} onChange={(e) => {
          const p = reg.deal_stages.find((s) => s.key === e.target.value)?.probability;
          setForm({ ...form, stage: e.target.value, probability: p != null ? String(p) : "" });
        }}>
          {reg.deal_stages.map((s) => <option key={s.key} value={s.key}>{s.label}</option>)}
          {!reg.deal_stages.some((s) => s.key === form.stage) && <option value={form.stage}>{form.stage}</option>}
        </Select></Field>
        {reasonList && (
          <fieldset>
            <legend className="mb-1 text-[13px] font-medium text-fg">Reasons (select all that apply)</legend>
            <div className="max-h-40 space-y-0.5 overflow-y-auto">
              {reasonList.map((r) => (
                <Checkbox key={r.key} label={r.label} checked={reasons.includes(r.key)}
                  onChange={() => setReasons((prev) => prev.includes(r.key) ? prev.filter((k) => k !== r.key) : [...prev, r.key])} />
              ))}
            </div>
          </fieldset>
        )}
        <FieldGrid cols={2}>
          <Field label="Value"><Input numeric type="number" trailing={functional} value={form.value} onChange={(e) => setForm({ ...form, value: e.target.value })} /></Field>
          <Field label="Probability"><Input numeric type="number" trailing="%" value={form.probability} onChange={(e) => setForm({ ...form, probability: e.target.value })} /></Field>
        </FieldGrid>
        <Field label="Close date"><Input type="date" value={form.expected_close_date} onChange={(e) => setForm({ ...form, expected_close_date: e.target.value })} /></Field>
        <Field label="Notes"><Textarea rows={3} value={form.notes} onChange={(e) => setForm({ ...form, notes: e.target.value })} placeholder="Notes about this deal" /></Field>
      </FieldGrid>
      {editing && <ActivityNotes entityType="deal" entityId={editing.id} onError={onError} />}
    </FormDialog>
  );
}

function NoteModal({ contactId, channel, onClose, onSaved, onError }: {
  contactId: string; channel: "whatsapp" | "email"; onClose: () => void; onSaved: () => void; onError: (m: string) => void;
}) {
  const [text, setText] = useState("");
  const [saving, setSaving] = useState(false);
  async function save() {
    if (!text.trim()) { onError("Note can't be empty."); return; }
    setSaving(true);
    try {
      await crmApi.createActivity({ entity_type: "contact", entity_id: contactId, contact_id: contactId, activity_type: "note", content: text.trim() });
      onSaved();
    } catch (err) { onError(extractApiError(err, "Failed to log note.")); }
    finally { setSaving(false); }
  }
  return (
    <FormDialog title="Log a note" onClose={onClose}
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" loading={saving} onClick={save}>Log note</Button>
      </>}>
      <Field label="Note">
        <Textarea rows={4} value={text} onChange={(e) => setText(e.target.value)} placeholder={`Note about this ${channel} conversation`} />
      </Field>
    </FormDialog>
  );
}
