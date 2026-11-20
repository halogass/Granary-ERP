import { formatCurrency } from "@/lib/currency";
import type { NaryCard } from "./types";

type Obj = Record<string, unknown>;
type Diff = NonNullable<NaryCard["diff"]>;

const isObj = (v: unknown): v is Obj => !!v && typeof v === "object" && !Array.isArray(v);

/** A value worth showing: not null, not an empty string, list or object. */
export function shown(v: unknown): boolean {
  if (v == null) return false;
  if (typeof v === "string") return v.trim() !== "";
  if (Array.isArray(v)) return v.some(shown);
  if (isObj(v)) return Object.values(v).some(shown);
  return true;
}

/** The message text a card sends: its own, or the one inside an agent's (MCP) proposal. */
export function cardText(card: NaryCard): string | null {
  if (typeof card.payload?.text === "string") return card.payload.text;
  const inner = card.action === "mcp.proposal" && isObj(card.payload?.proposal) ? card.payload.proposal : null;
  return inner && typeof inner.text === "string" ? inner.text : null;
}

// the keys of an agent's proposal that are bookkeeping (the API leaves them off newer cards too)
const HIDDEN = new Set(["kind", "summary", "label", "status", "payload", "action", "warnings", "attach", "files", "text",
  "notes_template", "notes_summary", "pricing_notes", "time_zone"]);

function proposalFields(p: Obj): Obj {
  const out: Obj = {};
  if ((p.kind === "quotation" || p.kind === "invoice") && isObj(p.payload)) {
    const doc = p.payload;
    for (const [k, v] of [["subject", doc.subject], ["customer", p.customer], ["currency", doc.currency ?? p.currency], ["total", p.total]] as const) {
      if (shown(v) && typeof v !== "object") out[k] = v;
    }
    const items = Array.isArray(doc.items) ? doc.items.filter(isObj) : [];
    if (items.length) {
      out.lines = items.map((i) => {
        const what = [i.name, i.sku, i.description].find((x) => typeof x === "string" && x.trim()) ?? "item";
        return `${i.quantity ?? 1} × ${what}${i.is_optional === true ? " (optional)" : ""}`;
      });
    }
    return out;
  }
  for (const [k, v] of Object.entries(p)) {
    if (HIDDEN.has(k) || k === "id" || k.endsWith("_id") || k.endsWith("_ids") || isObj(v)) continue;
    if (Array.isArray(v)) {
      const items = v.filter((x) => shown(x) && typeof x !== "object");
      if (items.length) out[k] = items;
    } else if (shown(v)) out[k] = v;
  }
  return out;
}

/**
 * The diff a card shows. Agent (MCP) cards saved before 21 Nov 2026 hold the whole proposal under `after.proposal`;
 * it is shown as its fields, as a newer card of that kind shows it, never as raw JSON.
 */
export function cardDiff(card: NaryCard): Diff | null {
  const d = card.diff;
  if (!d) return null;
  const after = d.after;
  if (card.action === "mcp.proposal" && after && Object.keys(after).length === 1 && isObj(after.proposal)) {
    return { ...d, after: proposalFields(after.proposal) };
  }
  return d;
}

const LABELS: Record<string, string> = {
  to: "To", cc: "Cc", bcc: "Bcc", from_inbox: "From inbox", due_at: "Due", entity_type: "Linked to",
  send_at_label: "Sends", pm: "Project manager", url: "URL", sku: "SKU", tax_id: "Tax ID", vat: "VAT",
};

/** A field key as words: "from_inbox" is "From inbox". */
export function fieldLabel(key: string): string {
  if (LABELS[key]) return LABELS[key];
  const words = key.replace(/[_.]+/g, " ").trim();
  return words ? words[0].toUpperCase() + words.slice(1) : key;
}

/** Money fields, formatted in the card's currency. */
const MONEY = /(^|_)(value|amount|price|total|subtotal|budget|revenue|cost|fee|discount_amount|tax_amount|amount_due)$/;
export const isMoneyField = (key: string) => MONEY.test(key) && !/(_pct|percentage|_rate)$/.test(key);

/** The currency the card's amounts are in: the diff's, else the payload's or the agent's proposal's. */
export function cardCurrency(card: NaryCard, diff: Diff | null = card.diff): string | null {
  const proposal = isObj(card.payload?.proposal) ? card.payload.proposal : null;
  const doc = proposal && isObj(proposal.payload) ? proposal.payload : null;
  const found = [diff?.currency, diff?.after?.currency, diff?.before?.currency, card.payload?.currency,
    proposal?.currency, doc?.currency].find((c) => typeof c === "string" && /^[A-Za-z]{3}$/.test(c));
  return (found as string | undefined)?.toUpperCase() ?? null;
}

function scalar(value: unknown, field: string | undefined, currency: string | null): string {
  if (value == null || value === "") return "-";
  if (field && isMoneyField(field)) {
    const n = typeof value === "number" ? value : typeof value === "string" && value.trim() !== "" ? Number(value) : NaN;
    // a card saved before its currency was recorded still groups the digits
    if (Number.isFinite(n)) return currency ? formatCurrency(n, currency) : n.toLocaleString("en-US", { maximumFractionDigits: 2 });
  }
  if (typeof value === "boolean") return value ? "Yes" : "No";
  return String(value);
}

