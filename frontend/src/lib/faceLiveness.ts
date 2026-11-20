/**
 * Active liveness on the device: guide the person through one random action (turn left / turn right; a blink
 * is still judged for challenges issued before 6 Nov 2026, but never asked for) and pick three key frames,
 * neutral, the action, neutral again.
 *
 * Mirrors rust-version/crates/services/src/face/liveness.rs (same landmark maths). The client thresholds are a
 * little STRICTER than the server's, so a sequence the device accepts is not then refused by the server, but
 * only just inside them, so the device never demands much more than the server does.
 * Coordinates are raw (un-mirrored) camera pixels: turning to the person's own left raises yaw.
 *
 * Each step has its own time budget (hr.face.step_timeouts_ms, sent by the server), started when the step
 * begins: "find" covers finding the face and holding still, "action" the blink or turn, "return" facing the
 * camera again. The strict face checks (one face, big enough, frontal) apply to the neutral frames; during the
 * action and the return a brief face loss or a smaller box (a turned head) does not reset anything.
 */

export type Point = [number, number];
export type LivenessAction = "blink" | "turn_left" | "turn_right";
/**
 * The actions a new check asks for (6 Nov 2026, Owner: blink was never reliably detected on phones), as the
 * server's services/src/face/liveness.rs ISSUED_ACTIONS.
 */
export const ISSUED_ACTIONS: readonly LivenessAction[] = ["turn_left", "turn_right"] as const;

export const ACTION_PROMPT: Record<LivenessAction, string> = {
  blink: "Blink your eyes",
  turn_left: "Turn your head to your left",
  turn_right: "Turn your head to your right",
};

/** Per-step budgets in milliseconds (hr.face.step_timeouts_ms). */
export interface StepTimeouts {
  find: number;
  action: number;
  return: number;
}

// Server: EYE_OPEN_MIN 0.20, BLINK_RATIO 0.70, FRONTAL_MAX_YAW 0.12, TURN_MIN_DELTA 0.20, MIN_FACE_PX 120
export const CLIENT = {
  eyeOpenMin: 0.21,
  blinkRatio: 0.66,
  /** The server compares the blink with the more closed of the two neutral frames: the last frame must keep it. */
  returnBlinkRatio: 0.69,
  frontalMaxYaw: 0.10,
  turnMinDelta: 0.22,
  /** Neutral frames (find/hold and the final capture). */
  minFacePx: 125,
  /** The action frame: the server re-detects it and needs MIN_FACE_PX too; a turned head gives a smaller box. */
  minActionFacePx: 122,
  steadyTicks: 2,
  /** Ticks without a face during the action or the return before "Look at the camera" shows; nothing resets. */
  lostTicks: 3,
};

const dist = (a: Point, b: Point) => Math.hypot(a[0] - b[0], a[1] - b[1]);

/** Mean eye aspect ratio of both eyes (points 36–41 and 42–47). */
export function eyeAspectRatio(lm: Point[]): number {
  const ear = (i: number) => {
    const p = lm.slice(i, i + 6);
    return (dist(p[1], p[5]) + dist(p[2], p[4])) / (2 * dist(p[0], p[3]) || 1e-6);
  };
  return (ear(36) + ear(42)) / 2;
}

/** Head-turn proxy: nose tip (30) between the jaw edges (0, 16); 0 = facing the camera. */
export function yaw(lm: Point[]): number {
  const width = lm[16][0] - lm[0][0];
  return Math.abs(width) > 1e-6 ? (lm[30][0] - lm[0][0]) / width - 0.5 : 0;
}

export interface Sample {
  /** Faces found in this video frame (only a single one is acceptable). */
  faces: number;
  /** Smaller side of the face box, in video pixels. */
  facePx: number;
  landmarks: Point[] | null;
}

export type Step = "find_face" | "hold_still" | "do_action" | "return" | "done" | "timeout";
/** The budget a step counts against. */
export type Budget = keyof StepTimeouts;

export const HINT = {
  look: "Look at the camera",
  onePerson: "Only one person in view, please",
  closer: "Move closer",
  hold: "Hold still",
  face: "Face the camera and hold still",
  blink: "Blink once, slowly",
  more: "Turn a little more",
  otherWay: "Turn the other way",
  back: "Now face the camera again",
  openEyes: "Open your eyes and face the camera",
} as const;

export interface TrackerUpdate {
  step: Step;
  /** Grab the current video frame as key frame 0, 1 or 2 (1 may be replaced by a better one later). */
  capture?: 0 | 1 | 2;
  hint?: string;
  /** The budget running now, its length and what is left of it (ms), for the countdown ring. */
  budget?: Budget;
  budgetMs?: number;
  remainingMs?: number;
}

const budgetOf = (s: Step): Budget | null =>
  s === "find_face" || s === "hold_still" ? "find" : s === "do_action" ? "action" : s === "return" ? "return" : null;

