"use client";

import Link from "next/link";
import { ExternalLink, Table2 } from "lucide-react";
import type { RecordsCard as Card } from "./types";
import { stack } from "@/lib/stackTable";
import { cn } from "@/lib/utils";

const LABEL: Record<string, { plural: string; path: string }> = {
  organization: { plural: "companies", path: "/crm/organizations" },
  contact: { plural: "contacts", path: "/crm/contacts" },
  lead: { plural: "leads", path: "/crm/leads" },
  deal: { plural: "deals", path: "/crm/deals" },
};

/** Columns worth showing per object; the rest stays in the record itself. */
const COLUMNS: Record<string, { key: string; header: string }[]> = {
  organization: [{ key: "industry", header: "Industry" }, { key: "city", header: "City" }],
  contact: [{ key: "email", header: "Email" }, { key: "position", header: "Position" }],
  lead: [{ key: "status", header: "Status" }, { key: "priority", header: "Priority" }],
  deal: [{ key: "stage", header: "Stage" }, { key: "value", header: "Value" }],
};

/** Deep link to the filter view, carrying the exact filters the assistant used. */
export function listHref(card: Pick<Card, "object_type" | "filters">): string {
  const payload = { object_type: card.object_type, filters: card.filters || [] };
  const encoded = typeof window === "undefined"
    ? Buffer.from(JSON.stringify(payload)).toString("base64")
    : window.btoa(unescape(encodeURIComponent(JSON.stringify(payload))));
  return `/crm/search?q=${encodeURIComponent(encoded)}`;
}

function cell(value: unknown): string {
  if (value == null || value === "") return "-";
  if (typeof value === "number") return value.toLocaleString();
  return String(value);
}

/** The result of a search_records call: what matched, a page of rows, and a way to see the rest. */
export default function RecordsCard({ card, onAskMore }: { card: Card; onAskMore?: (text: string) => void }) {
  const meta = LABEL[card.object_type] ?? { plural: card.object_type, path: "/crm" };
  const columns = COLUMNS[card.object_type] ?? [];
  const shown = card.items?.length ?? 0;

  return (
    <div className="mt-2 rounded-lg border border-line overflow-hidden not-prose">
      <div className="flex items-center gap-2 px-3 py-2 bg-surface-2 border-b border-line">
        <Table2 size={14} className="text-fg-subtle" />
        <p className="text-xs font-semibold text-fg-muted flex-1">
          {card.count.toLocaleString()} {meta.plural}
          {shown < card.count && ` · showing ${card.offset + 1}–${card.offset + shown}`}
        </p>
        <Link href={listHref(card)} className="inline-flex items-center gap-1 text-xs font-semibold text-brand-ink hover:underline">
          Open as list <ExternalLink size={11} />
        </Link>
      </div>
      {shown === 0 ? (
        <p className="px-3 py-4 text-xs text-fg-subtle">Nothing matched those filters.</p>
      ) : (
        <div className="max-h-72 overflow-y-auto">
          <table className={cn("w-full text-xs", stack.table)}>
            <thead className={cn("bg-surface sticky top-0", stack.thead)}>
              <tr className="text-left text-fg-subtle">
                <th className="px-3 py-1.5 font-semibold">Name</th>
                {columns.map((c) => <th key={c.key} className="px-3 py-1.5 font-semibold">{c.header}</th>)}
              </tr>
            </thead>
            <tbody className={cn("divide-y divide-line-soft", stack.tbody, "max-md:p-2")}>
              {card.items.map((row, i) => (
                <tr key={String(row.id ?? i)} className={cn("hover:bg-surface-2", stack.tr)}>
                  <td className={cn("px-3 py-1.5", stack.title)}>
                    <Link href={row.id ? `${meta.path}/${String(row.id)}` : meta.path}
                      className="font-medium text-fg hover:text-brand-ink">
                      {cell(row.label)}
                    </Link>
                  </td>
                  {columns.map((c) => <td key={c.key} data-label={c.header} className={cn("px-3 py-1.5 text-fg-muted", stack.td, "max-md:break-words")}>{cell(row[c.key])}</td>)}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {card.has_more && onAskMore && (
        <button onClick={() => onAskMore(`Show the next ${card.limit} ${meta.plural}`)}
          className="w-full px-3 py-2 text-xs font-semibold text-fg-muted hover:bg-surface-2 border-t border-line">
          Show next {card.limit}
        </button>
      )}
    </div>
  );
}
