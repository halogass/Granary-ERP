/**
 * Putting an AI reply draft into the email reply editor.
 *
 * The model returns plain text; the editor holds HTML with the sender's signature at the bottom
 * (a `data-granary-signature` block). The draft replaces what was typed above the signature and keeps
 * the signature; its line breaks survive as paragraphs.
 */
function escapeHtml(t: string) {
  return t.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** A plain-text AI draft as editor HTML: blank lines split paragraphs, single newlines stay breaks. */
export function draftToHtml(draft: string): string {
  return draft.trim().split(/\n\s*\n/)
    .map((para) => `<p>${escapeHtml(para.trim()).replace(/\n/g, "<br>")}</p>`)
    .join("");
}

/** Replace what is typed above the signature with the draft, keeping the signature block. */
export function withDraft(currentHtml: string, draft: string): string {
  const at = currentHtml.search(/<div data-granary-signature/i);
  const signature = at >= 0 ? currentHtml.slice(at) : "";
  return draftToHtml(draft) + (signature ? `<p><br></p>${signature}` : "");
}

