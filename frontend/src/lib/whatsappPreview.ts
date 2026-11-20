/** What a chat's last message was when it has no text (system notices show a word, not WhatsApp's type name). */
export const PREVIEW_LABELS: Record<string, string> = {
  image: "Photo", video: "Video", document: "Document", audio: "Audio", ptt: "Voice note", sticker: "Sticker",
  location: "Location", vcard: "Contact card", multi_vcard: "Contact cards", call_log: "Call", revoked: "Message deleted",
  notification_template: "WhatsApp notice", e2e_notification: "WhatsApp notice", notification: "WhatsApp notice",
  gp2: "Group update", ciphertext: "Waiting for this message",
};

export function previewLabel(type?: string | null): string {
  if (!type || type === "chat" || type === "text") return "No messages";
  return PREVIEW_LABELS[type] ?? "Message";
}
