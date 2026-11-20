import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it, vi } from "vitest";

import {
  MIN_FLASH_MS, chroma, flashTiming, judgeFlash, passiveVerdict, pearson, pickColors, regionMean, releaseCapture, runFlash,
  skinRegion, type Rgb,
} from "@/lib/faceFlash";

const PALETTE: Rgb[] = [[255, 120, 120], [120, 255, 140], [120, 150, 255], [255, 230, 120], [220, 130, 255], [120, 240, 240]];
const C3: Rgb[] = [[255, 120, 120], [120, 255, 140], [120, 150, 255]];
const N: Rgb = [100, 100, 100];
const tint = (k: number, colors: Rgb[]) => colors.map((c) => [N[0] + k * c[0], N[1] + k * c[1], N[2] + k * c[2]] as Rgb);

// Expected values from the plain-Python reference (no longer shipped),
// chromaticity units, lag 0 and lag 1 tried.
const HAND = [
  { name: "reflects the colours", neutral: N, flashes: tint(0.1, C3), colors: C3, min_delta: 1.5, min_corr: 0.6,
    delta: 4.088728, corr: 0.999908, outcome: "pass", lag: 0 },
  { name: "too little change (bright light)", neutral: N, flashes: tint(0.02, C3), colors: C3, min_delta: 1.5, min_corr: 0.6,
    delta: 0.925506, corr: 0.999882, outcome: "inconclusive", lag: 0 },
  { name: "change against the colours", neutral: N, flashes: tint(-0.1, C3), colors: C3, min_delta: 1.5, min_corr: 0.6,
    delta: 3.830640, corr: 0.390046, outcome: "fail", lag: 1 },
  { name: "no change at all (zero variance)", neutral: N, flashes: [N, N, N], colors: C3, min_delta: 1.5, min_corr: 0.6,
    delta: 0, corr: 0, outcome: "inconclusive", lag: 0 },
  { name: "partial response passes (two flashes: no lag 1)", neutral: [10, 10, 10], flashes: [[16, 10, 10], [10, 12, 10]],
    colors: [[10, 0, 0], [0, 10, 0]], min_delta: 0.5, min_corr: 0.6, delta: 11.215278, corr: 0.967247, outcome: "pass", lag: 0 },
  { name: "partial response under a strict corr", neutral: [10, 10, 10], flashes: [[16, 10, 10], [10, 12, 10]],
    colors: [[10, 0, 0], [0, 10, 0]], min_delta: 0.5, min_corr: 0.99, delta: 11.215278, corr: 0.967247, outcome: "fail", lag: 0 },
  { name: "a camera one colour late passes at lag 1", neutral: N, flashes: [N, ...tint(0.1, C3).slice(0, 2)], colors: C3,
    min_delta: 1.5, min_corr: 0.6, delta: 3.438249, corr: 0.999966, outcome: "pass", lag: 1 },
  { name: "auto exposure changing the brightness still passes", neutral: N,
    flashes: tint(0.1, C3).map((f, i) => f.map((x) => x * [0.7, 1.2, 0.85][i]) as Rgb), colors: C3,
    min_delta: 1.5, min_corr: 0.6, delta: 4.088728, corr: 0.999908, outcome: "pass", lag: 0 },
] as const;

