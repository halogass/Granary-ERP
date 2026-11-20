export type Citation = { type: string; id?: string; title?: string; slug?: string };

export type ProposalItem = {
  name: string; quantity: number; unit_price: number; unit?: string; description?: string;
  sku?: string | null; product_id?: string | null; discount_percentage?: number; is_optional?: boolean;
  /** Server-priced line total (covers recurring terms); preferred over qty × price when present. */
  amount?: number | null;
};

export type SopStep = { title: string; description: string; responsible?: string };

/** A create/update the assistant prepared for a CRM record. Nothing is saved until the
 *  user confirms, at which point the card posts it to /crm/ai/apply. */
export type RecordChange = {
  kind: "record_change";
  object_type: "organization" | "contact" | "lead" | "deal" | "activity";
  action: "create" | "update";
  record_id?: string | null;
  label?: string;
  /** Names for the ids the card touches (organization_id → the organisation's name). */
  display?: Record<string, string>;
  /** A create card's short name; other cards of the same answer point to it with "@ref". */
  ref?: string | null;
  before: Record<string, unknown>;
  after: Record<string, unknown>;
};

export type DocProposal = {
  kind: "quotation" | "invoice" | "sop";
  payload: Record<string, unknown> & { items?: ProposalItem[]; currency?: string; sop_steps?: SopStep[] };
  customer?: string; currency?: string; total?: number; title?: string; step_count?: number; warnings?: string[];
  /** 1 Nov 2026: a header discount the Owner approved, as text ("10% off, ...") and its amount. */
  discount?: string; discount_amount?: number;
  /** 4 Nov 2026: what the house notes template filled, what was written for the deal and what is missing. */
  notes_summary?: string;
};

/** A follow-up reminder the assistant prepared; saved only when the user confirms. */
export type FollowUpProposal = {
  kind: "followup_reminder";
  title: string; note?: string | null; summary?: string;
  entity_type: string; entity_id?: string | null; chat_id?: string | null; due_at: string;
  /** The company time zone (IANA): the card shows and submits the time in it, not in the browser's zone. */
  time_zone?: string | null;
};

/** Deep web research on a CRM record; starts only when the user confirms the card. */
export type ResearchProposal = {
  kind: "research";
  object_type: "organization" | "contact";
  record_id: string; label: string; summary?: string;
};

/** Set by the server on every card: `proposal_id` lets the card report what the user did with it,
 *  and `status` / `result` bring that outcome back when the chat is reopened. */
export type ProposalMeta = {
  proposal_id?: string;
  /** withdrawn: Nary took it back; superseded: a newer card to the same person replaced it. */
  status?: "pending" | "applied" | "discarded" | "failed" | "withdrawn" | "superseded";
  result?: { id?: string; label?: string } | null;
};

/** An email or WhatsApp message Nary sends as itself once the user confirms this exact text. */
export type CustomerMessageProposal = {
  kind: "customer_message";
  channel: "email" | "whatsapp";
  text: string; recipient: string; summary?: string; warning?: string;
  chat_id?: string | null; thread_id?: string | null; contact_id?: string | null;
  to?: string | null; subject?: string | null;
  /** Email only: the CRM inbox it goes from and the copies, exactly as they will be sent. */
  from_inbox?: string | null; cc?: string[] | null; bcc?: string[] | null;
  /** Documents and files sent with the message (both channels), and one label each for the card (name, size,
   * type, approval). */
  attach?: { type: string; id: string }[] | null; attachments?: string[] | null;
  /** 29 Oct 2026: the files (not the sales documents) with their size and type; a picture stored on a record
   * has a thumbnail loaded through the authenticated attachment download. */
  files?: CardFile[] | null;
};

/** One file on a customer message card. On WhatsApp a picture goes as a photo, anything else as a document.
 * 1 Nov 2026: a video carries its length; send_as says how a video or an emailed file goes (WhatsApp:
 * "video" plays in the chat, "document" over nary.wa.video_max_mb or not MP4; email: "link" over the email
 * limit). */
export type CardFile = {
  id: string; name: string; size: number; size_label: string; mime_type: string; type: string;
  picture: boolean; source?: string; thumbnail_url?: string | null;
  video?: boolean; duration_seconds?: number | null; duration_label?: string | null;
  send_as?: "photo" | "video" | "document" | "link" | null;
};

/** Linking a WhatsApp chat to a CRM contact. */
export type ChatLinkProposal = { kind: "chat_link"; chat_id: string; contact_id: string; label?: string; summary?: string };

/** Moving, finishing or cancelling one of the user's follow-up reminders. */
export type ReminderChangeProposal = {
  kind: "reminder_change"; reminder_id: string; action: "snooze" | "done" | "cancel"; until?: string | null;
  label?: string; summary?: string;
};

/** A meeting at an agreed time: a Google Calendar event with Meet, then (optionally) the customer's message
 * with {link} replaced by the Meet link. */
export type MeetingProposal = {
  kind: "meeting"; title: string; start: string; duration_minutes: number;
  /** The company time zone (IANA) the summary's time is written in. */
  time_zone?: string | null;
  staff_ids: string[]; staff: string[]; customer_email?: string | null;
  deal_id?: string | null; lead_id?: string | null; contact_id?: string | null;
  message?: { channel: "email" | "whatsapp"; text: string; recipient?: string; chat_id?: string; thread_id?: string;
              contact_id?: string; to?: string; subject?: string } | null;
  label?: string; summary?: string;
};

/** One of Nary's actions asked for in the chat (cancel a meeting, add guests, merge two contacts), at its Ask
 * level on Nary → Autonomy; optionally a message to the customer afterwards. */
export type NaryActionProposal = {
  kind: "nary_action"; action: string; payload: Record<string, unknown>; summary: string; label?: string;
  message?: MeetingProposal["message"];
  /** What the card changes, line by line (a quotation update: fields before and after, lines, total). */
  details?: string[];
};

export type Proposal = (DocProposal | RecordChange | FollowUpProposal | ResearchProposal | CustomerMessageProposal
  | ChatLinkProposal | ReminderChangeProposal | MeetingProposal | NaryActionProposal) & ProposalMeta;

export type ProposalOutcome = "applied" | "discarded";
export type ResolveProposal = (status: ProposalOutcome, result?: { id?: string; label?: string }) => void;

/** A filtered record list the assistant looked up, rendered as a table under the answer. */
export type RecordsCard = {
  type: "records";
  object_type: "organization" | "contact" | "lead" | "deal";
  filters: { field: string; op: string; value?: unknown }[];
  count: number; offset: number; limit: number; has_more: boolean;
  items: Record<string, unknown>[];
};

export type ChatMessage = {
  id: string;
  role: "user" | "assistant";
  content: string;
  citations?: Citation[];
  proposals?: Proposal[];
  cards?: RecordsCard[];
};

export type Conversation = { id: string; title: string; updated_at?: string };
