/** The access levels an MCP connection can have (backend/app/services/ai/mcp_access.py). */
export const LEVELS: Record<string, { label: string; hint: string }> = {
  read: { label: "Read only", hint: "Look things up. Nothing can be changed." },
  propose: { label: "Propose", hint: "Every change becomes an approval card; nothing changes until it is approved." },
  act: { label: "Act as me", hint: "Changes apply at once, like my own clicks. Messages to customers and invoices still wait for approval." },
};

/** The Owner's level (Nary's approver only): act as me plus everything Nary does. */
export const FULL_HINT =
  "Work like Nary: decide its cards, answer its questions, pause it, send WhatsApp and email as Nary, book and " +
  "change meetings and draft invoices. Messages, meetings and money go only after your yes in the chat. Includes Act as me.";

/** The level the consent page starts on. Stage 8: never one that can act (act, full): an agent can ask for
 *  it, but the person has to pick it (and type their password) themselves. */
export function initialLevel(requested: string[], allowed: string[]): string {
  const asked = ["propose", "read"].find((l) => requested.includes(l) && allowed.includes(l));
  return asked ?? "propose";
}