describe("judgeFlash (contract maths)", () => {
  for (const v of HAND) {
    it(v.name, () => {
      const j = judgeFlash(v.neutral as Rgb, v.flashes as unknown as Rgb[], v.colors as unknown as Rgb[], v.min_delta, v.min_corr);
      expect(j.delta).toBeCloseTo(v.delta, 5);
      expect(j.corr).toBeCloseTo(v.corr, 5);
      expect(j.outcome).toBe(v.outcome);
      expect(j.lag).toBe(v.lag);
    });
  }

  it("chromaticity drops brightness: a frame only darker or brighter is no colour response", () => {
    expect(chroma([150, 100, 90])).toEqual(chroma([75, 50, 45]));
    expect(chroma([0, 0, 0])).toEqual([0, 0, 0]);
    const n: Rgb = [150, 100, 90];
    const flashes = [0.7, 1.1, 0.8, 1.2, 0.9].map((g) => n.map((x) => x * g) as Rgb);
    const j = judgeFlash(n, flashes, [...C3, [255, 230, 120], [220, 130, 255]], 1.5, 0.6);
    expect(j.outcome).toBe("inconclusive");
    expect(j.delta).toBeLessThan(1e-9);
  });

  it("mismatched lengths are inconclusive", () => {
    expect(judgeFlash(N, [N], [], 1.5, 0.6)).toEqual({ delta: 0, corr: 0, outcome: "inconclusive", lag: 0 });
  });

  it("pearson is 0 when one side is flat", () => {
    expect(pearson([1, 2, 3], [4, 4, 4])).toBe(0);
    expect(pearson([1, 2, 3], [2, 4, 6])).toBeCloseTo(1, 10);
  });
});

/**
 * The shared golden vectors written by the backend (rust-version/compat-tests/golden/face/passive_flash.json).
 * Accepted shapes: an array of cases or {cases: [...]}; each case has neutral, flashes, colors, min_delta,
 * min_corr (or flash_min_delta / flash_min_corr) and the expected delta, corr, outcome (top level or under
 * "expected").
 */
// vitest runs from frontend/.
const GOLDEN = resolve(process.cwd(), "../rust-version/compat-tests/golden/face/passive_flash.json");
describe.skipIf(!existsSync(GOLDEN))("judgeFlash vs the shared golden vectors", () => {
  const raw = existsSync(GOLDEN) ? JSON.parse(readFileSync(GOLDEN, "utf8")) : [];
  const cases: Record<string, unknown>[] = Array.isArray(raw) ? raw : raw.cases ?? [];
  it("has cases", () => expect(cases.length).toBeGreaterThan(0));
  cases.forEach((c, i) => {
    it(String(c.name ?? `case ${i}`), () => {
      const exp = (c.expected ?? c) as { delta: number; corr: number; outcome: string; lag?: number };
      const j = judgeFlash(
        (c.neutral ?? c.neutral_mean) as Rgb, (c.flashes ?? c.flash_means) as Rgb[], (c.colors ?? c.colours) as Rgb[],
        Number(c.min_delta ?? c.flash_min_delta), Number(c.min_corr ?? c.flash_min_corr),
      );
      expect(j.delta).toBeCloseTo(exp.delta, 6);
      expect(j.corr).toBeCloseTo(exp.corr, 6);
      expect(j.outcome).toBe(exp.outcome);
      expect(j.lag).toBe(exp.lag ?? 0);
    });
  });
});

describe("passiveVerdict", () => {
  it("passes only on a flash pass with a real enough face", () => {
    expect(passiveVerdict("pass", 0.9, 0.7)).toEqual({ passed: true });
    expect(passiveVerdict("pass", 0.5, 0.7)).toEqual({ passed: false, reason: "spoof", outcome: "passive_failed" });
    expect(passiveVerdict("fail", 0.99, 0.7)).toEqual({ passed: false, reason: "flash_failed", outcome: "passive_failed" });
    expect(passiveVerdict("inconclusive", 0.99, 0.7))
      .toEqual({ passed: false, reason: "flash_inconclusive", outcome: "passive_inconclusive" });
  });
});

describe("pickColors", () => {
  it("draws from the palette and never repeats a colour back to back", () => {
    for (let run = 0; run < 200; run++) {
      const seq = pickColors(PALETTE, 5);
      expect(seq).toHaveLength(5);
      for (const c of seq) expect(PALETTE).toContainEqual(c);
      for (let i = 1; i < seq.length; i++) expect(seq[i]).not.toEqual(seq[i - 1]);
    }
  });

  it("skips the previous colour with the injected random index", () => {
    // First pick index 0; then r=0 among the others -> index 1; then r=0 -> index 0 again (not 1).
    const seq = pickColors(PALETTE, 3, () => 0);
    expect(seq).toEqual([PALETTE[0], PALETTE[1], PALETTE[0]]);
    // A two-colour palette alternates.
    expect(pickColors([[1, 1, 1], [2, 2, 2]], 4, () => 0)).toEqual([[1, 1, 1], [2, 2, 2], [1, 1, 1], [2, 2, 2]]);
  });

  it("refuses a palette that cannot avoid repeats", () => {
    expect(() => pickColors([[1, 2, 3]], 3)).toThrow();
  });
});

