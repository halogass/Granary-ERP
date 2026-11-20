/**
 * Passive liveness, part 1: the colour flash (Release 2, passive_contract.md).
 *
 * The screen lights the face with a short sequence of soft colours; a live face (3-D skin, close to the screen)
 * reflects each colour, a printed photo or a replay on another screen mostly does not. One frame is grabbed
 * before the first colour (neutral) and one per colour after it has settled. The judgement compares the
 * skin-region colour change of each flash frame with the screen colour, with the same maths the server uses
 * for phones (rust-version, shared golden vectors in compat-tests/golden/face/passive_flash.json).
 */
import { cpuCanvas, grabFrame } from "@/hooks/useCamera";

export type Rgb = [number, number, number];
export type FlashOutcome = "pass" | "fail" | "inconclusive";
/** Why a passive check did not pass, as the server records it (details.passive.fallback_reason). */
export type FallbackReason = "flash_inconclusive" | "flash_failed" | "spoof" | "no_face";

/** The kiosk session's `passive` object (hr.face.passive.* settings). */
export interface PassiveSettings {
  enabled: boolean;
  flash_count: number;
  flash_ms: number;
  settle_ms: number;
  palette: Rgb[];
  flash_min_delta: number;
  flash_min_corr: number;
  spoof_min_real: number;
}

/** The one-line notice shown before the flash. */
export const FLASH_NOTICE = "The screen will flash soft colours for about 2 seconds";
/** How long the notice stays up before the first colour (UI pacing only). */
export const NOTICE_MS = 1200;
/** Never more than 4 colour changes a second (WCAG 2.3.1 asks for under 3 flashes a second). */
export const MIN_FLASH_MS = 250;

// ─── Colours ──────────────────────────────────────────────────────────────────

/** A uniform random integer in [0, n) from the platform's crypto RNG. */
export function cryptoRandomInt(n: number): number {
  if (n <= 1) return 0;
  const limit = Math.floor(0x100000000 / n) * n;
  const buf = new Uint32Array(1);
  for (;;) {
    crypto.getRandomValues(buf);
    if (buf[0] < limit) return buf[0] % n;
  }
}

/** `count` colours from the palette, never the same colour twice in a row (kiosk: the tablet picks). */
export function pickColors(palette: Rgb[], count: number, randomInt: (n: number) => number = cryptoRandomInt): Rgb[] {
  if (palette.length < 2) throw new Error("The flash palette needs at least two colours.");
  const out: Rgb[] = [];
  let prev = -1;
  for (let i = 0; i < count; i++) {
    // Choose among the other colours: index r skips the previous one.
    let idx = prev < 0 ? randomInt(palette.length) : randomInt(palette.length - 1);
    if (prev >= 0 && idx >= prev) idx += 1;
    out.push([...palette[idx]] as Rgb);
    prev = idx;
  }
  return out;
}

// ─── Skin region ──────────────────────────────────────────────────────────────

export interface Region { x0: number; x1: number; y0: number; y1: number }
export interface Pixels { data: Uint8ClampedArray | Uint8Array; width: number; height: number }

/**
 * Cheeks and nose between the nose bridge and the nose bottom, from the 68 landmarks of the raw (un-mirrored)
 * frame: x in [lm[3].x, lm[13].x], y in [lm[28].y, lm[33].y].
 */
export function skinRegion(landmarks: [number, number][]): Region {
  const [ax, bx] = [landmarks[3][0], landmarks[13][0]];
  const [ay, by] = [landmarks[28][1], landmarks[33][1]];
  return { x0: Math.min(ax, bx), x1: Math.max(ax, bx), y0: Math.min(ay, by), y1: Math.max(ay, by) };
}

/** Mean R, G, B over the region (edges rounded to the nearest pixel, inclusive, clamped to the image); null when empty. */
export function regionMean(img: Pixels, r: Region): Rgb | null {
  const x0 = Math.max(0, Math.round(r.x0));
  const x1 = Math.min(img.width - 1, Math.round(r.x1));
  const y0 = Math.max(0, Math.round(r.y0));
  const y1 = Math.min(img.height - 1, Math.round(r.y1));
  if (x1 < x0 || y1 < y0) return null;
  let sr = 0, sg = 0, sb = 0, n = 0;
  for (let y = y0; y <= y1; y++) {
    let i = (y * img.width + x0) * 4;
    for (let x = x0; x <= x1; x++, i += 4) {
      sr += img.data[i];
      sg += img.data[i + 1];
      sb += img.data[i + 2];
      n += 1;
    }
  }
  return [sr / n, sg / n, sb / n];
}

