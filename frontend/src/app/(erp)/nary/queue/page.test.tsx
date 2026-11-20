import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import WorkQueuePage from "./page";

const list = vi.fn();
vi.mock("@/components/nary/workQueue", async (orig) => ({
  ...(await orig<typeof import("@/components/nary/workQueue")>()),
  workQueueApi: { list: (...a: unknown[]) => list(...a) },
}));
vi.mock("@/components/nary/NaryTabs", () => ({
  default: ({ subtitle, actions }: { subtitle?: string; actions?: React.ReactNode }) => <div><p>{subtitle}</p>{actions}</div>,
}));
vi.mock("next/navigation", () => ({ usePathname: () => "/nary/queue", useRouter: () => ({ push: vi.fn(), replace: vi.fn() }) }));

const PROMPT = "Connect to Granary. Call nary_work_queue. Work the urgent items first.";
const DATA = {
  total: 2, matching: 2, shown: 2, by_urgency: { urgent: 1, today: 1, normal: 0 },
  by_kind: { whatsapp_reply: 1, followup_due: 1 }, nary_paused: true, scheduled_task_prompt: PROMPT,
  items: [
    { id: "whatsapp_reply:1555@c.us", kind: "whatsapp_reply", title: "Robert is waiting for an answer on WhatsApp: \"Hello\"",
      subject: { name: "Robert", chat_id: "1555@c.us" }, since: "2026-11-04T01:00:00Z", urgency: "urgent",
      suggested_tool: "get_whatsapp_messages", suggested_args: { chat_id: "1555@c.us" },
      hint: "Read the chat, then answer with propose_customer_message.", link: "/crm/whatsapp?chat=1555@c.us" },
    { id: "followup_due:r1", kind: "followup_due", title: "Follow-up due Wed 04 Nov 09:00: Call John Doe",
      subject: { name: "Call John Doe", reminder_id: "r1" }, since: "2026-11-04T02:00:00Z", urgency: "today",
      suggested_tool: "propose_reminder_change", suggested_args: { reminder_id: "r1", action: "done" },
      hint: "Do the follow-up.", link: "/crm/leads/l1" },
  ],
};

describe("Nary > Work queue (3 Nov 2026)", () => {
  beforeEach(() => {
    list.mockReset();
    list.mockResolvedValue({ data: DATA });
  });

  it("lists what is waiting with its urgency, next tool and a link to the record", async () => {
    render(<WorkQueuePage />);
    expect(await screen.findAllByText(/Robert is waiting for an answer on WhatsApp/)).not.toHaveLength(0);
    expect(screen.getAllByText("get_whatsapp_messages").length).toBeGreaterThan(0);
    expect(screen.getAllByText("propose_reminder_change").length).toBeGreaterThan(0);
    const open = screen.getAllByRole("link", { name: "Open" });
    expect(open.map((a) => a.getAttribute("href"))).toContain("/crm/whatsapp?chat=1555@c.us");
    expect(screen.getByText(/Nary is paused/)).toBeTruthy();
    expect(list).toHaveBeenCalledWith({ urgency: undefined, kinds: undefined, limit: 500 });
  });

  it("filters by urgency and by kind", async () => {
    render(<WorkQueuePage />);
    await screen.findAllByText(/Robert is waiting/);
    fireEvent.click(screen.getByRole("button", { name: /Urgent/ }));
    await waitFor(() => expect(list).toHaveBeenLastCalledWith({ urgency: "urgent", kinds: undefined, limit: 500 }));
    fireEvent.change(screen.getByLabelText("Kind"), { target: { value: "followup_due" } });
    await waitFor(() => expect(list).toHaveBeenLastCalledWith({ urgency: "urgent", kinds: "followup_due", limit: 500 }));
  });

  it("shows the scheduled-task prompt word for word", async () => {
    render(<WorkQueuePage />);
    expect(await screen.findByText(PROMPT)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Copy prompt" })).toBeTruthy();
  });

  it("says why when the server refuses", async () => {
    list.mockRejectedValue({ response: { data: { detail: "The work queue is for the Owner and system admins" } } });
    render(<WorkQueuePage />);
    expect(await screen.findByRole("alert")).toHaveProperty("textContent", "The work queue is for the Owner and system admins");
  });
});
