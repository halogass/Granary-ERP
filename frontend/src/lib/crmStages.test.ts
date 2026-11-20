import { describe, it, expect } from "vitest";
import { boardStages, canMoveLead, isActiveStage, stageStyle, winRate, type FunnelRegistry } from "./crmStages";

const reg: FunnelRegistry = {
  deal_stages: [
    { key: "new", label: "New", order: 1, is_closed: false, is_won: false, side: false, probability: 5 },
    { key: "won", label: "Won", order: 7, is_closed: true, is_won: true, side: false, probability: 100 },
    { key: "handover", label: "Handover", order: 8, is_closed: true, is_won: true, side: false, probability: 100 },
    { key: "nurture", label: "Nurture", order: 10, is_closed: false, is_won: false, side: true, probability: 5 },
    { key: "lost", label: "Lost", order: 11, is_closed: true, is_won: false, side: true, probability: 0 },
  ],
  lead_statuses: [{ key: "triaging", label: "Triaging", order: 2, is_closed: false, is_active: true }],
  lead_transitions: { new: ["triaging", "disqualified"], promoted: [] },
  lead_start_statuses: ["new"], lead_sources: [], loss_reasons: [], disqualify_reasons: [],
};

describe("crmStages", () => {
  it("keeps lost off the board and orders the columns", () => {
    expect(boardStages(reg).map((s) => s.key)).toEqual(["new", "won", "handover", "nurture"]);
  });
  it("counts the won group and ignores open deals in the win rate", () => {
    expect(winRate(reg, ["new", "won", "handover", "lost"])).toBe(67);
    expect(winRate(reg, ["new"])).toBeNull();
  });
  it("treats only open ladder stages as active", () => {
    expect([isActiveStage(reg, "new"), isActiveStage(reg, "nurture"), isActiveStage(reg, "won")]).toEqual([true, false, false]);
  });
  it("labels deal stages and lead statuses, and shows an unknown key as itself", () => {
    expect(stageStyle(reg, "triaging").label).toBe("Triaging");
    expect(stageStyle(reg, "closed_won").label).toBe("closed won");
  });
  it("follows the lead transition map", () => {
    expect(canMoveLead(reg, "new", "triaging")).toBe(true);
    expect(canMoveLead(reg, "promoted", "new")).toBe(false);
  });
});