// ─── Judgement ────────────────────────────────────────────────────────────────

export interface FlashJudgement {
  delta: number;
  corr: number;
  outcome: FlashOutcome;
  /** 0: flash frame i judged against colour i; 1: against colour i-1 (a camera one colour late). */
  lag: 0 | 1;
}

/** x / (r + g + b) * 255 per channel (0 when the sum is 0): brightness (auto exposure) drops out. */
export function chroma(m: Rgb): Rgb {
  const s = m[0] + m[1] + m[2];
  if (s <= 0) return [0, 0, 0];
  return [m[0] / s * 255, m[1] / s * 255, m[2] / s * 255];
}

function centred(rows: Rgb[]): number[][] {
  const k = rows.length;
  const mean = [0, 1, 2].map((c) => rows.reduce((s, r) => s + r[c], 0) / k);
  return rows.map((r) => [r[0] - mean[0], r[1] - mean[1], r[2] - mean[2]]);
}

/** A centred sum of squares below this counts as zero variance (corr 0), as on the server. */
export const ZERO_VARIANCE_EPS = 1e-9;

/** Pearson correlation; 0 when either side has zero variance. */
export function pearson(a: number[], b: number[]): number {
  const n = a.length;
  if (!n || n !== b.length) return 0;
  const ma = a.reduce((s, v) => s + v, 0) / n;
  const mb = b.reduce((s, v) => s + v, 0) / n;
  let sab = 0, saa = 0, sbb = 0;
  for (let i = 0; i < n; i++) {
    const da = a[i] - ma, db = b[i] - mb;
    sab += da * db;
    saa += da * da;
    sbb += db * db;
  }
  // same epsilon as the server (services/src/face/flash.rs, golden zero_variance_eps)
  if (saa < ZERO_VARIANCE_EPS || sbb < ZERO_VARIANCE_EPS) return 0;
  return sab / Math.sqrt(saa * sbb);
}

/** corr and delta of flash frames against colours, pair by pair, in chromaticity units. */
function series(neutral: Rgb, flashes: Rgb[], colors: Rgb[]): { corr: number; delta: number } {
  const n = chroma(neutral);
  const d = centred(flashes.map((f) => {
    const c = chroma(f);
    return [c[0] - n[0], c[1] - n[1], c[2] - n[2]] as Rgb;
  }));
  const e = centred(colors.map(chroma));
  const dFlat = d.flat();
  const delta = dFlat.reduce((s, v) => s + Math.abs(v), 0) / dFlat.length;
  return { corr: pearson(dFlat, e.flat()), delta };
}

/**
 * The contract's flash judgement (lag + chromaticity revision, migration 0167), as the server's
 * services/src/face/flash.rs: every mean and colour as chromaticity; d_i = F_i - N, e_i = C_i; centre both over
 * i per channel; delta = mean |d'|; corr = Pearson(concat d', concat e'); judged at lag 0 and, with 3 or more
 * flashes, at lag 1 (F_i against C_{i-1}, the first frame dropped), the higher corr wins (ties: lag 0);
 * delta < minDelta -> inconclusive, else corr >= minCorr -> pass, else fail.
 */
export function judgeFlash(
  neutral: Rgb, flashes: Rgb[], colors: Rgb[], minDelta: number, minCorr: number,
): FlashJudgement {
  const k = flashes.length;
  if (!k || k !== colors.length) return { delta: 0, corr: 0, outcome: "inconclusive", lag: 0 };
  let best = series(neutral, flashes, colors);
  let lag: 0 | 1 = 0;
  if (k >= 3) {
    const late = series(neutral, flashes.slice(1), colors.slice(0, k - 1));
    if (late.corr > best.corr) { best = late; lag = 1; }
  }
  const outcome: FlashOutcome = best.delta < minDelta ? "inconclusive" : best.corr >= minCorr ? "pass" : "fail";
  return { delta: best.delta, corr: best.corr, outcome, lag };
}

