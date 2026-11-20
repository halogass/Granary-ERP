import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";

let granted: string[] = [];
vi.mock("@/hooks/usePermissions", () => ({
  usePermissions: () => ({
    loaded: true, isAdmin: false,
    can: (c: string) => granted.includes(c),
    holds: (c: string) => granted.includes(c),
  }),
}));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1" }, access_token: "tok" } }) }));
vi.mock("next/navigation", () => ({ useSearchParams: () => new URLSearchParams(window.location.search) }));
const sync = vi.fn();
vi.mock("@/lib/api", () => ({
  crmInboxApi: {
    mailboxes: () => Promise.resolve({ data: { mailboxes: [] } }),
    listThreads: () => Promise.resolve({ data: { items: [], total: 0 } }),
    getThread: () => Promise.resolve({ data: {
      id: "t1", subject: "Hello", status: "open", is_read: true, message_count: 0, messages: [],
      last_sender_email: "b@c.example", last_sender_name: "B", last_message_at: "2026-10-26T01:00:00Z",
    } }),
    replyDefaults: () => Promise.resolve({ data: { to: [], cc: [], subject: "Re: Hello" } }),
    sync: (...a: unknown[]) => sync(...a),
  },
  crmApi: {},
  aiApi: {},
  messagingApi: {},
  emailSignatureApi: {
    render: () => Promise.resolve({ data: { html: "" } }),
    getMine: () => Promise.resolve({ data: { fields: [] } }),
  },
  authApi: { me: () => Promise.resolve({ data: {} }) },
}));
vi.mock("@/components/crm/CrmContextPanel", () => ({ default: () => null }));
vi.mock("@/components/crm/FollowUpButton", () => ({ default: () => null }));
vi.mock("@/components/ai/DocAnalyzeButton", () => ({ default: () => null }));
vi.mock("@/components/ai/ReplyPromptButton", () => ({ default: () => null }));
vi.mock("@/components/crm/ScheduleSendModal", () => ({ default: () => null }));

import CrmEmailPage from "./page";

beforeEach(() => {
  window.localStorage.clear();
  Element.prototype.scrollIntoView = vi.fn();
  vi.stubGlobal("fetch", vi.fn(() => Promise.resolve({ ok: false, json: () => Promise.resolve([]) })));
});

describe("CRM email inbox permissions", () => {
  it("without a CRM view permission the inbox is not shown", async () => {
    granted = [];
    window.history.replaceState(null, "", "/crm/email");
    render(<CrmEmailPage />);
    expect(await screen.findByText("No access to the CRM email inbox")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Sync now/ })).toBeNull();
  });

  it("view without crm:send_email: reads, but no Sync and no reply", async () => {
    granted = ["crm:view_own"];
    window.history.replaceState(null, "", "/crm/email?thread=t1");
    render(<CrmEmailPage />);
    expect((await screen.findAllByText("Hello")).length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: /Sync now/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Reply" })).toBeNull();
    expect(document.querySelector("[data-reply-composer]")).toBeNull();
  });

  it("with crm:send_email the reply and Sync are there", async () => {
    granted = ["crm:view_all", "crm:send_email"];
    window.history.replaceState(null, "", "/crm/email?thread=t1");
    render(<CrmEmailPage />);
    expect(await screen.findByRole("button", { name: "Reply" })).toBeTruthy();
    expect(screen.getAllByRole("button", { name: /Sync now/ }).length).toBeGreaterThan(0);
  });
});
