/**
 * Passive liveness on the kiosk (the tablet judges): flash judgement on the face's skin region plus the
 * anti-spoof score on the neutral frame, combined into the contract's verdict and the metrics the kiosk posts.
 */
import type { AntiSpoof } from "@/lib/antiSpoof";
import type { DetectedFace } from "@/lib/faceEngine";
import {
  canvasPixels, judgeFlash, passiveVerdict, regionMean, skinRegion,
  type FallbackReason, type FlashCapture, type PassiveSettings, type PassiveVerdict, type Rgb,
} from "@/lib/faceFlash";

/** liveness.metrics.passive of POST /kiosk/clock. */
export interface PassiveMetrics {
  corr: number | null;
  delta: number | null;
  spoof_real: number | null;
  /** 0167: 1 when the camera was judged one colour late. */
  lag?: 0 | 1;
  fallback_reason?: FallbackReason;
}

export interface PassiveJudgement { verdict: PassiveVerdict; metrics: PassiveMetrics }

const round = (n: number, d: number) => Number(n.toFixed(d));
/** The server's liveness MIN_FACE_PX (services/src/face/liveness.rs): a smaller face is not judged. */
export const MIN_FACE_PX = 120;

const noFace = (): PassiveJudgement => ({
  verdict: { passed: false, reason: "no_face", outcome: "passive_inconclusive" },
  metrics: { corr: null, delta: null, spoof_real: null, fallback_reason: "no_face" },
});

/**
 * Judge one flash capture. `face` is the single face found on the neutral frame (SSD + 68 landmarks); its skin
 * region is used on every flash frame (the flash lasts under 2 seconds, the person holds still). No face, or
 * one smaller than the server's MIN_FACE_PX, is "no_face" (not sure), never "spoof".
 */
export async function judgeCapture(
  capture: FlashCapture, face: DetectedFace | null, settings: PassiveSettings, antiSpoof: AntiSpoof,
): Promise<PassiveJudgement> {
  if (!face || face.landmarks.length < 68) return noFace();
  if (Math.min(face.box.width, face.box.height) < MIN_FACE_PX) return noFace();
  const neutralPx = canvasPixels(capture.neutral);
  if (!neutralPx) return noFace();
  const region = skinRegion(face.landmarks);
  const neutral = regionMean(neutralPx, region);
  const flashes = capture.flashes.map((c) => {
    const px = canvasPixels(c);
    return px ? regionMean(px, region) : null;
  });
  if (!neutral || flashes.some((f) => !f)) return noFace();

  const flash = judgeFlash(neutral, flashes as Rgb[], capture.colors, settings.flash_min_delta, settings.flash_min_corr);
  let spoof: number | null = null;
  try {
    spoof = await antiSpoof.score(neutralPx, face.box);
  } catch {
    spoof = null;
  }
  const verdict = passiveVerdict(flash.outcome, spoof ?? -1, settings.spoof_min_real);
  const metrics: PassiveMetrics = {
    corr: round(flash.corr, 3), delta: round(flash.delta, 2), spoof_real: spoof === null ? null : round(spoof, 3),
    lag: flash.lag,
  };
  if (!verdict.passed) metrics.fallback_reason = verdict.reason;
  return { verdict, metrics };
}
