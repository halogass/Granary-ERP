import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import ProposalCard from "./ProposalCard";

const linkChat = vi.fn();
const snooze = vi.fn();
const send = vi.fn();
const bookMeeting = vi.fn();
const runAction = vi.fn();
vi.mock("@/lib/api", () => ({
  naryApi: { linkChat: (...a: unknown[]) => linkChat(...a), send: (...a: unknown[]) => send(...a),
             bookMeeting: (...a: unknown[]) => bookMeeting(...a), runAction: (...a: unknown[]) => runAction(...a) },
  remindersApi: { snooze: (...a: unknown[]) => snooze(...a), done: vi.fn(), cancel: vi.fn() },
  researchApi: {}, salesApi: {}, financeApi: {}, kbApi: {},
}));
vi.mock("next/link", () => ({
  default: ({ href, children }: { href: string; children: React.ReactNode }) => <a href={href}>{children}</a>,
}));

describe("Nary's small cards", () => {
  it("links a WhatsApp chat to a contact only on Confirm", async () => {
    linkChat.mockResolvedValue({ data: { label: "linked to Diana Prince" } });
    const resolved = vi.fn();
    render(<ProposalCard proposal={{ kind: "chat_link", chat_id: "123@lid", contact_id: "c1",
      summary: "Link the WhatsApp chat +1 555-010-0142 to Diana Prince" }} onResolved={resolved} />);
    expect(linkChat).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: /Confirm/ }));
    await waitFor(() => expect(linkChat).toHaveBeenCalledWith("123@lid", "c1"));
    expect(await screen.findByText(/Done: Link the WhatsApp chat/)).toBeTruthy();
    expect(resolved).toHaveBeenCalledWith("applied", expect.anything());
  });

  it("books a meeting and sends the link only on Book and send", async () => {
    bookMeeting.mockResolvedValue({ data: { id: "m1", label: "Booked Diana call Wed 14 Oct 11:00, Meet: https://meet.google.com/abc",
                                            meet_link: "https://meet.google.com/abc", html_link: null, sent: "sent" } });
    const resolved = vi.fn();
    render(<ProposalCard proposal={{ kind: "meeting", title: "Diana call", start: "2026-10-14T11:00:00+07:00",
      duration_minutes: 45, staff_ids: ["u1"], staff: ["Owner"], summary: "Book “Diana call” Wed 14 Oct 11:00",
      message: { channel: "whatsapp", text: "See you at 11:00: {link}", recipient: "Diana", chat_id: "81813@c.us" } }}
      onResolved={resolved} />);
    expect(screen.getByText(/\[Meet link\]/)).toBeTruthy();
    expect(bookMeeting).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: /Book and send/ }));
    await waitFor(() => expect(bookMeeting).toHaveBeenCalledWith(expect.objectContaining({
      title: "Diana call", start: "2026-10-14T11:00:00+07:00", staff_ids: ["u1"],
      message: expect.objectContaining({ text: "See you at 11:00: {link}" }) })));
    expect(await screen.findByText(/Open Google Meet/)).toBeTruthy();
    expect(resolved).toHaveBeenCalledWith("applied", { id: "m1", label: expect.stringContaining("Booked") });
  });

  it("says why the link was not sent and hands it over, when the booking worked but the message didn't", async () => {
    bookMeeting.mockResolvedValue({ data: { id: "m2", meet_link: "https://meet.google.com/xyz", html_link: null, sent: null,
      sent_error: "This is a staff chat; customer messages never go there.",
      label: "Booked TEST Tue 13 Oct 15:00, Meet: https://meet.google.com/xyz; the WhatsApp message was NOT sent: "
           + "This is a staff chat; customer messages never go there. Send the Meet link yourself" } });
    render(<ProposalCard proposal={{ kind: "meeting", title: "TEST", start: "2026-10-13T15:00:00+07:00",
      duration_minutes: 30, staff_ids: [], staff: [], summary: "Book “TEST”",
      message: { channel: "whatsapp", text: "Link: {link}", recipient: "Diana", chat_id: "853@lid" } }} />);
    await userEvent.click(screen.getByRole("button", { name: /Book and send/ }));
    expect(await screen.findByText(/NOT sent: This is a staff chat/)).toBeTruthy();
    expect(screen.getByText("Copy the link")).toBeTruthy();
  });

  it("cancels a meeting only on Confirm, and says when the customer message failed", async () => {
    runAction.mockResolvedValue({ data: { id: "a1", status: "applied", sent: null,
      sent_error: "This is a staff member's chat; customer messages never go there.",
      label: "Cancelled TEST Tue 13 Oct 15:00; the message was NOT sent: This is a staff member's chat" } });
    const resolved = vi.fn();
    render(<ProposalCard proposal={{ kind: "nary_action", action: "meeting.cancel", payload: { meeting_id: "m1", reason: "test" },
      summary: "Cancel “TEST” Tue 13 Oct 15:00 (test); Google tells the invitees",
      message: { channel: "whatsapp", text: "Sorry, the meeting is cancelled.", recipient: "Diana", chat_id: "853@lid" } }}
      onResolved={resolved} />);
    expect(screen.getByText(/Sorry, the meeting is cancelled/)).toBeTruthy();
    expect(runAction).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: /Confirm/ }));
    await waitFor(() => expect(runAction).toHaveBeenCalledWith({ action: "meeting.cancel",
      payload: { meeting_id: "m1", reason: "test" }, message: expect.objectContaining({ chat_id: "853@lid" }) }));
    expect(await screen.findByText(/NOT sent: This is a staff member's chat/)).toBeTruthy();
    expect(resolved).toHaveBeenCalledWith("applied", expect.objectContaining({ id: "a1" }));
  });

  it("moves a reminder to the time on the card", async () => {
    snooze.mockResolvedValue({ data: {} });
    render(<ProposalCard proposal={{ kind: "reminder_change", reminder_id: "r1", action: "snooze",
      until: "2026-10-15T09:00:00+07:00", summary: "Move “Follow up David” to Thu 15 Oct 09:00" }} />);
    await userEvent.click(screen.getByRole("button", { name: /Confirm/ }));
    await waitFor(() => expect(snooze).toHaveBeenCalledWith("r1", { until: "2026-10-15T09:00:00+07:00" }));
  });

  it("shows a card Nary took back or replaced, with nothing to click", () => {
    const base = { kind: "customer_message" as const, channel: "whatsapp" as const, recipient: "Diana", text: "Hello" };
    const { rerender } = render(<ProposalCard proposal={{ ...base, status: "withdrawn" }} />);
    expect(screen.getByText(/Nary took this card back/)).toBeTruthy();
    expect(screen.queryByRole("button")).toBeNull();
    rerender(<ProposalCard proposal={{ ...base, status: "superseded" }} />);
    expect(screen.getByText(/Replaced by a newer card/)).toBeTruthy();
  });

  it("lists an email's attachments and sends them with it", async () => {
    send.mockResolvedValue({ data: { id: "c1", status: "applied", label: "Email" } });
    render(<ProposalCard proposal={{ kind: "customer_message", channel: "email", recipient: "nick@globex.example",
      to: "nick@globex.example", subject: "List prices", text: "Hi Nicholas",
      attach: [{ type: "quotation", id: "q1" }], attachments: ["Quotation Q-1.pdf"] }} />);
    expect(screen.getByText(/Quotation Q-1.pdf/)).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: /Send as Nary/ }));
    await waitFor(() => expect(send).toHaveBeenCalledWith(expect.objectContaining({ attach: [{ type: "quotation", id: "q1" }] })));
  });
});
