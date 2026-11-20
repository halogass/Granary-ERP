"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2, MessageCircle, Plus, Search, Star, Trash2, Users } from "lucide-react";
import { naryApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Button, Card, CardBody, CardHeader, IconButton, Input, Pill, Skeleton } from "@/components/ui";

interface Link { id: string; chat_id: string; phone: string | null; label: string | null; is_primary: boolean; is_group: boolean }
interface Hit {
  chat_id: string; name: string | null; number: string | null; is_group: boolean;
  saved_contact: boolean; has_chat: boolean; linked_to: string | null;
}

const looksLikeNumber = (s: string) => s.replace(/\D/g, "").length >= 8 && /^[+\d\s().-]+$/.test(s.trim());
/** A typed number in WhatsApp's international form: digits only, country code first. */
const international = (s: string) => s.replace(/\D/g, "");

/** Profile (or an admin, via `userId`): the WhatsApp chats that are this person's. Nary treats them as
 *  staff, never as customers; notifications go to the primary one (the star). A group can be linked
 *  as internal but never gets notifications. The search covers the company WhatsApp: people who can
 *  read the WhatsApp inbox search by name or part of a number; everyone else confirms a full number. */
export default function MyWhatsAppChats({ userId, title = "My WhatsApp chats" }: { userId?: string; title?: string }) {
  const [links, setLinks] = useState<Link[]>([]);
  const [suggestions, setSuggestions] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<Hit[]>([]);
  const [full, setFull] = useState<boolean | null>(null);
  const [searching, setSearching] = useState(false);
  const [label, setLabel] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const seq = useRef(0);

  const load = useCallback(async () => {
    try {
      const res = await naryApi.staffChats(userId);
      setLinks(res.data.links);
      setSuggestions(res.data.suggestions);
    } catch (e) { setError(extractApiError(e)); }
    finally { setLoading(false); }
  }, [userId]);
  useEffect(() => { load(); }, [load]);

  // Debounced search; only the latest answer is shown.
  useEffect(() => {
    const q = query.trim();
    if (q.length < 2) { setHits([]); return; }
    const mine = ++seq.current;
    const t = setTimeout(async () => {
      setSearching(true);
      try {
        const res = await naryApi.searchStaffChats(q, userId);
        if (mine === seq.current) { setHits(res.data.results); setFull(res.data.full); }
      } catch (e) { if (mine === seq.current) setError(extractApiError(e)); }
      finally { if (mine === seq.current) setSearching(false); }
    }, 300);
    return () => clearTimeout(t);
  }, [query, userId]);

  const act = async (fn: () => Promise<unknown>) => {
    setBusy(true); setError(null);
    try { await fn(); await load(); } catch (e) { setError(extractApiError(e)); } finally { setBusy(false); }
  };
  const link = (chat: string, lbl?: string) => act(async () => {
    await naryApi.addStaffChat({ chat, label: lbl || label.trim() || undefined, user_id: userId });
    setQuery(""); setHits([]); setLabel("");
  });

  const linked = new Set(links.map((l) => l.chat_id));
  const q = query.trim();
  const typedNumberNotFound = looksLikeNumber(q) && !searching
    && !hits.some((h) => h.number === international(q));

  return (
    <Card>
      <CardHeader title={<span className="inline-flex items-center gap-2"><MessageCircle className="size-4 text-fg-muted" aria-hidden />{title}</span>} />
      <CardBody>
        <p className="mb-4 text-xs text-fg-muted">
          Every number linked here is treated as staff, never as a customer. Nary&apos;s notifications and briefs go to
          the starred one. From any of them you can chat with Nary by writing to the company WhatsApp: it answers like
          Nary&apos;s chat, with your permissions (send &quot;Nary new&quot; for a fresh conversation). A group can be linked so
          Nary treats it as internal; it never gets notifications or answers there.
        </p>

        {loading ? <Skeleton className="mb-4 h-8 w-full" /> : (
          <ul className="mb-4 space-y-1">
            {links.length === 0 && <li className="text-xs text-fg-muted">No chats linked yet.</li>}
            {links.map((l) => (
              <li key={l.id} className="flex items-center gap-2 text-sm">
                {l.is_group ? (
                  <span className="flex w-8 justify-center" title="Group: internal, no notifications"><Users className="size-4 text-fg-subtle" aria-hidden /></span>
                ) : (
                  <button type="button" disabled={busy || l.is_primary} aria-label={l.is_primary ? "Primary chat" : `Make ${l.chat_id} primary`}
                    title={l.is_primary ? "Notifications go here" : "Send notifications here"}
                    onClick={() => act(() => naryApi.makeStaffChatPrimary(l.id))}
                    className="flex size-11 items-center justify-center rounded-lg hover:bg-surface-2 disabled:cursor-default disabled:hover:bg-transparent md:size-8">
                    <Star aria-hidden className={`size-4 ${l.is_primary ? "fill-current text-brand-ink" : "text-fg-subtle hover:text-fg"}`} />
                  </button>
                )}
                <span className="font-mono text-[13px] text-fg">{l.is_group ? "Group" : l.phone ? `+${l.phone}` : l.chat_id}</span>
                {l.label && <span className="text-xs text-fg-muted">{l.label}</span>}
                {l.is_primary && <Pill tone="blue">primary</Pill>}
                <IconButton icon={Trash2} size="sm" label={`Unlink ${l.chat_id}`} disabled={busy}
                  onClick={() => act(() => naryApi.removeStaffChat(l.id))} className="ml-auto text-fg-subtle hover:text-tone-red" />
              </li>
            ))}
          </ul>
        )}

        {suggestions.length > 0 && (
          <div className="mb-3 flex flex-wrap gap-2">
            {suggestions.map((s) => (
              <Button key={s} size="sm" disabled={busy} onClick={() => link(s, "From the employee record")}>
                <Plus aria-hidden /> Link +{s.replace("@c.us", "")}
              </Button>
            ))}
          </div>
        )}

        <div className="flex flex-wrap gap-2">
          <div className="min-w-[200px] flex-1">
            <Input type="search" leading={Search} value={query} onChange={(e) => setQuery(e.target.value)} aria-label="Search WhatsApp chats"
              autoComplete="off"
              placeholder={full === false ? "Type the full number, e.g. +1 555 010 0199" : "Search a name, number or group"} />
          </div>
          <Input value={label} onChange={(e) => setLabel(e.target.value)} placeholder="Label (optional)" aria-label="Label"
            autoComplete="off" className="w-40" />
        </div>

        {q.length >= 2 && (
          <ul className="mt-2 overflow-hidden rounded-lg border border-line" aria-label="Search results">
            {searching && <li className="px-3 py-2 text-xs text-fg-muted"><Loader2 className="inline size-3 animate-spin" aria-hidden /> Searching...</li>}
            {!searching && hits.length === 0 && !typedNumberNotFound && (
              <li className="px-3 py-2 text-xs text-fg-muted">
                {full === false ? "Type your complete WhatsApp number." : "No chat or saved contact matches."}
              </li>
            )}
            {hits.map((h) => {
              const mine = linked.has(h.chat_id);
              const taken = !!h.linked_to && !mine;
              return (
                <li key={h.chat_id} className="flex flex-wrap items-center gap-2 border-b border-line-soft px-3 py-2 text-sm last:border-0">
                  {h.is_group ? <Users className="size-4 text-fg-subtle" aria-hidden /> : <MessageCircle className="size-4 text-fg-subtle" aria-hidden />}
                  <span className="font-medium text-fg">{h.name || h.chat_id}</span>
                  {h.number && <span className="font-mono text-xs text-fg-muted">+{h.number}</span>}
                  {h.is_group && <span className="text-[11px] text-fg-subtle">group</span>}
                  {h.saved_contact && <span className="text-[11px] text-fg-subtle">saved contact</span>}
                  <span className="ml-auto">
                    {mine ? <span className="text-xs text-tone-blue">linked</span>
                      : taken ? <span className="text-xs text-fg-muted">linked to {h.linked_to}</span>
                      : <Button size="sm" variant="primary" disabled={busy} onClick={() => link(h.chat_id, h.is_group ? (h.name || undefined) : undefined)}>Link</Button>}
                  </span>
                </li>
              );
            })}
            {typedNumberNotFound && (
              <li className="flex flex-wrap items-center gap-2 px-3 py-2 text-sm">
                <span className="text-xs text-fg-muted">No chat with this number yet; link it anyway, it counts from its first message.</span>
                <Button size="sm" disabled={busy} onClick={() => link(q)} className="ml-auto">Link number</Button>
              </li>
            )}
          </ul>
        )}
        {error && <p role="alert" className="mt-2 text-xs text-tone-red">{error}</p>}
      </CardBody>
    </Card>
  );
}
