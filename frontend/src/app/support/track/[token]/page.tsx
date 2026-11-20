"use client";

import { useState, useEffect } from "react";
import { MessageCircle, AlertTriangle } from "lucide-react";
import { Button, Card, CardBody, CardHeader, KeyValues, Pill, Skeleton, StatusPill } from "@/components/ui";
import { PublicPage, PublicNotice } from "@/components/portal/PublicPage";
import { cn } from "@/lib/utils";

interface Message { id: string; body: string; author_name: string | null; is_internal: boolean; created_at: string; }
interface Ticket {
  number: string; subject: string; description: string | null; status: string; priority: string;
  customer_name: string; customer_email: string; customer_company: string | null;
  team_name: string | null; category_name: string | null;
  created_at: string; resolved_at: string | null;
  messages: Message[];
}

const STATUS_LABEL: Record<string, string> = {
  new: "Received", open: "In progress", pending: "Pending", resolved: "Resolved", closed: "Closed",
};

export default function TrackTicketPage({ params }: { params: { token: string } }) {
  const [ticket, setTicket] = useState<Ticket | null>(null);
  const [loading, setLoading] = useState(true);
  const [notFound, setNotFound] = useState(false);

  useEffect(() => {
    fetch(`/api/v1/helpdesk/public/tickets/${params.token}`)
      .then(r => { if (!r.ok) throw new Error("not found"); return r.json(); })
      .then(setTicket)
      .catch(() => setNotFound(true))
      .finally(() => setLoading(false));
  }, [params.token]);

  function fmt(d: string) {
    return new Date(d).toLocaleString("en-GB", { day: "2-digit", month: "short", year: "numeric", hour: "2-digit", minute: "2-digit" });
  }

  if (loading) {
    return (
      <PublicPage subtitle="Support">
        <div className="space-y-4" aria-busy="true" aria-label="Loading ticket">
          <Skeleton className="h-16" /><Skeleton className="h-40" /><Skeleton className="h-56" />
        </div>
      </PublicPage>
    );
  }

  if (notFound || !ticket) {
    return (
      <PublicPage subtitle="Support" width="sm" center>
        <PublicNotice icon={AlertTriangle} tone="orange" title="Ticket not found"
          action={<Button variant="primary" asChild><a href="/support">Submit a new ticket</a></Button>}>
          <p>We could not find a ticket with this tracking link. Please check the link in your confirmation message.</p>
        </PublicNotice>
      </PublicPage>
    );
  }

  const initial = ticket.customer_name.charAt(0).toUpperCase();

  return (
    <PublicPage subtitle="Support">
      <header className="mb-5">
        <p className="mb-1 text-xs font-semibold uppercase tracking-[0.16em] text-brand-ink">Support ticket</p>
        <div className="flex flex-wrap items-center gap-3">
          <h1 className="font-mono text-xl font-semibold text-fg">{ticket.number}</h1>
          <StatusPill family="ticket" status={ticket.status} label={STATUS_LABEL[ticket.status] ?? undefined} />
        </div>
        <p className="mt-1 text-sm text-fg">{ticket.subject}</p>
      </header>

      <div className="space-y-4">
        <Card>
          <CardHeader title="Ticket details" />
          <CardBody>
            <KeyValues items={[
              { label: "Submitted by", value: `${ticket.customer_name}${ticket.customer_company ? ` · ${ticket.customer_company}` : ""}` },
              { label: "Submitted on", value: fmt(ticket.created_at) },
              { label: "Priority", value: <StatusPill family="priority" status={ticket.priority} /> },
              ...(ticket.team_name ? [{ label: "Team", value: ticket.team_name }] : []),
              ...(ticket.category_name ? [{ label: "Category", value: ticket.category_name }] : []),
              ...(ticket.resolved_at ? [{ label: "Resolved on", value: fmt(ticket.resolved_at) }] : []),
            ]} />
          </CardBody>
        </Card>

        <Card>
          <CardHeader title="Conversation" />
          <CardBody>
            <ol className="space-y-5">
              <li className="flex gap-3">
                <Avatar>{initial}</Avatar>
                <div className="min-w-0 flex-1">
                  <div className="mb-1 flex flex-wrap items-baseline gap-x-2">
                    <p className="text-sm font-semibold text-fg">{ticket.customer_name}</p>
                    <span className="text-xs text-fg-muted">{fmt(ticket.created_at)}</span>
                  </div>
                  <div className="whitespace-pre-wrap rounded-lg bg-surface-2 px-4 py-3 text-sm text-fg">
                    {ticket.description ?? "(no description provided)"}
                  </div>
                </div>
              </li>

              {ticket.messages.map(msg => {
                const isAgent = !!msg.author_name;
                return (
                  <li key={msg.id} className={cn("flex gap-3", !isAgent && "flex-row-reverse")}>
                    <Avatar brand={isAgent}>{isAgent ? (msg.author_name?.charAt(0).toUpperCase() ?? "A") : initial}</Avatar>
                    <div className={cn("min-w-0 flex-1", !isAgent && "flex flex-col items-end")}>
                      <div className={cn("mb-1 flex flex-wrap items-baseline gap-x-2", !isAgent && "flex-row-reverse")}>
                        <p className="text-sm font-semibold text-fg">{isAgent ? msg.author_name : ticket.customer_name}</p>
                        <span className="text-xs text-fg-muted">{fmt(msg.created_at)}</span>
                        {isAgent && <Pill tone="blue">Support team</Pill>}
                      </div>
                      <div className={cn("inline-block max-w-full whitespace-pre-wrap rounded-lg px-4 py-3 text-sm text-fg",
                        isAgent ? "border border-line bg-surface" : "bg-surface-2")}>
                        {msg.body}
                      </div>
                    </div>
                  </li>
                );
              })}
            </ol>
            {ticket.messages.length === 0 && (
              <div className="mt-5 flex flex-col items-center gap-2 border-t border-line pt-5 text-center">
                <MessageCircle className="size-5 text-fg-subtle" aria-hidden />
                <p className="text-[13px] text-fg-muted">Our team will respond shortly.</p>
              </div>
            )}
          </CardBody>
        </Card>

        <p className="text-center text-xs text-fg-muted">
          Bookmark this page to follow your ticket.{" "}
          <a href="/support" className="font-medium text-brand-ink hover:underline">Submit another ticket</a>
        </p>
      </div>
    </PublicPage>
  );
}

function Avatar({ brand, children }: { brand?: boolean; children: React.ReactNode }) {
  return (
    <span aria-hidden className={cn("flex size-8 shrink-0 items-center justify-center rounded-full text-xs font-semibold",
      brand ? "bg-brand text-brand-on" : "bg-surface-2 text-fg-muted")}>
      {children}
    </span>
  );
}
