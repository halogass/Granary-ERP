import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import SendDocumentModal from "./SendDocumentModal";

const send = vi.fn();
const mailboxes = vi.fn(() => Promise.resolve({ data: { mailboxes: [] as unknown[] } }));
vi.mock("@/lib/api", () => ({
  documentsApi: { send: (...a: unknown[]) => send(...a) },
  crmInboxApi: { mailboxes: () => mailboxes() },
  whatsappApi: { getConversations: () => Promise.resolve({ data: [] }) },
}));

const DEFAULTS = { name: "Rachel", email: "rachel@cust.test", whatsapp: "", approved: true, number: "INV-2026-001" };

function open(onSent = vi.fn()) {
  render(<SendDocumentModal entityType="invoice" entityId="inv-1" label="Invoice" defaults={DEFAULTS}
    onClose={() => {}} onSent={onSent} />);
  return onSent;
}

async function reviewAndConfirm() {
  await userEvent.click(await screen.findByRole("button", { name: /review/i }));
  await userEvent.click(screen.getByRole("button", { name: /confirm & send/i }));
}

beforeEach(() => send.mockReset());

describe("sending a document", () => {
  it("asks before sending the same document to the same recipient again", async () => {
    send.mockRejectedValueOnce({ response: { status: 409, data: { detail: {
      code: "recently_sent", sent_at: "2026-09-22T07:02:00Z", to: ["rachel@cust.test"],
      message: "This invoice was already sent to rachel@cust.test a few minutes ago.",
    } } } });
    send.mockResolvedValueOnce({ data: {} });
    const onSent = open();

    await reviewAndConfirm();
    expect(await screen.findByText(/was already sent to/i)).toBeInTheDocument();
    expect(send).toHaveBeenCalledTimes(1);
    expect(send.mock.calls[0][2]).not.toHaveProperty("force");

    await userEvent.click(screen.getByRole("button", { name: /send again/i }));
    expect(send).toHaveBeenCalledTimes(2);
    expect(send.mock.calls[1][2]).toMatchObject({ force: true, to: ["rachel@cust.test"] });
    expect(onSent).toHaveBeenCalled();
  });

  it("warns that a timed-out send may already have gone out", async () => {
    send.mockRejectedValueOnce({ response: { status: 504, data: "<html>Gateway Time-out</html>" } });
    open();
    await reviewAndConfirm();
    expect(await screen.findByText(/may still have been sent/i)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /send again/i })).not.toBeInTheDocument();
  });

  it("shows the server's reason for other failures", async () => {
    send.mockRejectedValueOnce({ response: { status: 502, data: {
      detail: "The PDF service could not render the invoice right now." } } });
    open();
    await reviewAndConfirm();
    expect(await screen.findByText(/PDF service could not render/i)).toBeInTheDocument();
  });

  it("sends Bcc and the chosen From inbox; the deal's inbox is null", async () => {
    mailboxes.mockResolvedValueOnce({ data: { mailboxes: [
      { id: "m1", address: "hello@example.com", name: "Your Company", purpose: "general", active: true, default: true },
      { id: "m2", address: "sales@example.com", name: "Your Company Sales", purpose: "sales", active: true, default: false },
    ] } });
    send.mockResolvedValue({ data: {} });
    open();
    await userEvent.type(screen.getByLabelText("Bcc"), "boss@example.com");
    await reviewAndConfirm();
    expect(send.mock.calls[0][2]).toMatchObject({ bcc: ["boss@example.com"], from_inbox: null });

    send.mockClear();
    await userEvent.selectOptions(await screen.findByLabelText("From"), "sales@example.com");
    await reviewAndConfirm();
    expect(send.mock.calls[0][2]).toMatchObject({ from_inbox: "sales@example.com" });
  });
});
