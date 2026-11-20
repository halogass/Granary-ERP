import { describe, it, expect } from "vitest";
import { isMyLead } from "./leadOwner";

// Batch E: the Leads "Mine" view used created_by_id while the API's own scope is assigned_to_id.
describe("leads: Mine", () => {
  it("is the lead assigned to me, whoever created it", () => {
    expect(isMyLead({ assigned_to_id: "u1" }, "u1")).toBe(true);
    expect(isMyLead({ assigned_to_id: "u2", created_by_id: "u1" } as { assigned_to_id: string }, "u1")).toBe(false);
    expect(isMyLead({ assigned_to_id: null }, "u1")).toBe(false);
    expect(isMyLead({ assigned_to_id: "u1" }, null)).toBe(false);
  });
});
