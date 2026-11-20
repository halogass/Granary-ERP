import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import RolesPage from "./page";

const list = vi.fn();
const create = vi.fn();
const update = vi.fn();
vi.mock("@/lib/api", () => ({
  rolesApi: {
    list: () => list(),
    permissions: () => Promise.resolve({ data: [
      { code: "crm:view_own", name: "View own CRM records", description: null },
      { code: "crm:view_all", name: "View all CRM records", description: null },
      { code: "finance:manage_accounts", name: "Manage accounts", description: null },
    ] }),
    create: (...a: unknown[]) => create(...a),
    update: (...a: unknown[]) => update(...a),
  },
}));
const perms = { isAdmin: true, isSuperadmin: false, loaded: true, held: ["crm:view_own", "crm:view_all"] };
vi.mock("@/hooks/usePermissions", () => ({
  usePermissions: () => ({
    loaded: perms.loaded, isAdmin: perms.isAdmin, isSuperadmin: perms.isSuperadmin,
    holds: (c: string) => perms.held.includes(c), can: (c: string) => perms.isAdmin || perms.held.includes(c),
  }),
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/settings/roles", useSearchParams: () => new URLSearchParams() }));

const ROLES = [
  { id: "r1", name: "executive", slug: "executive", description: "Default Executive role", is_system_role: true, permissions: ["crm:view_all"] },
  { id: "r2", name: "sales_lead", slug: "sales_lead", description: "Leads", is_system_role: false, permissions: ["crm:view_own"] },
];

describe("Settings > Roles", () => {
  beforeEach(() => {
    list.mockReset(); create.mockReset(); update.mockReset();
    list.mockResolvedValue({ data: ROLES });
    perms.isAdmin = true; perms.isSuperadmin = false;
  });

  it("lists roles and edits only custom ones", async () => {
    render(<RolesPage />);
    expect((await screen.findAllByText("sales_lead")).length).toBeGreaterThan(0);
    expect(screen.getAllByText("executive").length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: "Edit role executive" })).toBeNull();
    expect(screen.getAllByRole("button", { name: "Edit role sales_lead" }).length).toBeGreaterThan(0);
  });

  it("creates a role with the ticked permissions; permissions the admin lacks are disabled", async () => {
    create.mockResolvedValue({ data: { id: "r3", name: "support_desk" } });
    render(<RolesPage />);
    fireEvent.click(await screen.findByRole("button", { name: /New role/ }));
    fireEvent.change(screen.getByPlaceholderText("sales_lead"), { target: { value: "support_desk" } });
    const finance = await screen.findByLabelText(/finance:manage_accounts/);
    expect((finance as HTMLInputElement).disabled).toBe(true);
    fireEvent.click(screen.getByLabelText(/crm:view_all/));
    fireEvent.click(screen.getByRole("button", { name: "Create role" }));
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    expect(create.mock.calls[0][0]).toEqual({ name: "support_desk", description: undefined, permission_codes: ["crm:view_all"] });
  });

  it("saves a custom role's whole permission set", async () => {
    update.mockResolvedValue({ data: { id: "r2", name: "sales_lead" } });
    render(<RolesPage />);
    fireEvent.click((await screen.findAllByRole("button", { name: "Edit role sales_lead" }))[0]);
    fireEvent.click(await screen.findByLabelText(/crm:view_all/));
    fireEvent.click(screen.getByLabelText(/crm:view_own/));
    fireEvent.click(screen.getByRole("button", { name: "Save role" }));
    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    expect(update.mock.calls[0]).toEqual(["r2", { description: "Leads", permission_codes: ["crm:view_all"] }]);
  });

  it("shows no create or edit actions to a non-admin", async () => {
    perms.isAdmin = false;
    render(<RolesPage />);
    expect((await screen.findAllByText("sales_lead")).length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: /New role/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Edit role sales_lead" })).toBeNull();
  });
});
