// Mention tokens inside messaging text: `<@user:UUID>` for people, `<#type:UUID>` for records
// (deal, project, sop, product). The channel view renders them as chips; every other place that
// shows message text (thread panel, previews, notifications) must go through these helpers so a
// raw token never reaches the screen.

export interface MessageMention { type: string; id: string; name: string }

/** Matches one complete mention token. Global: reset `lastIndex` or use with `replace`/`split`. */
export const MENTION_TOKEN_RE = /<@user:[0-9a-f-]{36}>|<#\w+:[0-9a-f-]{36}>/g;

/** A token cut off by a server-side clip (previews keep the first 100 or 200 characters). */
const CUT_TOKEN_RE = /<[@#][\w:-]*$/;

export function mentionKey(m: MessageMention): string {
  return m.type === "user" ? `<@user:${m.id}>` : `<#${m.type}:${m.id}>`;
}

export function parseMentionToken(token: string): { type: string; id: string } | null {
  const user = token.match(/^<@user:([0-9a-f-]{36})>$/);
  if (user) return { type: "user", id: user[1] };
  const entity = token.match(/^<#(\w+):([0-9a-f-]{36})>$/);
  if (entity) return { type: entity[1], id: entity[2] };
  return null;
}

/** The label a chip shows for a token: the stored name when known, else a generic one. */
export function mentionLabel(type: string, name?: string): string {
  return `${type === "user" ? "@" : "#"}${name?.trim() || type}`;
}

/** Message text with every mention token replaced by its readable label, for plain-text places. */
export function mentionsToPlainText(raw: string | null | undefined, mentions?: MessageMention[] | null): string {
  if (!raw) return "";
  const byKey = new Map<string, MessageMention>();
  for (const m of mentions ?? []) byKey.set(mentionKey(m), m);
  return raw
    .replace(MENTION_TOKEN_RE, (token) => {
      const known = byKey.get(token);
      if (known) return mentionLabel(known.type, known.name);
      const parsed = parseMentionToken(token);
      return parsed ? mentionLabel(parsed.type) : token;
    })
    .replace(CUT_TOKEN_RE, "");
}
