import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FilteredList, HeldChats, IdentityLinks } from "./IntakeLists";

describe("Nary intake lists", () => {
  it("asks business or personal about a held chat without showing its messages", async () => {
    const onDecide = vi.fn();
    render(<HeldChats canDecide onDecide={onDecide} rows={[{
      chat_id: "15550100@c.us", phone: "+15550100", name: "John", saved_on_phone: true, triage: null, last_inbound_at: null }]} />);
    expect(screen.getByText(/saved on the company phone/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Personal" }));
    expect(onDecide).toHaveBeenCalledWith("15550100@c.us", false);
    await userEvent.click(screen.getByRole("button", { name: "Business" }));
    expect(onDecide).toHaveBeenCalledWith("15550100@c.us", true);
  });

  it("lets the Owner correct a filtered verdict", async () => {
    const onCorrect = vi.fn();
    render(<FilteredList canCorrect onCorrect={onCorrect} rows={[{
      id: "v1", subject_type: "wa_chat", subject_ref: "x@c.us", verdict: "job_seeker", reason: "asks for a job",
      excerpt: "Ada lowongan magang?", cascaded: false, corrected_to: null, created_at: "2026-10-10T01:00:00Z" }]} />);
    expect(screen.getByText("Job seeker")).toBeInTheDocument();
    await userEvent.selectOptions(screen.getByLabelText("Correct the verdict"), "prospect");
    expect(onCorrect).toHaveBeenCalledWith("v1", "prospect");
  });

  it("offers an undo only for links still in force", async () => {
    const onUndo = vi.fn();
    const base = { chat_id: "c@c.us", score: 80, cascaded: true, review: "same company", target: "Robert <b@x.co>",
                   signals: [{ signal: "name_company", score: 75, detail: "name 100%" }], created_at: "2026-10-10T01:00:00Z" };
    render(<IdentityLinks canUndo onUndo={onUndo} rows={[
      { ...base, id: "l1", tier: "likely", status: "linked" }, { ...base, id: "l2", tier: "likely", status: "undone" }]} />);
    const undo = screen.getAllByRole("button", { name: /undo/i });
    expect(undo).toHaveLength(1);
    await userEvent.click(undo[0]);
    expect(onUndo).toHaveBeenCalledWith("l1");
  });
});
