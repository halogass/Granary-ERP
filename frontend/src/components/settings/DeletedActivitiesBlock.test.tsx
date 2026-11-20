import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const listDeleted = vi.fn();
const restore = vi.fn();
let me: Record<string, unknown> = { is_superadmin: true };
vi.mock("next/link", () => ({ default: ({ children, href }: { children: React.ReactNode; href: string }) => <a href={href}>{children}</a> }));
vi.mock("@/lib/api", () => ({
  crmApi: {
    listDeletedActivities: (...a: unknown[]) => listDeleted(...a),
    restoreDeletedActivity: (...a: unknown[]) => restore(...a),
  },
  authApi: { me: () => Promise.resolve({ data: me }) },
}));

import DeletedActivitiesBlock from "./DeletedActivitiesBlock";

const ROW = {
  id: "v1", activity_id: "a1", type: "call", subject: "Called about the pilot", entity_type: "lead", entity_id: "l1",
  reason: "Logged on the wrong lead", voided_by: "Rachel", voided_at: "2026-10-26T03:00:00Z", restored_at: null,
};

beforeEach(() => {
  listDeleted.mockResolvedValue({ data: { items: [
    ROW,
    { ...ROW, id: "v2", activity_id: "a2", subject: "Old note", type: "note", entity_type: "deal", entity_id: "d1",
      reason: null, restored_at: "2026-10-20T03:00:00Z" },
  ] } });
  restore.mockResolvedValue({ data: { activity_id: "a1", restored: true } });
});

describe("deleted activities", () => {
  it("lists them with a record link and restores one", async () => {
    render(<DeletedActivitiesBlock />);
    expect(await screen.findByText("Called about the pilot")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Lead" })).toHaveAttribute("href", "/crm/leads/l1");
    expect(screen.getByRole("link", { name: "Deal" })).toHaveAttribute("href", "/crm/deals/d1");
    expect(screen.getByText("Reason: Logged on the wrong lead")).toBeInTheDocument();
    expect(screen.getAllByText(/Deleted by Rachel/)).toHaveLength(2);
    expect(screen.getByText(/^Restored /)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Restore" }));
    expect(restore).toHaveBeenCalledWith("v1");
    await waitFor(() => expect(screen.queryByRole("button", { name: "Restore" })).toBeNull());
    expect(screen.getAllByText(/^Restored /)).toHaveLength(2);
  });
});
