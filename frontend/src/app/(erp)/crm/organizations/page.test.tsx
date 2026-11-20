import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1" } } }) }));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  usePathname: () => "/crm/organizations",
  useSearchParams: () => new URLSearchParams(),
}));
vi.mock("@/lib/api", () => ({
  crmApi: {
    listOrgs: () => Promise.resolve({ data: { items: [
      { id: "o1", name: "Evil Co", website: "javascript:alert(document.cookie)", created_at: "2026-10-01T00:00:00Z" },
      { id: "o2", name: "Acme", website: "acme.example", created_at: "2026-10-01T00:00:00Z" },
    ] } }),
  },
  researchApi: {},
}));
vi.mock("@/components/ui/AttachmentPanel", () => ({ default: () => null }));
vi.mock("@/components/crm/AiSelectionBar", () => ({ default: () => null }));

import OrganizationsPage from "./page";

describe("organizations list: website links", () => {
  it("never renders a javascript: website as a link", async () => {
    render(<OrganizationsPage />);
    expect((await screen.findAllByText("Acme")).length).toBeGreaterThan(0);
    const hrefs = Array.from(document.querySelectorAll("a")).map((a) => a.getAttribute("href") ?? "");
    expect(hrefs.some((h) => /^\s*javascript:/i.test(h))).toBe(false);
    expect(screen.queryByLabelText("Website of Evil Co")).toBeNull();
    expect(screen.getByLabelText("Website of Acme").getAttribute("href")).toBe("https://acme.example");
  });
});
