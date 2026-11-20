import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { TASK_PRIORITY_LABELS, TASK_STATUS_LABELS } from "./projectTasks";

describe("project task statuses and priorities", () => {
  it("use the API's values", () => {
    expect(Object.keys(TASK_STATUS_LABELS)).toEqual(["todo", "in_progress", "review", "done", "cancelled"]);
    expect(Object.keys(TASK_PRIORITY_LABELS)).toEqual(["low", "medium", "high", "critical"]);
  });

  it("are the only lists on My Tasks and the project page (no blocked/urgent)", () => {
    for (const page of ["projects/tasks/page.tsx", "projects/[id]/page.tsx"]) {
      const src = readFileSync(join(__dirname, "..", "app", "(erp)", page), "utf8");
      expect(src).toContain("@/lib/projectTasks");
      expect(src).not.toMatch(/\bblocked:\s*"/);
      expect(src).not.toMatch(/\burgent:\s*"/);
    }
  });
});
