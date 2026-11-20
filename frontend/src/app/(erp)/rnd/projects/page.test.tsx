import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";

import RNDProjectsPage from "./page";

const listProjects = vi.fn();
vi.mock("@/lib/api", () => ({
  rndApi: { listProjects: (...a: unknown[]) => listProjects(...a), createProject: vi.fn(), deleteProject: vi.fn() },
  catalogApi: {
    list: () => Promise.resolve({ data: [
      { code: "portal", name: "Customer order portal", color: "#2563eb" },
      { code: "kits", name: "Meeting-room kits", color: "#16a34a" },
    ] }),
  },
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/rnd/projects", useSearchParams: () => new URLSearchParams() }));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1", email: "a@b.c" } } }) }));

// What GET /rnd/projects returns: product_ref as stored (here upper case) and the per-status item counts
const PROJECTS = [
  { id: "p1", code: "OPV2", name: "Order portal v2", status: "active", product_ref: "PORTAL",
    backlog_counts: { todo: 3, done: 2 }, created_at: "2026-11-01T00:00:00Z" },
  { id: "p2", code: "MRK3", name: "Room kit 3", status: "active", product_ref: "kits",
    backlog_counts: { in_progress: 4 }, created_at: "2026-11-02T00:00:00Z" },
];

describe("R&D projects list", () => {
  beforeEach(() => {
    listProjects.mockReset().mockResolvedValue({ data: PROJECTS });
    window.history.replaceState({}, "", "/rnd/projects");
  });

  it("names each row's product line whatever the case of product_ref", async () => {
    render(<RNDProjectsPage />);
    expect((await screen.findAllByText("Order portal v2")).length).toBeGreaterThan(0);
    expect((await screen.findAllByText("Customer order portal")).length).toBeGreaterThan(0);
    expect(screen.getAllByText("Meeting-room kits").length).toBeGreaterThan(0);
  });

  it("totals the items of every project", async () => {
    render(<RNDProjectsPage />);
    await screen.findAllByText("Order portal v2");
    const tile = screen.getByText("Total items").parentElement!;
    expect(tile.textContent).toContain("9");
  });
});
