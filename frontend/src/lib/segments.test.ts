import { describe, it, expect } from "vitest";
import {
  parseSegments, segmentFilterOptions, segmentFilterParams, segmentLabel, segmentOf, segmentOptions, segmentPatch,
} from "./segments";

describe("segments", () => {
  it("reads the configured list: keys or {key, label}; anything else is no list", () => {
    expect(parseSegments(["Retail", { key: "gov", label: "Public sector" }, { key: "edu" }, 3, "", null])).toEqual([
      { key: "Retail", label: "Retail" }, { key: "gov", label: "Public sector" }, { key: "edu", label: "edu" },
    ]);
    expect(parseSegments(undefined)).toEqual([]);
    expect(parseSegments("Retail")).toEqual([]);
  });

  it("labels a value, keeps an unknown one as itself and shows none as a dash", () => {
    const segs = parseSegments([{ key: "gov", label: "Public sector" }]);
    expect(segmentLabel(segs, "gov")).toBe("Public sector");
    expect(segmentLabel(segs, "old")).toBe("old");
    expect(segmentLabel(segs, null)).toBe("-");
    expect(segmentOptions(segs, "old").map((o) => o.value)).toEqual(["gov", "old"]);
    expect(segmentFilterOptions(segs).at(-1)?.value).toBe("none");
  });

  it("keeps the API field names in one place", () => {
    expect(segmentOf({ segment: "gov" })).toBe("gov");
    expect(segmentOf(null)).toBe("");
    expect(segmentPatch(" gov ")).toEqual({ segment: "gov" });
    expect(segmentPatch("")).toEqual({ segment: null });
    expect(segmentFilterParams("gov")).toEqual({ segment: "gov" });
    expect(segmentFilterParams("")).toEqual({});
  });
});
