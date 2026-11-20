import { describe, it, expect } from "vitest";

import { cardZone, fromZonedInput, toZonedInput, zoneLabel, zoneOffsetMinutes } from "./zonedTime";

describe("company time zone helpers", () => {
  it("shows an instant as the company's wall clock, whatever the browser's zone", () => {
    expect(toZonedInput("2026-11-21T02:00:00Z", "Asia/Novosibirsk")).toBe("2026-11-21T09:00");
    expect(toZonedInput("2026-11-21T02:00:00Z", "America/New_York")).toBe("2026-11-20T21:00");
    expect(toZonedInput("not a date", "Asia/Novosibirsk")).toBe("");
  });

  it("reads a wall clock in the company zone back as the right instant", () => {
    expect(fromZonedInput("2026-11-21T09:00", "Asia/Novosibirsk")).toBe("2026-11-21T02:00:00.000Z");
    expect(fromZonedInput("2026-11-21T10:30", "Asia/Kolkata")).toBe("2026-11-21T05:00:00.000Z");
    expect(fromZonedInput("", "Asia/Novosibirsk")).toBe("");
  });

  it("handles daylight saving on both sides of the change", () => {
    // New York: EDT (UTC-4) until 1 Nov 2026, EST (UTC-5) after
    expect(fromZonedInput("2026-10-30T09:00", "America/New_York")).toBe("2026-10-30T13:00:00.000Z");
    expect(fromZonedInput("2026-11-03T09:00", "America/New_York")).toBe("2026-11-03T14:00:00.000Z");
    expect(zoneOffsetMinutes(Date.parse("2026-07-01T00:00:00Z"), "Europe/London")).toBe(60);
  });

  it("round-trips", () => {
    for (const tz of ["Asia/Novosibirsk", "Europe/Berlin", "America/Los_Angeles", "UTC"]) {
      const iso = "2026-03-15T06:45:00.000Z";
      expect(fromZonedInput(toZonedInput(iso, tz), tz)).toBe(iso);
    }
  });

  it("labels the zone with its offset, and falls back to the browser's zone for a bad name", () => {
    expect(zoneLabel("Asia/Novosibirsk", "2026-11-21T02:00:00Z")).toBe("Asia/Novosibirsk (GMT+7)");
    expect(zoneLabel("Asia/Kolkata", "2026-11-21T02:00:00Z")).toBe("Asia/Kolkata (GMT+5:30)");
    expect(zoneLabel("America/New_York", "2026-11-21T02:00:00Z")).toBe("America/New_York (GMT-5)");
    expect(cardZone("Asia/Novosibirsk")).toBe("Asia/Novosibirsk");
    expect(cardZone("Not/AZone")).toBe(Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC");
    expect(cardZone(null)).toBe(Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC");
  });
});