export type PassiveVerdict =
  | { passed: true }
  | { passed: false; reason: FallbackReason; outcome: "passive_inconclusive" | "passive_failed" };

/** Flash pass AND spoof real >= setting -> passed; flash inconclusive -> inconclusive; anything else -> failed. */
export function passiveVerdict(flash: FlashOutcome, spoofReal: number, spoofMinReal: number): PassiveVerdict {
  if (flash === "inconclusive") return { passed: false, reason: "flash_inconclusive", outcome: "passive_inconclusive" };
  if (flash === "fail") return { passed: false, reason: "flash_failed", outcome: "passive_failed" };
  if (!(spoofReal >= spoofMinReal)) return { passed: false, reason: "spoof", outcome: "passive_failed" };
  return { passed: true };
}

// ─── Running the flash ────────────────────────────────────────────────────────

export interface FlashCapture {
  neutral: HTMLCanvasElement;
  flashes: HTMLCanvasElement[];
  colors: Rgb[];
  /** Milliseconds from the start to each flash capture, and the whole flash. */
  captureAtMs: number[];
  durationMs: number;
  aborted: boolean;
}

export interface FlashDeps {
  grab?: (video: HTMLVideoElement, into: HTMLCanvasElement) => void;
  wait?: (ms: number) => Promise<void>;
  now?: () => number;
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, Math.max(0, ms)));

/** The flash and settle times actually used: flash never under MIN_FLASH_MS, settle inside the flash. */
export function flashTiming(flashMs: number, settleMs: number): { flashMs: number; settleMs: number } {
  const f = Math.max(MIN_FLASH_MS, Math.round(flashMs || 0));
  const s = Math.min(Math.max(0, Math.round(settleMs || 0)), f - 1);
  return { flashMs: f, settleMs: s };
}

/**
 * Show each colour full screen through `show` (null clears it), grab the neutral frame first and one frame
 * per colour `settleMs` after it went up. The flash is the check itself, so reduced-motion users get it too,
 * at the same gentle pace (never faster than MIN_FLASH_MS per colour).
 */
export async function runFlash(
  video: HTMLVideoElement, colors: Rgb[], flashMs: number, settleMs: number,
  show: (color: Rgb | null) => void, signal?: AbortSignal, deps: FlashDeps = {},
): Promise<FlashCapture> {
  const grab = deps.grab ?? ((v, c) => { grabFrame(v, c); });
  const wait = deps.wait ?? sleep;
  const now = deps.now ?? (() => performance.now());
  const t = flashTiming(flashMs, settleMs);
  const began = now();
  // CPU-backed canvases (willReadFrequently): six GPU canvases plus the JPEG encodes starve the GPU of a
  // low-end phone, and the face engine (WebGL) stalled right after the flash (6 Nov 2026).
  const neutral = cpuCanvas();
  const flashes: HTMLCanvasElement[] = [];
  const captureAtMs: number[] = [];
  show(null);
  grab(video, neutral);
  try {
    for (const c of colors) {
      if (signal?.aborted) break;
      const up = now();
      show(c);
      await wait(t.settleMs);
      if (signal?.aborted) break;
      const frame = cpuCanvas();
      grab(video, frame);
      flashes.push(frame);
      captureAtMs.push(Math.round(now() - began));
      await wait(t.flashMs - (now() - up));
    }
  } finally {
    show(null);
  }
  return {
    neutral, flashes, colors, captureAtMs, durationMs: Math.round(now() - began),
    aborted: !!signal?.aborted || flashes.length < colors.length,
  };
}

/** Free the capture canvases' backing stores once they have been encoded or judged. */
export function releaseCapture(capture: Pick<FlashCapture, "neutral" | "flashes">): void {
  for (const c of [capture.neutral, ...capture.flashes]) {
    c.width = 0;
    c.height = 0;
  }
}

/** The RGBA pixels of a canvas (null when the canvas has no 2-D context, e.g. in tests). */
export function canvasPixels(canvas: HTMLCanvasElement): Pixels | null {
  const ctx = canvas.getContext("2d");
  if (!ctx || !canvas.width || !canvas.height) return null;
  return ctx.getImageData(0, 0, canvas.width, canvas.height);
}
