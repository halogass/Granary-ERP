import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import ProposalCard from "./ProposalCard";

const send = vi.fn();
const download = vi.fn();
vi.mock("@/lib/api", () => ({
  naryApi: { send: (...a: unknown[]) => send(...a) },
  attachmentApi: { download: (...a: unknown[]) => download(...a) },
  researchApi: {}, salesApi: {}, financeApi: {}, kbApi: {}, remindersApi: {},
}));
vi.mock("next/link", () => ({
  default: ({ href, children }: { href: string; children: React.ReactNode }) => <a href={href}>{children}</a>,
}));

const EMAIL = { kind: "customer_message" as const, channel: "email" as const, recipient: "diana@northwind.example",
  to: "diana@northwind.example", thread_id: "t1", subject: "Re: Follow-Up on Video Analytics",
  text: "Dear Mr. Smith,\n\nThank you for following up.\n\nBest regards," };

// No mockReset here: with this Vitest a reset mock reports the card's handled rejection as a failure.
// vitest.setup.ts clears the calls after each test; every test sets its own result.

describe("send-as-Nary card in the chat", () => {
  it("shows the exact text and sends nothing until Send as Nary is clicked", async () => {
    send.mockResolvedValue({ data: { id: "c1", status: "applied", label: "Email to diana@northwind.example" } });
    const resolved = vi.fn();
    render(<ProposalCard proposal={EMAIL} onResolved={resolved} />);
    expect(screen.getByText(/Thank you for following up/)).toBeTruthy();
    expect(screen.getByText(/Subject: Re: Follow-Up/)).toBeTruthy();
    expect(send).not.toHaveBeenCalled();
    await userEvent.click(screen.getByText("Send as Nary"));
    await waitFor(() => expect(send).toHaveBeenCalledWith(expect.objectContaining({
      channel: "email", thread_id: "t1", to: "diana@northwind.example", text: EMAIL.text })));
    expect(await screen.findByText(/Sent by email to diana@northwind.example, as Nary/)).toBeTruthy();
    expect(resolved).toHaveBeenCalledWith("applied", expect.objectContaining({ id: "c1" }));
  });

  it("shows why it could not be sent", async () => {
    send.mockRejectedValue({ response: { data: { detail: "Nary only replies in chats the customer started." } } });
    render(<ProposalCard proposal={{ ...EMAIL, channel: "whatsapp", chat_id: "1555@c.us", recipient: "Nicholas" }} />);
    expect(screen.queryByText(/Subject:/)).toBeNull();
    await userEvent.click(screen.getByText("Send as Nary"));
    await waitFor(() => expect(screen.getByText(/customer started/)).toBeTruthy());
  });

  it("can be cancelled", async () => {
    render(<ProposalCard proposal={EMAIL} />);
    await userEvent.click(screen.getByText("Cancel"));
    expect(screen.getByText("Not sent.")).toBeTruthy();
    expect(send).not.toHaveBeenCalled();
  });

  it("shows From, Cc and Bcc as they will be sent and passes them to the send", async () => {
    send.mockResolvedValue({ data: { id: "c2", status: "applied" } });
    render(<ProposalCard proposal={{ ...EMAIL, from_inbox: "sales@example.com",
      cc: ["rachel@northwind.example"], bcc: ["owner@example.com"] }} />);
    expect(screen.getByText("From: sales@example.com")).toBeTruthy();
    expect(screen.getByText("To: diana@northwind.example")).toBeTruthy();
    expect(screen.getByText("Cc: rachel@northwind.example")).toBeTruthy();
    expect(screen.getByText("Bcc: owner@example.com")).toBeTruthy();
    await userEvent.click(screen.getByText("Send as Nary"));
    await waitFor(() => expect(send).toHaveBeenCalledWith(expect.objectContaining({
      cc: ["rachel@northwind.example"], bcc: ["owner@example.com"], from_inbox: "sales@example.com",
      subject: "Re: Follow-Up on Video Analytics" })));
  });

  it("leaves out empty Cc and Bcc lines", () => {
    render(<ProposalCard proposal={{ ...EMAIL, cc: [], bcc: [] }} />);
    expect(screen.queryByText(/^Cc:/)).toBeNull();
    expect(screen.queryByText(/^Bcc:/)).toBeNull();
  });

  it("lists every file with its size and type and shows a picture on a record as a thumbnail", async () => {
    download.mockResolvedValue({ data: new Blob(["png"], { type: "image/png" }) });
    const created = vi.fn(() => "blob:thumb");
    const revoke = vi.fn();
    Object.assign(URL, { createObjectURL: created, revokeObjectURL: revoke });
    render(<ProposalCard proposal={{ ...EMAIL, channel: "whatsapp", chat_id: "1555@c.us", recipient: "Robert",
      attach: [{ type: "file", id: "f-png" }, { type: "file", id: "f-pdf" }],
      attachments: ["Office floor plan.png (240 KB, PNG image)", "Site survey.pdf (1000 B, PDF)"],
      files: [
        { id: "f-png", name: "Office floor plan.png", size: 245760, size_label: "240 KB", mime_type: "image/png",
          type: "PNG image", picture: true, source: "attachment", thumbnail_url: "/api/v1/attachments/f-png/download" },
        { id: "f-pdf", name: "Site survey.pdf", size: 1000, size_label: "1000 B", mime_type: "application/pdf",
          type: "PDF", picture: false, source: "attachment", thumbnail_url: null },
      ] }} />);
    expect(screen.getByText(/Office floor plan.png \(240 KB, PNG image\)/)).toBeTruthy();
    expect(screen.getByText(/Site survey.pdf \(1000 B, PDF\)/)).toBeTruthy();
    await waitFor(() => expect(screen.getByRole("img", { name: "Office floor plan.png" })
      .getAttribute("src")).toBe("blob:thumb"));
    // the thumbnail comes through the authenticated download, only for the picture
    expect(download).toHaveBeenCalledTimes(1);
    expect(download).toHaveBeenCalledWith("f-png");
    expect(screen.getByTitle(/sent as a photo/)).toBeTruthy();
  });

  // 1 Nov 2026: videos show size and length, and how each goes
  it("shows a video with its size, length and type, and how it goes on WhatsApp and email", () => {
    const video = { id: "f-mp4", name: "Tutorial.mp4", size: 61_000_000, size_label: "58.2 MB", mime_type: "video/mp4",
      type: "MP4 video", picture: false, video: true, duration_seconds: 60.2, duration_label: "1:00",
      source: "attachment", thumbnail_url: null };
    const { unmount } = render(<ProposalCard proposal={{ ...EMAIL, channel: "whatsapp", chat_id: "1555@c.us",
      recipient: "Robert", attach: [{ type: "file", id: "f-mp4" }],
      files: [{ ...video, send_as: "document" as const }] }} />);
    const list = screen.getByRole("list", { name: "Videos" });
    expect(list.textContent).toContain("Tutorial.mp4 (58.2 MB, 1:00, MP4 video)");
    expect(list.textContent).toContain("goes as a WhatsApp document");
    unmount();
    render(<ProposalCard proposal={{ ...EMAIL, attach: [{ type: "file", id: "f-mp4" }],
      files: [{ ...video, size: 9_000_000, size_label: "8.6 MB", send_as: "link" as const }] }} />);
    expect(screen.getByRole("list", { name: "Videos" }).textContent)
      .toContain("Tutorial.mp4 (8.6 MB, 1:00, MP4 video): goes as a download link in the email");
    // no thumbnail fetch for a video
    expect(download).not.toHaveBeenCalled();
  });
});
