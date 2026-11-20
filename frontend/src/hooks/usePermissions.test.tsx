import { describe, it, expect, vi } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";

vi.mock("@/lib/api", () => ({
  authApi: { me: vi.fn().mockResolvedValue({ data: { is_superadmin: false, permissions: ["sales:view_costs"] } }) },
}));

import { usePermissions } from "./usePermissions";

describe("usePermissions", () => {
  it("keeps can and holds stable between renders (pages use them in effect deps)", async () => {
    const hook = renderHook(() => usePermissions());
    await waitFor(() => expect(hook.result.current.loaded).toBe(true));
    const { can, holds } = hook.result.current;
    hook.rerender();
    hook.rerender();
    expect(hook.result.current.can).toBe(can);
    expect(hook.result.current.holds).toBe(holds);
    expect(can("sales:view_costs")).toBe(true);
    expect(can("documents:manage_all")).toBe(false);
  });
});