/** A diff value: text, a list, or a nested list of fields; never JSON. Money fields are in the card's currency. */
export function DiffValue({ value, field, currency = null }: { value: unknown; field?: string; currency?: string | null }) {
  if (Array.isArray(value)) {
    const items = value.filter(shown);
    return (
      <ul className="list-disc space-y-0.5 pl-4">
        {items.map((v, i) => <li key={i}><DiffValue value={v} field={field} currency={currency} /></li>)}
      </ul>
    );
  }
  if (isObj(value)) {
    return (
      <dl className="space-y-0.5">
        {Object.entries(value).filter(([, v]) => shown(v)).map(([k, v]) => (
          <div key={k}><dt className="inline text-xs text-fg-subtle">{fieldLabel(k)}: </dt><dd className="inline"><DiffValue value={v} field={k} currency={currency} /></dd></div>
        ))}
      </dl>
    );
  }
  return <>{scalar(value, field, currency)}</>;
}

/**
 * The card's title. An agent's (MCP) card is named by what it does ("Quotation draft for Hoshino Design Office",
 * "Email reply to Hiroshi Haddad"); the API sends it as diff.title, and cards saved before 21 Nov 2026 get it here.
 */
export function cardTitle(card: NaryCard): string {
  const d = card.diff;
  if (typeof d?.title === "string" && d.title.trim()) return d.title;
  const p = card.action === "mcp.proposal" && isObj(card.payload?.proposal) ? card.payload.proposal : null;
  if (!p) return card.label;
  const str = (k: string) => (typeof p[k] === "string" && (p[k] as string).trim() ? (p[k] as string).trim() : null);
  const forCustomer = (what: string) => (str("customer") ? `${what} for ${str("customer")}` : what);
  switch (p.kind) {
    case "quotation": return forCustomer("Quotation draft");
    case "invoice": return forCustomer("Invoice draft");
    case "customer_message": {
      const email = p.channel === "email";
      const reply = !!(str("thread_id") || str("chat_id"));
      const what = email ? (reply ? "Email reply" : "Email") : (reply ? "WhatsApp reply" : "WhatsApp message");
      const who = str("recipient") ?? str("to");
      return who ? `${what} to ${who}` : what;
    }
    case "followup_reminder": return "Follow-up reminder";
    case "reminder_change": return "Reminder change";
    case "meeting": return "Meeting";
    case "record_change": {
      const obj = (str("object_type") ?? "record").replace(/_/g, " ");
      return p.action === "create" ? `New ${obj}` : p.action === "delete" ? `Delete ${obj}` : `Update ${obj}`;
    }
    case "nary_action": {
      const merges: Record<string, string> = { "organization.merge": "Merge organizations", "contact.merge": "Merge contacts", "lead.merge": "Merge leads" };
      return merges[String(p.action)] ?? card.label;
    }
    default: return card.label;
  }
}

const SECTION_LABELS: [string, string][] = [
  ["scope_sold", "Scope sold"], ["scope_not_sold", "Not sold"], ["success_criteria", "Success criteria"],
  ["commitments", "Our commitments"], ["sensitivities", "Sensitivities"], ["red_flags", "Red flags"],
  ["contacts", "Key contacts"], ["open_items", "Open items"],
];

function lines(v: unknown): string[] {
  if (!Array.isArray(v)) return [];
  return v.map((x) => {
    if (typeof x === "string") return x.trim();
    if (isObj(x) && typeof x.quote === "string") return x.source ? `${x.quote} (${x.source})` : x.quote;
    return "";
  }).filter(Boolean);
}

/**
 * "Start the project from the handoff package" (handoff.kickoff): the project it makes, its PM and customer, the
 * kickoff tasks and the handoff package. Empty package sections are left out. Cards saved before 21 Nov 2026 hold
 * only `after.package`; the project name then comes from the card label.
 */
export function KickoffDetails({ card }: { card: NaryCard }) {
  const after: Obj = card.diff?.after ?? {};
  const pkg: Obj = isObj(after.package) ? after.package : {};
  const sections: Obj = isObj(after.handoff) ? after.handoff : pkg;
  const label = card.diff?.label ?? "";
  const project = typeof after.project === "string" ? after.project
    : label.startsWith("Start the project for ") ? label.slice("Start the project for ".length) : null;
  const summary = typeof after.handoff_summary === "string" ? after.handoff_summary
    : typeof pkg.summary === "string" ? pkg.summary : null;
  const tasks = lines(after.kickoff_tasks);
  const facts = ([["Project", project], ["Customer", after.customer], ["Project manager", after.pm]] as const)
    .filter(([, v]) => typeof v === "string" && v.trim());
  const filled = SECTION_LABELS.map(([k, l]) => [l, lines(sections[k])] as const).filter(([, ls]) => ls.length);
  return (
    <div className="mt-3 space-y-3 text-[13px]">
      {facts.length > 0 && (
        <dl className="grid grid-cols-1 gap-x-3 gap-y-1 sm:grid-cols-[auto,1fr]">
          {facts.map(([k, v]) => (
            <div key={k} className="contents">
              <dt className="text-xs text-fg-subtle">{k}</dt>
              <dd className="min-w-0 break-words text-fg">{String(v)}</dd>
            </div>
          ))}
        </dl>
      )}
      {tasks.length > 0 && (
        <section>
          <h4 className="text-xs font-semibold text-fg-muted">Kickoff tasks</h4>
          <ol className="mt-1 list-decimal space-y-0.5 pl-5 text-fg">{tasks.map((t, i) => <li key={i}>{t}</li>)}</ol>
        </section>
      )}
      {(summary || filled.length > 0) && (
        <section>
          <h4 className="text-xs font-semibold text-fg-muted">Handoff package</h4>
          {summary && <p className="mt-1 text-fg">{summary}</p>}
          {filled.map(([l, ls]) => (
            <div key={l} className="mt-1.5">
              <p className="text-xs text-fg-subtle">{l}</p>
              <ul className="list-disc space-y-0.5 pl-4 text-fg">{ls.map((x, i) => <li key={i}>{x}</li>)}</ul>
            </div>
          ))}
        </section>
      )}
    </div>
  );
}
