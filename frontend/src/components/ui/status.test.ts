import { describe, it, expect } from "vitest";
import { STATUS_TONES, statusLabel, statusTone } from "./status";

describe("status tones", () => {
  it("maps known statuses per family and falls back to the generic map, then neutral", () => {
    expect(statusTone("invoice", "overdue")).toBe("red");
    expect(statusTone("invoice", "Partially Paid")).toBe("orange");
    expect(statusTone("deal", "proposal")).toBe("orange");
    expect(statusTone("ticket", "rejected")).toBe("red");        // generic
    expect(statusTone("deal", "something_new")).toBe("neutral");
    expect(statusTone("deal", null)).toBe("neutral");
  });

  it("keeps red for needs-you, failed and overdue only", () => {
    const fine = ["paid", "won", "approved", "accepted", "done", "completed", "signed", "active"];
    for (const fam of Object.values(STATUS_TONES)) {
      for (const s of fine) if (fam[s]) expect(fam[s]).not.toBe("red");
    }
  });

  it("humanizes a status", () => {
    expect(statusLabel("partially_paid")).toBe("Partially paid");
    expect(statusLabel(undefined)).toBe("-");
  });
});
