import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const getThread = vi.fn();
const replyDefaults = vi.fn();
const reply = vi.fn();

vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1" }, access_token: "tok" } }) }));
vi.mock("next/navigation", () => ({ useSearchParams: () => new URLSearchParams(window.location.search) }));
vi.mock("@/lib/api", () => ({
  crmInboxApi: {
    mailboxes: () => Promise.resolve({ data: { mailboxes: [] } }),
    listThreads: () => Promise.resolve({ data: { items: [], total: 0 } }),
    getThread: (...a: unknown[]) => getThread(...a),
    replyDefaults: (...a: unknown[]) => replyDefaults(...a),
    reply: (...a: unknown[]) => reply(...a),
  },
  crmApi: {},
  aiApi: {},
  messagingApi: {},
  emailSignatureApi: {
    render: () => Promise.resolve({ data: { html: '<div data-granary-signature="1"><b>Robert Smith</b><br>Owner</div>' } }),
    getMine: () => Promise.resolve({ data: { fields: [{ name: "name", value: "Robert Smith" }, { name: "title", value: "Owner" }] } }),
  },
  authApi: { me: () => Promise.resolve({ data: { permissions: ["crm:view_all", "crm:send_email"] } }) },
  emailImageSendersApi: {
    list: () => Promise.resolve({ data: { senders: [] } }),
    add: () => Promise.resolve({ data: {} }),
    remove: () => Promise.resolve({ data: {} }),
  },
}));
vi.mock("@/components/crm/CrmContextPanel", () => ({ default: () => null }));
vi.mock("@/components/crm/FollowUpButton", () => ({ default: () => null }));
vi.mock("@/components/ai/DocAnalyzeButton", () => ({ default: () => null }));
vi.mock("@/components/ai/ReplyPromptButton", () => ({ default: () => null }));
vi.mock("@/components/crm/ScheduleSendModal", () => ({ default: () => null }));

import CrmEmailPage from "./page";

const THREAD = {
  id: "t1", subject: "ERP rollout", status: "open", is_read: true, message_count: 1,
  last_sender_email: "robert@cust.example", last_sender_name: "Robert Smith", last_message_at: "2026-10-26T01:00:00Z",
  linked_contact_id: null, linked_contact_name: null, linked_org_id: null, linked_org_name: null,
  linked_lead_id: null, linked_deal_id: null,
  messages: [{
    id: "m1", thread_id: "t1", direction: "inbound", from_email: "robert@cust.example", from_name: "Robert Smith",
    to_emails: ["hello@example.com"], cc_emails: ["rachel@cust.example"], subject: "ERP rollout",
    body_html: null, body_text: "Can you quote 20 laptops?", received_at: "2026-10-26T01:00:00Z", attachments: [],
  }],
};

const composer = () => document.querySelector("[data-reply-composer]") as HTMLElement;
const editor = () => screen.getByRole("textbox", { name: /Write a reply/ });

function typeInEditor(html: string) {
  const el = editor();
  el.innerHTML = html;
  fireEvent.input(el);
}

beforeEach(() => {
  window.localStorage.clear();
  window.history.replaceState(null, "", "/crm/email?thread=t1");
  Element.prototype.scrollIntoView = vi.fn();
  vi.stubGlobal("fetch", vi.fn(() => Promise.resolve({ ok: false, json: () => Promise.resolve([]) })));
  getThread.mockResolvedValue({ data: THREAD });
  replyDefaults.mockResolvedValue({ data: {
    from_inbox: "hello@example.com", to: ["robert@cust.example"], cc: ["rachel@cust.example"], subject: "Re: ERP rollout",
  } });
  reply.mockResolvedValue({ data: { ...THREAD.messages[0], id: "m2", direction: "outbound" } });
});

describe("email reply composer", () => {
  it("is folded to a bar by default and Reply opens it to the last sender only", async () => {
    render(<CrmEmailPage />);
    const replyBtn = await screen.findByRole("button", { name: "Reply" });
    expect(composer()).not.toBeVisible();
    expect(screen.getByRole("button", { name: "Reply all" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Forward" })).toBeVisible();
    await waitFor(() => expect(replyDefaults).toHaveBeenCalledWith("t1"));

    await userEvent.click(replyBtn);
    expect(composer()).toBeVisible();
    // The header folds to one line: To the last sender, no Cc
    expect(await screen.findByText("To Robert Smith · Re: ERP rollout")).toBeInTheDocument();
    expect(screen.queryByLabelText("Bcc")).toBeNull();

    // Reply all keeps the defaults' Cc
    await userEvent.click(screen.getByRole("button", { name: "Reply all" }));
    expect(screen.getByText("To Robert Smith +1 cc · Re: ERP rollout")).toBeInTheDocument();
  });

  it("Esc folds the composer and keeps the draft; R opens it again", async () => {
    render(<CrmEmailPage />);
    await userEvent.click(await screen.findByRole("button", { name: "Reply all" }));
    typeInEditor("<p>Hello Robert</p>");

    fireEvent.keyDown(editor(), { key: "Escape" });
    expect(composer()).not.toBeVisible();
    expect(screen.getByText("Unsent reply all draft")).toBeInTheDocument();
    expect(JSON.parse(window.localStorage.getItem("granary.crm.emailComposer.u1") || "{}").open).toBe(false);

    fireEvent.keyDown(document.body, { key: "r" });
    expect(composer()).toBeVisible();
    expect(editor().innerHTML).toContain("Hello Robert");
  });

  it("previews the signature as one line but sends it in full", async () => {
    render(<CrmEmailPage />);
    await userEvent.click(await screen.findByRole("button", { name: "Reply" }));
    const sig = await screen.findByRole("button", { name: "Signature: Robert Smith, Owner" });
    expect(sig).toHaveAttribute("aria-expanded", "false");
    await screen.findByText("To Robert Smith · Re: ERP rollout");
    typeInEditor("<p>Twenty laptops, noted.</p>");

    await userEvent.click(screen.getByRole("button", { name: "Send reply" }));
    await waitFor(() => expect(reply).toHaveBeenCalled());
    const fd = reply.mock.calls[0][1] as FormData;
    expect(fd.get("body_html")).toBe('<p>Twenty laptops, noted.</p><div data-granary-signature="1"><b>Robert Smith</b><br>Owner</div>');
    expect(fd.get("to")).toBe("robert@cust.example");
    expect(fd.get("cc")).toBe("");
  });
});
