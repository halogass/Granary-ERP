import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import ReplyPromptButton, { buildInstruction } from "./ReplyPromptButton";
import { draftToHtml, withDraft } from "@/lib/replyDraft";

describe("drafting a reply with directions", () => {
  it("sends what the user asked for", async () => {
    const onGenerate = vi.fn().mockResolvedValue(undefined);
    render(<ReplyPromptButton onGenerate={onGenerate} />);
    await userEvent.click(screen.getByText("Suggest"));
    await userEvent.type(screen.getByPlaceholderText(/Confirm delivery/), "Offer a 5% discount if they sign this week");
    await userEvent.click(screen.getByText("Keep it short"));
    await userEvent.click(screen.getByText("Draft reply"));
    await waitFor(() => expect(onGenerate).toHaveBeenCalledWith(
      "Offer a 5% discount if they sign this week. Keep it short"));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("drafts without directions when the box is left empty", async () => {
    const onGenerate = vi.fn().mockResolvedValue(undefined);
    render(<ReplyPromptButton variant="dark" onGenerate={onGenerate} />);
    await userEvent.click(screen.getByLabelText("Draft a reply with AI"));
    await userEvent.click(screen.getByText("Draft reply"));
    await waitFor(() => expect(onGenerate).toHaveBeenCalledWith(""));
  });

  it("offers to start from what the user already typed", async () => {
    const onGenerate = vi.fn().mockResolvedValue(undefined);
    render(<ReplyPromptButton onGenerate={onGenerate} currentDraft="hi, is 10k ok for the price?" />);
    await userEvent.click(screen.getByText("Suggest"));
    expect((screen.getByLabelText(/Start from what I/) as HTMLInputElement).checked).toBe(true);
    await userEvent.click(screen.getByText("Draft reply"));
    await waitFor(() => expect(onGenerate.mock.calls[0][0]).toContain("hi, is 10k ok for the price?"));
  });

  it("keeps the panel open with the reason when drafting fails", async () => {
    const onGenerate = vi.fn().mockRejectedValue(new Error("No AI API key configured"));
    render(<ReplyPromptButton onGenerate={onGenerate} />);
    await userEvent.click(screen.getByText("Suggest"));
    await userEvent.click(screen.getByText("Draft reply"));
    expect(await screen.findByText("No AI API key configured")).toBeTruthy();
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("builds the instruction", () => {
    expect(buildInstruction("  ", "draft", false)).toBe("");
    expect(buildInstruction("Be formal", "", true)).toBe("Be formal");
    expect(buildInstruction("", "my text", true)).toContain('"""\nmy text\n"""');
  });
});

describe("AI draft into the email editor", () => {
  it("keeps paragraphs and line breaks, and escapes HTML", () => {
    expect(draftToHtml("Dear Robert,\n\nThanks <3\nRegards")).toBe("<p>Dear Robert,</p><p>Thanks &lt;3<br>Regards</p>");
  });

  it("replaces the typed text but keeps the signature", () => {
    const sig = '<div data-granary-signature="1"><p>John · Your Company</p></div>';
    const out = withDraft(`<p>old text</p><p><br></p>${sig}`, "New reply");
    expect(out).toBe(`<p>New reply</p><p><br></p>${sig}`);
    expect(withDraft("<p>old</p>", "Hi")).toBe("<p>Hi</p>");
  });
});
