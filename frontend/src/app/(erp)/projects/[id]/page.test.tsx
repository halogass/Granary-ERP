import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";

import ProjectDetailPage from "./page";

const get = vi.fn();
const update = vi.fn();
const addMember = vi.fn();
const removeMember = vi.fn();
const updateMember = vi.fn();
let perms: string[] = [];

vi.mock("@/lib/api", () => ({
  projectsApi: {
    get: (...a: unknown[]) => get(...a),
    listMilestones: () => Promise.resolve({ data: [] }),
    update: (...a: unknown[]) => update(...a),
    addMember: (...a: unknown[]) => addMember(...a),
    removeMember: (...a: unknown[]) => removeMember(...a),
    updateMember: (...a: unknown[]) => updateMember(...a),
  },
  messagingApi: {
    people: () => Promise.resolve({ data: [
      { id: "u1", name: "Ada Lovelace", department: "Engineering", position: "Lead" },
      { id: "u2", name: "Grace Hopper", department: "Engineering", position: "Engineer" },
    ] }),
  },
}));
vi.mock("@/lib/entitySources", () => ({
  orgSource: { fetch: () => Promise.resolve([]) },
  localSource: (options: { id: string; label: string }[]) => ({
    fetch: () => Promise.resolve(options),
    resolve: (id: string) => Promise.resolve(options.find((o) => o.id === id) ?? null),
  }),
}));
vi.mock("@/hooks/usePermissions", () => ({
  usePermissions: () => ({
    loaded: true, isAdmin: false, isSuperadmin: false,
    can: (c: string) => perms.includes(c), holds: (c: string) => perms.includes(c),
  }),
}));
vi.mock("@/hooks/useCurrencySettings", () => ({ useCurrencySettings: () => ({ functional: "USD" }) }));
vi.mock("@/components/pm/KanbanBoard", () => ({ default: () => null }));
vi.mock("@/components/ui/AttachmentPanel", () => ({ default: () => null }));
vi.mock("next/navigation", () => ({
  useParams: () => ({ id: "p1" }), useRouter: () => ({ push: vi.fn() }),
  usePathname: () => "/projects/p1", useSearchParams: () => new URLSearchParams(),
}));

const PROJECT = {
  id: "p1", code: "PRJ-1", name: "Warehouse rollout", status: "planning", currency: "USD", budget: 1000,
  description: "Phase one", start_date: "2026-11-01", end_date: "2026-12-01",
  customer_org_id: null, customer_org: null, pm_id: "u1",
  pm_user: { id: "u1", full_name: "Ada Lovelace", email: "ada@example.com" },
  members: [
    { user_id: "u1", full_name: "Ada Lovelace", email: "ada@example.com", role: "project_manager", allocation_percentage: 100 },
    { user_id: "u2", full_name: "Grace Hopper", email: "grace@example.com", role: "member", allocation_percentage: 50 },
  ],
  tasks: [],
};

describe("project page: edit project and team", () => {
  beforeEach(() => {
    get.mockReset(); update.mockReset(); addMember.mockReset(); removeMember.mockReset(); updateMember.mockReset();
    updateMember.mockResolvedValue({ data: {} });
    get.mockResolvedValue({ data: PROJECT });
    update.mockResolvedValue({ data: { id: "p1" } });
    removeMember.mockResolvedValue({});
  });

  it("hides edit and team actions without projects:edit", async () => {
    perms = [];
    render(<ProjectDetailPage />);
    expect(await screen.findByText("Warehouse rollout")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Edit project/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /Add member/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /Remove Grace Hopper/ })).toBeNull();
  });

  it("saves status and the other fields through PATCH /projects/{id}", async () => {
    perms = ["projects:edit"];
    render(<ProjectDetailPage />);
    fireEvent.click(await screen.findByRole("button", { name: /Edit project/ }));
    const dialog = await screen.findByRole("dialog");
    const status = within(dialog).getAllByRole("combobox")
      .find((s) => Array.from((s as HTMLSelectElement).options ?? []).some((o) => o.value === "on_hold")) as HTMLSelectElement;
    fireEvent.change(status, { target: { value: "active" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    expect(update).toHaveBeenCalledWith("p1", expect.objectContaining({
      name: "Warehouse rollout", status: "active", pm_id: "u1", customer_org_id: null, budget: "1000",
    }));
  });

  it("removes a team member, but never the project manager", async () => {
    perms = ["projects:edit"];
    vi.spyOn(window, "confirm").mockReturnValue(true);
    render(<ProjectDetailPage />);
    await screen.findByText("Warehouse rollout");
    fireEvent.mouseDown(screen.getByRole("tab", { name: /Team/ }));
    fireEvent.click(screen.getByRole("tab", { name: /Team/ }));
    const remove = await screen.findByRole("button", { name: "Remove Grace Hopper" });
    expect(screen.queryByRole("button", { name: "Remove Ada Lovelace" })).toBeNull();
    fireEvent.click(remove);
    await waitFor(() => expect(removeMember).toHaveBeenCalledWith("p1", "u2"));
  });

  it("has no tab without content (the old Time tab)", async () => {
    perms = [];
    render(<ProjectDetailPage />);
    await screen.findByText("Warehouse rollout");
    const names = screen.getAllByRole("tab").map((t) => t.textContent ?? "");
    expect(names.some((n) => /^Time/.test(n))).toBe(false);
  });

  it("edits a member's role and allocation in place; the PM keeps the role", async () => {
    perms = ["projects:edit"];
    render(<ProjectDetailPage />);
    await screen.findByText("Warehouse rollout");
    fireEvent.mouseDown(screen.getByRole("tab", { name: /Team/ }));
    fireEvent.click(screen.getByRole("tab", { name: /Team/ }));

    fireEvent.click(await screen.findByRole("button", { name: "Edit Grace Hopper" }));
    let dialog = await screen.findByRole("dialog");
    const role = within(dialog).getAllByRole("combobox")
      .find((s) => Array.from((s as HTMLSelectElement).options ?? []).some((o) => o.value === "tech_lead")) as HTMLSelectElement;
    fireEvent.change(role, { target: { value: "tech_lead" } });
    const alloc = within(dialog).getByRole("spinbutton");
    fireEvent.change(alloc, { target: { value: "" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(updateMember).toHaveBeenCalledWith("p1", "u2", { role: "tech_lead", allocation_percentage: null }));

    fireEvent.click(await screen.findByRole("button", { name: "Edit Ada Lovelace" }));
    dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByRole("spinbutton"), { target: { value: "40" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(updateMember).toHaveBeenCalledWith("p1", "u1", { allocation_percentage: 40 }));
  });
});