/** Feed one sample per animation tick; it tells the caller what to show and when to capture. */
export class ChallengeTracker {
  step: Step = "find_face";
  /** The step the check was in when it timed out (or the last step reached). */
  reached: Step = "find_face";
  private steady = 0;
  private lost = 0;
  private baseEar = 0;
  private baseYaw = 0;
  private best = 0;          // best action strength so far (EAR drop, or yaw change in the right direction)
  private bestEar = Infinity;
  private acted = false;
  private stepStarted: number | null = null;
  private budgetNow: Budget = "find";

  constructor(readonly action: LivenessAction, readonly timeouts: StepTimeouts) {}

  private enter(step: Step, now: number) {
    const b = budgetOf(step);
    if (b && b !== this.budgetNow) {
      this.budgetNow = b;
      this.stepStarted = now;
    }
    this.step = step;
    if (step !== "timeout") this.reached = step;
  }

  private out(u: Omit<TrackerUpdate, "budget" | "budgetMs" | "remainingMs">, now: number): TrackerUpdate {
    if (u.step === "done" || u.step === "timeout") return u;
    const budgetMs = this.timeouts[this.budgetNow];
    return { ...u, budget: this.budgetNow, budgetMs, remainingMs: Math.max(0, budgetMs - (now - (this.stepStarted ?? now))) };
  }

  update(s: Sample, now: number = Date.now()): TrackerUpdate {
    if (this.step === "done" || this.step === "timeout") return { step: this.step };
    // The first sample starts the clock: engine warm-up and camera start are never counted.
    if (this.stepStarted === null) this.stepStarted = now;
    if (now - this.stepStarted > this.timeouts[this.budgetNow]) {
      this.step = "timeout";
      return { step: "timeout" };
    }
    const r = this.judge(s, now);
    return this.out(r, now);
  }

  private judge(s: Sample, now: number): TrackerUpdate {
    const neutral = this.step === "find_face" || this.step === "hold_still";
    const noFace = s.faces === 0 || !s.landmarks;

    if (s.faces > 1) return { step: this.step, hint: HINT.onePerson };
    if (neutral) {
      if (noFace) { this.steady = 0; return { step: this.step, hint: HINT.look }; }
      if (s.facePx < CLIENT.minFacePx) { this.steady = 0; return { step: this.step, hint: HINT.closer }; }
    } else if (noFace) {
      // During the action or the return a turned head can drop the face for a tick or two: keep everything.
      this.lost += 1;
      if (this.step === "do_action" && this.acted) this.enter("return", now);
      return { step: this.step, hint: this.lost > CLIENT.lostTicks ? HINT.look : this.actionHint(null) };
    }
    this.lost = 0;
    const lm = s.landmarks as Point[];
    const e = eyeAspectRatio(lm);
    const y = yaw(lm);
    const frontal = Math.abs(y) <= CLIENT.frontalMaxYaw && e >= CLIENT.eyeOpenMin;

    if (neutral) {
      this.steady = frontal ? this.steady + 1 : 0;
      this.enter("hold_still", now);
      if (this.steady < CLIENT.steadyTicks) return { step: this.step, hint: frontal ? HINT.hold : HINT.face };
      this.baseEar = e;
      this.baseYaw = y;
      this.enter("do_action", now);
      return { step: this.step, capture: 0, hint: this.actionHint(null) };
    }

    if (this.step === "do_action") {
      const strength = this.action === "blink" ? this.baseEar - e
        : this.action === "turn_left" ? y - this.baseYaw : this.baseYaw - y;
      // A blink is caught on whichever tick saw the eyes closed (slow ticks can miss most of it).
      const enough = this.action === "blink" ? e <= CLIENT.blinkRatio * this.baseEar : strength >= CLIENT.turnMinDelta;
      const capturable = s.faces === 1 && s.facePx >= CLIENT.minActionFacePx;
      if (enough && capturable && strength > this.best) {
        this.best = strength;
        if (this.action === "blink") this.bestEar = e;
        this.acted = true;
        return { step: this.step, capture: 1 };
      }
      if (enough && !capturable && !this.acted) return { step: this.step, hint: HINT.closer };
      if (!this.acted) return { step: this.step, hint: this.actionHint(strength) };
      // The action is done (this tick was no better): the return gets its own budget from here.
      this.enter("return", now);
    }

    if (this.step === "return") {
      if (!frontal) return { step: this.step, hint: HINT.back };
      // The final neutral frame is strict again.
      if (s.facePx < CLIENT.minFacePx) return { step: this.step, hint: HINT.closer };
      if (this.action === "blink" && this.bestEar > CLIENT.returnBlinkRatio * Math.min(this.baseEar, e)) {
        return { step: this.step, hint: HINT.openEyes };
      }
      this.enter("done", now);
      return { step: "done", capture: 2 };
    }
    return { step: this.step };
  }

  private actionHint(strength: number | null): string | undefined {
    if (this.step === "return") return HINT.back;
    if (this.step !== "do_action") return undefined;
    if (this.action === "blink") return HINT.blink;
    if (strength === null) return undefined;
    if (strength <= -CLIENT.turnMinDelta / 2) return HINT.otherWay;
    if (strength >= CLIENT.turnMinDelta / 3) return HINT.more;
    return undefined;
  }
}
