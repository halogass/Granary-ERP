import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

let sessionUser: Record<string, unknown> = { id: "u1" };
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: sessionUser } }) }));
const list = vi.fn();
vi.mock("@/lib/api", () => ({
  noteTemplatesApi: { list: (...a: unknown[]) => list(...a), create: vi.fn(), remove: vi.fn(() => Promise.resolve()) },
}));

import NoteTemplatePicker, { canManageTemplate } from "./NoteTemplatePicker";

const MINE = { id: "t1", name: "Mine", body: "<p>ok</p>", doc_type: "quotation", created_by_id: "u1" };
const THEIRS = { id: "t2", name: "Theirs", body: '<p>x</p><img src=x onerror="alert(1)">', doc_type: null, created_by_id: "u2" };

beforeEach(() => {
  sessionUser = { id: "u1" };
  list.mockReset().mockResolvedValue({ data: [MINE, THEIRS] });
});

describe("NoteTemplatePicker", () => {
  it("only the creator, the Owner and admins may manage a template", () => {
    expect(canManageTemplate(MINE, { id: "u1" })).toBe(true);
    expect(canManageTemplate(THEIRS, { id: "u1" })).toBe(false);
    expect(canManageTemplate(THEIRS, { id: "u1", is_superadmin: true })).toBe(true);
    expect(canManageTemplate(THEIRS, { id: "u1", is_system_admin: true })).toBe(true);
    expect(canManageTemplate({ ...THEIRS, created_by_id: undefined }, { id: "u1" })).toBe(false);
    expect(canManageTemplate({ ...THEIRS, can_edit: true }, { id: "u1" })).toBe(true);
  });

  it("shows Delete only on the user's own templates and inserts sanitized HTML", async () => {
    const onInsert = vi.fn();
    render(<NoteTemplatePicker docType="quotation" currentValue="" onInsert={onInsert} />);
    await userEvent.click(screen.getByRole("button", { name: /Insert saved note/ }));
    expect(await screen.findByText("Theirs")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Delete Mine" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Delete Theirs" })).toBeNull();
    await userEvent.click(screen.getByText("Theirs"));
    expect(onInsert).toHaveBeenCalledTimes(1);
    expect(onInsert.mock.calls[0][0]).not.toMatch(/onerror/i);
  });
});
