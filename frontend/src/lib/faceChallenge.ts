/**
 * Run one liveness challenge on the live camera: track the face, guide the person, and capture the three
 * key frames (neutral, action, neutral) the tracker asks for. The engine is warmed up first (not counted), and
 * the result says how the check ended, for the attempt log.
 *
 * After a passive flash (6 Nov 2026, the Owner's Android 10 phone): the video is resumed if it was paused and the
 * engine re-warmed with a timed blank detection before the person's steps start; every detection runs under a
 * watchdog. A detection that takes longer than STALL_MS (WebGL stalled or its context lost) moves the engine to
 * the CPU backend once and the check goes on; what happened is reported in `note`.
 */
import { ensureVideoPlaying, grabFrame } from "@/hooks/useCamera";
import {
  blankDetection, fallbackToCpu, trackFaces, warmUpEngine, webglContextLost, type DetectedFace, type FaceApi,
} from "@/lib/faceEngine";
import {
  ChallengeTracker, type Budget, type LivenessAction, type Step, type StepTimeouts,
} from "@/lib/faceLiveness";

export interface ChallengeProgress {
  step: Step;
  hint?: string;
  budget?: Budget;
  budgetMs?: number;
  remainingMs?: number;
}

export type ChallengeOutcome = "done" | "timeout" | "aborted";

export interface ChallengeResult {
  outcome: ChallengeOutcome;
  /** The three key frames, only when the outcome is "done". */
  frames: HTMLCanvasElement[] | null;
  /** The furthest step the person reached ("done" when finished). */
  step: Exclude<Step, "timeout">;
  durationMs: number;
  avgTickMs: number | null;
  /** The first tick's time (ms) and how many ticks ran (stalled ones included); always set by runChallenge. */
  firstTickMs?: number | null;
  ticks?: number;
  /** What went wrong on the device, comma separated (video_resumed, stall, webgl_lost, cpu_fallback); "" when nothing. */
  note?: string;
}

export interface ChallengeOptions {
  /** A passive flash ran just before: resume the video and re-warm the engine first. */
  afterFlash?: boolean;
}

/** Test seams. */
export interface ChallengeDeps {
  track?: (api: FaceApi, input: HTMLCanvasElement) => Promise<DetectedFace[]>;
  grab?: (video: HTMLVideoElement, into: HTMLCanvasElement) => void;
  ensureVideo?: (video: HTMLVideoElement) => Promise<boolean>;
  warm?: (api: FaceApi) => Promise<void>;
  rewarm?: (api: FaceApi) => Promise<void>;
  toCpu?: (api: FaceApi) => Promise<boolean>;
  contextLost?: (api: FaceApi) => boolean;
  stallMs?: number;
}

const TICK_MS = 90;
/**
 * A detection slower than this is a stall, not a slow phone (technical constant, not a business number: the
 * slowest healthy phone in the attempt log ticks in about 750 ms on WebGL and 1.5 s on the CPU).
 */
export const STALL_MS = 3000;

const STALLED = Symbol("stalled");
const ABORTED = Symbol("aborted");

/** `work` raced against the abort signal and, when `watchdog`, against STALL_MS. */
function guarded<T>(
  work: Promise<T>, signal: AbortSignal | undefined, watchdog: boolean, stallMs: number,
): Promise<T | typeof STALLED | typeof ABORTED> {
  return new Promise((resolve) => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const onAbort = () => finish(ABORTED);
    const finish = (v: T | typeof STALLED | typeof ABORTED) => {
      clearTimeout(timer);
      signal?.removeEventListener("abort", onAbort);
      resolve(v);
    };
    if (signal?.aborted) return finish(ABORTED);
    if (watchdog) timer = setTimeout(() => finish(STALLED), stallMs);
    signal?.addEventListener("abort", onAbort);
    work.then(finish, () => finish([] as unknown as T));
  });
}

export async function runChallenge(
  api: FaceApi, video: HTMLVideoElement, action: LivenessAction, timeouts: StepTimeouts,
  onProgress: (p: ChallengeProgress) => void, signal?: AbortSignal, opts: ChallengeOptions = {}, deps: ChallengeDeps = {},
): Promise<ChallengeResult> {
  const track = deps.track ?? trackFaces;
  const grab = deps.grab ?? ((v: HTMLVideoElement, c: HTMLCanvasElement) => { grabFrame(v, c); });
  const stallMs = deps.stallMs ?? STALL_MS;
  const notes = new Set<string>();
  let tried = false;
  // The CPU backend is slow but never stalls: once on it, detections are awaited without the watchdog.
  let watchdog = true;
  /** Note the stall (and a lost context) and move to the CPU, once. */
  const recover = async () => {
    notes.add("stall");
    if ((deps.contextLost ?? webglContextLost)(api)) notes.add("webgl_lost");
    if (tried) return;
    tried = true;
    if (await (deps.toCpu ?? fallbackToCpu)(api)) {
      notes.add("cpu_fallback");
      watchdog = false;
    }
  };

  if (opts.afterFlash) {
    if (await (deps.ensureVideo ?? ensureVideoPlaying)(video)) notes.add("video_resumed");
    // Re-warm with a timed blank detection: a stalled engine shows here, before the person's steps start.
    const warm = await guarded((deps.rewarm ?? blankDetection)(api), signal, true, stallMs);
    if (warm === STALLED) await recover();
  } else {
    await (deps.warm ?? warmUpEngine)(api);
  }

  const tracker = new ChallengeTracker(action, timeouts);
  const work = document.createElement("canvas");
  const keys = [0, 1, 2].map(() => document.createElement("canvas"));
  const copy = (to: HTMLCanvasElement) => {
    to.width = work.width;
    to.height = work.height;
    to.getContext("2d")?.drawImage(work, 0, 0);
  };
  const began = performance.now();
  let ticks = 0;
  let tickTotal = 0;
  let firstTickMs: number | null = null;
  const result = (outcome: ChallengeOutcome, frames: HTMLCanvasElement[] | null): ChallengeResult => ({
    outcome, frames,
    step: tracker.reached === "timeout" ? "find_face" : tracker.reached,
    durationMs: Math.round(performance.now() - began),
    avgTickMs: ticks ? Math.round(tickTotal / ticks) : null,
    firstTickMs, ticks,
    note: [...notes].join(","),
  });
  while (!signal?.aborted) {
    const started = performance.now();
    grab(video, work);
    const got = await guarded(track(api, work), signal, watchdog, stallMs);
    if (got === ABORTED) break;
    const took = performance.now() - started;
    ticks += 1;
    tickTotal += took;
    if (firstTickMs === null) firstTickMs = Math.round(took);
    if (got === STALLED) await recover();
    // a stalled tick counts as a frame without a face, so the step budgets still run out
    const faces = got === STALLED ? [] : got;
    const main = faces[0];
    const update = tracker.update({
      faces: faces.length,
      facePx: main ? Math.min(main.box.width, main.box.height) : 0,
      landmarks: main ? main.landmarks : null,
    });
    if (update.capture !== undefined) copy(keys[update.capture]);
    onProgress({
      step: update.step, hint: update.hint, budget: update.budget, budgetMs: update.budgetMs, remainingMs: update.remainingMs,
    });
    if (update.step === "done") return result("done", keys);
    if (update.step === "timeout") return result("timeout", null);
    await new Promise((r) => setTimeout(r, Math.max(0, TICK_MS - (performance.now() - started))));
  }
  return result("aborted", null);
}
