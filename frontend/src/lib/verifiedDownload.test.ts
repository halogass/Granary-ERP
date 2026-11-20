import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  verificationApi: { forEntity: vi.fn(), issue: vi.fn(), downloadPdf: vi.fn() },
  approvalsApi: { forEntity: vi.fn() },
}));
vi.mock("./api", () => api);

import { downloadVerifiedCopy } from "./verifiedDownload";

const activeCopy = { id: "v1", status: "active", live_state: "valid", has_pdf: true, document_number: "Q-1" };

describe("downloadVerifiedCopy", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    globalThis.URL.createObjectURL = vi.fn(() => "blob:x");
    globalThis.URL.revokeObjectURL = vi.fn();
    api.verificationApi.downloadPdf.mockResolvedValue({ data: new Blob(["%PDF"]) });
  });

  it("downloads the active verified copy instead of printing", async () => {
    api.verificationApi.forEntity.mockResolvedValue({ data: { supported: true, qr_enabled: true, verifications: [activeCopy] } });
    const print = vi.fn();
    expect(await downloadVerifiedCopy("invoice", "i1", print)).toEqual({ outcome: "verified" });
    expect(api.verificationApi.downloadPdf).toHaveBeenCalledWith("v1");
    expect(print).not.toHaveBeenCalled();
  });

  it("issues a copy for an approved document that has none yet", async () => {
    api.verificationApi.forEntity.mockResolvedValue({ data: { supported: true, qr_enabled: true, verifications: [] } });
    api.approvalsApi.forEntity.mockResolvedValue({ data: { gate: "approved" } });
    api.verificationApi.issue.mockResolvedValue({ data: { ...activeCopy, id: "v2" } });
    const print = vi.fn();
    expect(await downloadVerifiedCopy("invoice", "i1", print)).toEqual({ outcome: "verified" });
    expect(api.verificationApi.issue).toHaveBeenCalledWith("invoice", "i1");
    expect(print).not.toHaveBeenCalled();
  });

  it("falls back to the plain print by default when not approved or not permitted", async () => {
    api.verificationApi.forEntity.mockResolvedValue({ data: { supported: true, qr_enabled: true, verifications: [] } });
    api.approvalsApi.forEntity.mockResolvedValue({ data: { gate: "pending" } });
    const print = vi.fn();
    expect(await downloadVerifiedCopy("invoice", "i1", print)).toEqual({ outcome: "print" });
    expect(print).toHaveBeenCalledOnce();

    api.verificationApi.forEntity.mockRejectedValue({ response: { status: 403 } });
    const print2 = vi.fn();
    expect(await downloadVerifiedCopy("invoice", "i1", print2)).toEqual({ outcome: "print" });
    expect(print2).toHaveBeenCalledOnce();
  });

  describe("requireApproval", () => {
    it.each(["not_submitted", "pending", "rejected", "stale", "cancelled"])("refuses a %s document without printing", async (gate) => {
      api.approvalsApi.forEntity.mockResolvedValue({ data: { gate } });
      const print = vi.fn();
      const result = await downloadVerifiedCopy("quotation", "q1", print, { requireApproval: true, label: "quotation" });
      expect(result.outcome).toBe("refused");
      expect(result.outcome === "refused" && result.message).toMatch(/quotation/);
      expect(print).not.toHaveBeenCalled();
      expect(api.verificationApi.downloadPdf).not.toHaveBeenCalled();
    });

    it("hands out only the verified copy of an approved document", async () => {
      api.approvalsApi.forEntity.mockResolvedValue({ data: { gate: "approved" } });
      api.verificationApi.forEntity.mockResolvedValue({ data: { supported: true, qr_enabled: true, verifications: [] } });
      api.verificationApi.issue.mockResolvedValue({ data: activeCopy });
      const print = vi.fn();
      expect(await downloadVerifiedCopy("quotation", "q1", print, { requireApproval: true })).toEqual({ outcome: "verified" });
      expect(print).not.toHaveBeenCalled();
    });

    it("refuses instead of printing when the verified copy cannot be produced", async () => {
      api.approvalsApi.forEntity.mockResolvedValue({ data: { gate: "approved" } });
      api.verificationApi.forEntity.mockResolvedValue({ data: { supported: true, qr_enabled: true, verifications: [] } });
      api.verificationApi.issue.mockRejectedValue({ response: { status: 502 } });
      const print = vi.fn();
      const result = await downloadVerifiedCopy("quotation", "q1", print, { requireApproval: true });
      expect(result.outcome).toBe("refused");
      expect(print).not.toHaveBeenCalled();
    });

    it("refuses when approval status cannot be checked", async () => {
      api.approvalsApi.forEntity.mockRejectedValue({ response: { status: 500 } });
      const print = vi.fn();
      expect((await downloadVerifiedCopy("quotation", "q1", print, { requireApproval: true })).outcome).toBe("refused");
      expect(print).not.toHaveBeenCalled();
    });
  });
});
