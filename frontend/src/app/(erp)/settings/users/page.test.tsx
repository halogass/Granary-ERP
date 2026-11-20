import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";

import UsersPage from "./page";

const create = vi.fn();
const assignRole = vi.fn();
const removeRole = vi.fn();
vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  usersApi: {
    list: () => Promise.resolve({ data: [
      { id: "u1", email: "ana@example.com", full_name: "Ana", is_active: true, is_superadmin: false, is_system_admin: false,
        portal_type: "internal", created_at: "2026-11-01T00:00:00Z", roles: ["sales_lead"] },
    ] }),
    create: (...a: unknown[]) => create(...a),
    update: vi.fn(), deactivate: vi.fn(), resetPassword: vi.fn(),
    assignRole: (...a: unknown[]) => assignRole(...a),
    removeRole: (...a: unknown[]) => removeRole(...a),
    totpStatus: () => Promise.resolve({ data: { enabled: false, confirmed_at: null, recovery_codes_left: 0 } }),
  },
  hrApi: { listEmployees: () => Promise.resolve({ data: [] }) },
  rolesApi: {
    list: () => Promise.resolve({ data: [
      { id: "r0", name: "superadmin", slug: "superadmin", description: null, is_system_role: true, permissions: ["system:superadmin"] },
      { id: "r1", name: "sales_lead", slug: "sales_lead", description: null, is_system_role: false, permissions: ["crm:view_own"] },
      { id: "r2", name: "engineer", slug: "engineer", description: null, is_system_role: true, permissions: ["crm:view_own"] },
      { id: "r3", name: "finance_admin", slug: "finance_admin", description: null, is_system_role: true, permissions: ["finance:manage_accounts"] },
    ] }),
  },
}));
const perms = { isAdmin: true, held: ["crm:view_own"] };
vi.mock("@/hooks/usePermissions", () => ({
  usePermissions: () => ({
    loaded: true, isAdmin: perms.isAdmin, isSuperadmin: false,
    holds: (c: string) => perms.held.includes(c), can: (c: string) => perms.isAdmin || perms.held.includes(c),
  }),
}));
vi.mock("@/components/nary/MyWhatsAppChats", () => ({ default: () => null }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/settings/users", useSearchParams: () => new URLSearchParams() }));

describe("Settings > Users: roles", () => {
  beforeEach(() => {
    create.mockReset(); assignRole.mockReset(); removeRole.mockReset();
    perms.isAdmin = true;
  });

  it("creates a user with the chosen roles, listing only roles the admin may grant", async () => {
    create.mockResolvedValue({ data: { id: "u2" } });
    render(<UsersPage />);
    fireEvent.click(await screen.findByRole("button", { name: /Add user/ }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByLabelText("engineer");
    expect(within(dialog).queryByLabelText("superadmin")).toBeNull();
    expect(within(dialog).queryByLabelText("finance_admin")).toBeNull();
    fireEvent.change(within(dialog).getByPlaceholderText("John Doe"), { target: { value: "Budi" } });
    fireEvent.change(within(dialog).getByPlaceholderText("john@example.com"), { target: { value: "budi@example.com" } });
    fireEvent.change(within(dialog).getByPlaceholderText("Min 6 characters"), { target: { value: "longpassword1" } });
    fireEvent.click(within(dialog).getByLabelText("engineer"));
    fireEvent.click(within(dialog).getByRole("button", { name: "Create user" }));
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    expect(create.mock.calls[0][0]).toEqual(expect.objectContaining({ email: "budi@example.com", role_names: ["engineer"] }));
  });

  it("assigns and removes roles on an existing user", async () => {
    assignRole.mockResolvedValue({ data: {} });
    removeRole.mockResolvedValue({ data: {} });
    render(<UsersPage />);
    fireEvent.click((await screen.findAllByRole("button", { name: "Edit user" }))[0]);
    const dialog = await screen.findByRole("dialog");
    const pick = await within(dialog).findByLabelText("Role to add");
    expect(Array.from((pick as HTMLSelectElement).options).map((o) => o.value)).toEqual(["", "engineer"]);
    fireEvent.change(pick, { target: { value: "engineer" } });
    fireEvent.click(within(dialog).getByRole("button", { name: /Add role/ }));
    await waitFor(() => expect(assignRole).toHaveBeenCalledWith("u1", "engineer"));
    fireEvent.click(await within(dialog).findByRole("button", { name: "Remove role sales_lead" }));
    await waitFor(() => expect(removeRole).toHaveBeenCalledWith("u1", "sales_lead"));
  });

  it("hides role editing and the Roles link from a user manager who is not a system admin", async () => {
    perms.isAdmin = false;
    render(<UsersPage />);
    fireEvent.click((await screen.findAllByRole("button", { name: "Edit user" }))[0]);
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByLabelText("Role to add")).toBeNull();
    expect(screen.queryByRole("link", { name: /Roles/ })).toBeNull();
  });
});
