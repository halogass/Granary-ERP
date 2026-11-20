import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

import ContentCalendarPage from "./page";

const updateContentStatus = vi.fn(() => Promise.resolve({ data: {} }));
vi.mock("@/lib/api", () => ({
  marketingApi: {
    listContent: () => Promise.resolve({ data: [
      { id: "c1", title: "Launch post", content_type: "social_post", platform: "linkedin", status: "review" },
      { id: "c2", title: "Draft article", content_type: "blog_post", platform: "website", status: "in_progress" },
    ] }),
    updateContentStatus: (...a: unknown[]) => updateContentStatus(...(a as [])),
  },
}));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1", email: "a@b.c" } } }) }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/marketing/content", useSearchParams: () => new URLSearchParams() }));
vi.mock("@/components/approvals/ApprovalPanel", () => ({
  default: ({ entityType, entityId }: { entityType: string; entityId: string }) => <div>approval panel {entityType} {entityId}</div>,
}));

// Batch E: the API refuses status "approved" (only the approval flow sets it), so an item in review offers no
// "Approved" button; its details carry the approval panel instead.
describe("content calendar approval", () => {
  it("offers no Approved button on an item in review", async () => {
    render(<ContentCalendarPage />);
    await screen.findAllByText("Launch post");
    expect(screen.queryByRole("button", { name: /^approved$/i })).toBeNull();
    // An item in progress still moves on to review.
    expect(screen.getAllByRole("button", { name: /in review/i }).length).toBeGreaterThan(0);
  });

  it("shows the approval panel in the item's details", async () => {
    render(<ContentCalendarPage />);
    const title = await screen.findAllByText("Launch post");
    fireEvent.click(title[0]);
    expect(await screen.findByText("approval panel marketing_content c1")).toBeInTheDocument();
  });
});
