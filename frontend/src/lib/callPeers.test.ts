import { describe, expect, it } from "vitest";
import { guestPeerId, withPeerName } from "./callPeers";

describe("guestPeerId", () => {
  it("matches the signalling id the API gives an admitted guest", () => {
    expect(guestPeerId("3f2a9c1e-77aa-4bcd-9e01-0123456789ab")).toBe("guest_3f2a9c1e");
  });
});

describe("withPeerName", () => {
  it("names a peer from its offer or the waiting room", () => {
    const m = withPeerName(new Map(), "guest_3f2a9c1e", "Aiko Tanaka");
    expect(m.get("guest_3f2a9c1e")).toBe("Aiko Tanaka");
  });
  it("ignores a missing or blank name", () => {
    const prev = new Map([["erp_1", "Ken"]]);
    expect(withPeerName(prev, "erp_1", undefined)).toBe(prev);
    expect(withPeerName(prev, "erp_1", "  ")).toBe(prev);
    expect(withPeerName(prev, "erp_1", "Ken")).toBe(prev);
  });
});
