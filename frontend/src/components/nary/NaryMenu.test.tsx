import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";

let path = "/nary/chat";
const replace = vi.fn();
vi.mock("next/navigation", () => ({ usePathname: () => path, useRouter: () => ({ replace }) }));
let me: { is_superadmin?: boolean; is_system_admin?: boolean; permissions?: string[] } = {};
vi.mock("@/hooks/usePermissions", () => ({
  usePermissions: () => ({
    loaded: true, isAdmin: !!(me.is_superadmin || me.is_system_admin), can: () => false,
    holds: (c: string) => !!me.is_superadmin || !!me.permissions?.includes(c),
  }),
}));

import NaryTabs from "./NaryTabs";
import NaryLayout from "@/app/(erp)/nary/layout";

describe("one Nary menu", () => {
  beforeEach(() => { replace.mockReset(); path = "/nary/chat"; });

  it("staff see only the Chat tab", () => {
    me = { permissions: ["crm:view_all"] };
    render(<NaryTabs />);
    expect(screen.getByRole("link", { name: "Chat" })).toBeTruthy();
    expect(screen.queryByRole("link", { name: "Overview" })).toBeNull();
    expect(screen.queryByRole("link", { name: "Approvals" })).toBeNull();
  });

  it("the Owner (nary:view) sees Chat and every agent tab", () => {
    me = { permissions: ["nary:view"] };
    render(<NaryTabs />);
    for (const name of ["Chat", "Overview", "Approvals", "Meetings", "Autonomy"])
      expect(screen.getByRole("link", { name })).toBeTruthy();
  });

  it("staff opening another Nary page are sent to the chat, without rendering it", async () => {
    me = { permissions: [] };
    path = "/nary/approvals";
    render(<NaryLayout><p>secret queue</p></NaryLayout>);
    expect(screen.queryByText("secret queue")).toBeNull();
    await waitFor(() => expect(replace).toHaveBeenCalledWith("/nary/chat"));
  });

  it("the work queue: the Owner (nary:approve) and system admins, not staff with nary:view only (3 Nov 2026)", async () => {
    me = { permissions: ["nary:view"] };
    const { unmount } = render(<NaryTabs />);
    expect(screen.queryByRole("link", { name: "Work queue" })).toBeNull();
    unmount();
    me = { permissions: ["nary:view", "nary:approve"] };
    render(<NaryTabs />);
    expect(screen.getByRole("link", { name: "Work queue" })).toBeTruthy();
    me = { is_system_admin: true, permissions: [] };
    path = "/nary/queue";
    render(<NaryLayout><p>the queue</p></NaryLayout>);
    expect(screen.getByText("the queue")).toBeTruthy();
    expect(replace).not.toHaveBeenCalled();
  });

  it("the chat itself is open to everyone", () => {
    me = { permissions: [] };
    render(<NaryLayout><p>chat</p></NaryLayout>);
    expect(screen.getByText("chat")).toBeTruthy();
    expect(replace).not.toHaveBeenCalled();
  });
});