describe("skin region", () => {
  const lm = Array.from({ length: 68 }, () => [0, 0] as [number, number]);
  lm[3] = [10.4, 50];
  lm[13] = [13.6, 50];
  lm[28] = [30, 20.2];
  lm[33] = [30, 21.7];

  it("spans lm[3]..lm[13] across and lm[28]..lm[33] down", () => {
    expect(skinRegion(lm)).toEqual({ x0: 10.4, x1: 13.6, y0: 20.2, y1: 21.7 });
  });

  it("averages R, G, B over the rounded, inclusive pixels", () => {
    const w = 20, h = 30;
    const data = new Uint8ClampedArray(w * h * 4);
    for (let y = 0; y < h; y++) {
      for (let x = 0; x < w; x++) {
        const i = (y * w + x) * 4;
        data[i] = x; data[i + 1] = y; data[i + 2] = 7; data[i + 3] = 255;
      }
    }
    // x 10..14, y 20..22 -> mean x 12, mean y 21.
    expect(regionMean({ data, width: w, height: h }, skinRegion(lm))).toEqual([12, 21, 7]);
    expect(regionMean({ data, width: w, height: h }, { x0: 25, x1: 30, y0: 0, y1: 1 })).toBeNull();
  });
});

describe("runFlash", () => {
  it("grabs the neutral frame, then one frame per colour after it settled, and clears the screen", async () => {
    let t = 0;
    const events: string[] = [];
    const grab = vi.fn(() => { events.push(`grab@${t}`); });
    const shown: (Rgb | null)[] = [];
    const cap = await runFlash(document.createElement("video"), C3, 300, 150, (c) => { shown.push(c); events.push(c ? `show@${t}` : `clear@${t}`); }, undefined, {
      grab, wait: async (ms) => { t += ms; }, now: () => t,
    });
    expect(grab).toHaveBeenCalledTimes(4);
    expect(cap.flashes).toHaveLength(3);
    expect(cap.captureAtMs).toEqual([150, 450, 750]);
    expect(cap.durationMs).toBe(900);
    expect(cap.aborted).toBe(false);
    expect(shown).toEqual([null, ...C3, null]);
    expect(events.slice(0, 4)).toEqual(["clear@0", "grab@0", "show@0", "grab@150"]);
  });

  it("never flashes faster than the floor, even when the setting asks for it", async () => {
    expect(flashTiming(100, 150)).toEqual({ flashMs: MIN_FLASH_MS, settleMs: 150 });
    expect(flashTiming(300, 500)).toEqual({ flashMs: 300, settleMs: 299 });
  });

  it("frees the capture canvases once encoded", () => {
    const neutral = document.createElement("canvas");
    const flashes = [document.createElement("canvas"), document.createElement("canvas")];
    for (const c of [neutral, ...flashes]) { c.width = 640; c.height = 480; }
    releaseCapture({ neutral, flashes });
    for (const c of [neutral, ...flashes]) expect([c.width, c.height]).toEqual([0, 0]);
  });

  it("stops and clears the screen when aborted", async () => {
    const ctl = new AbortController();
    const shown: (Rgb | null)[] = [];
    const cap = await runFlash(document.createElement("video"), C3, 300, 150, (c) => shown.push(c), ctl.signal, {
      grab: () => {}, wait: async () => { ctl.abort(); }, now: () => 0,
    });
    expect(cap.aborted).toBe(true);
    expect(shown[shown.length - 1]).toBeNull();
  });
});
