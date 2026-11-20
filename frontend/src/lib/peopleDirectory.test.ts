import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

// Batch E: people pickers staff use read the staff directory (GET /messaging/people, any internal user),
// not the admin users list (GET /users, system:manage_users only), which refused them.
const PICKERS = [
  "components/pm/KanbanBoard.tsx",
  "components/documents/DocumentDialogs.tsx",
  "app/(erp)/nary/page.tsx",
  "app/(erp)/crm/deals/page.tsx",
  "app/(erp)/projects/[id]/page.tsx",
  "app/(erp)/messaging/page.tsx",
];

describe("people pickers", () => {
  it("use the staff directory, not the admin users list", () => {
    for (const f of PICKERS) {
      const src = readFileSync(join(__dirname, "..", f), "utf8");
      expect(src, f).not.toMatch(/usersApi\.list/);
      expect(src, f).toMatch(/peopleApi\.directory\(\)|messagingApi\.people\(\)/);
    }
  });

  it("the directory is GET /messaging/people", () => {
    const api = readFileSync(join(__dirname, "api.ts"), "utf8");
    expect(api).toContain('directory: () => api.get<StaffPerson[]>("/messaging/people")');
  });
});
