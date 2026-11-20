import { describe, expect, it } from "vitest";
import { CLIENT, ChallengeTracker, HINT, ISSUED_ACTIONS, eyeAspectRatio, yaw, type Point, type Sample, type StepTimeouts } from "./faceLiveness";

/** Same synthetic face as the backend fixtures: EAR = eyeOpen / 20, yaw shift moves the nose tip. */
function landmarks(eyeOpen = 6, yawShift = 0): Point[] {
  const pts: Point[] = Array.from({ length: 68 }, () => [0, 0]);
  for (let i = 0; i < 17; i++) pts[i] = [100 + i * 12.5, 200 - Math.abs(i - 8) * 5];
  for (let i = 27; i < 36; i++) pts[i] = [200 + yawShift, 170 + (i - 27) * 5];
  pts[30] = [200 + yawShift, 205];
  for (const [base, x0] of [[36, 130], [42, 230]] as const) {
    pts[base] = [x0, 150];
    pts[base + 1] = [x0 + 10, 150 - eyeOpen];
    pts[base + 2] = [x0 + 30, 150 - eyeOpen];
    pts[base + 3] = [x0 + 40, 150];
    pts[base + 4] = [x0 + 30, 150 + eyeOpen];
    pts[base + 5] = [x0 + 10, 150 + eyeOpen];
  }
  return pts;
}

const STEPS: StepTimeouts = { find: 8000, action: 10000, return: 5000 };
const sample = (lm: Point[] | null, faces = 1, facePx = 220): Sample => ({ faces: lm ? faces : 0, facePx, landmarks: lm });
const LOST = null;

function run(tracker: ChallengeTracker, frames: (Point[] | null | Sample)[], tickMs = 100, start = 1000) {
  const captures: number[] = [];
  const hints: (string | undefined)[] = [];
  let t = start;
  let last;
  for (const f of frames) {
    const s = f === null || Array.isArray(f) ? sample(f) : f;
    last = tracker.update(s, (t += tickMs));
    if (last.capture !== undefined) captures.push(last.capture);
    hints.push(last.hint);
  }
  return { captures, last, hints, t };
}

describe("face liveness maths", () => {
  it("matches the backend EAR and yaw", () => {
    expect(eyeAspectRatio(landmarks(6))).toBeCloseTo(0.3, 2);
    expect(yaw(landmarks())).toBeCloseTo(0, 2);
    expect(yaw(landmarks(6, 60))).toBeCloseTo(0.3, 2);
  });

  it("keeps the client just inside the server's thresholds", () => {
    // Server: EYE_OPEN_MIN 0.20, BLINK_RATIO 0.70, FRONTAL_MAX_YAW 0.12, TURN_MIN_DELTA 0.20, MIN_FACE_PX 120
    expect(CLIENT).toMatchObject({
      eyeOpenMin: 0.21, blinkRatio: 0.66, frontalMaxYaw: 0.10, turnMinDelta: 0.22, minFacePx: 125, steadyTicks: 2,
    });
    expect(CLIENT.eyeOpenMin).toBeGreaterThan(0.20);
    expect(CLIENT.blinkRatio).toBeLessThan(0.70);
    expect(CLIENT.returnBlinkRatio).toBeLessThan(0.70);
    expect(CLIENT.frontalMaxYaw).toBeLessThan(0.12);
    expect(CLIENT.turnMinDelta).toBeGreaterThan(0.20);
    expect(CLIENT.minActionFacePx).toBeGreaterThan(120);
  });
});

describe("ChallengeTracker", () => {
  const open = landmarks(6);

  it("captures neutral, blink, neutral", () => {
    const { captures, last } = run(new ChallengeTracker("blink", STEPS),
      [open, open, landmarks(1.6), landmarks(1.2), open]);
    expect(captures).toEqual([0, 1, 1, 2]);           // the deeper blink replaces the first action frame
    expect(last?.step).toBe("done");
  });

  it("accepts the softer client blink (0.66) the old client refused (0.62)", () => {
    // EAR 0.195 against 0.30 is a ratio of 0.65
    const { captures } = run(new ChallengeTracker("blink", STEPS), [open, open, landmarks(3.9), open]);
    expect(captures).toEqual([0, 1, 2]);
  });

  it("captures a left turn and waits for the return to centre", () => {
    const t = new ChallengeTracker("turn_left", STEPS);
    const { captures } = run(t, [open, open, landmarks(6, 55), landmarks(6, 40)]);
    expect(captures).toEqual([0, 1]);
    expect(t.step).toBe("return");
    expect(run(t, [open], 100, 2000).captures).toEqual([2]);
    expect(t.step).toBe("done");
  });

  it("does not accept the wrong direction or a still face", () => {
    const wrong = run(new ChallengeTracker("turn_right", STEPS), [open, open, landmarks(6, 55), open]);
    expect(wrong.captures).toEqual([0]);
    expect(wrong.hints).toContain(HINT.otherWay);
    const still = run(new ChallengeTracker("blink", STEPS), [open, open, open, open, open, open]);
    expect(still.captures).toEqual([0]);
    expect(still.hints.at(-1)).toBe(HINT.blink);
  });

  it("asks for a little more on a partial turn", () => {
    const r = run(new ChallengeTracker("turn_left", STEPS), [open, open, landmarks(6, 25)]);
    expect(r.hints.at(-1)).toBe(HINT.more);
  });

  it("refuses two faces and a small face in the neutral frames", () => {
    const t = new ChallengeTracker("blink", STEPS);
    expect(t.update(sample(open, 2), 100).hint).toBe(HINT.onePerson);
    expect(t.update(sample(open, 1, 60), 200).hint).toBe(HINT.closer);
    expect(t.update(sample(open, 1, 124), 300).hint).toBe(HINT.closer);
  });

  it("gives each step its own budget, started when the step begins", () => {
    const t = new ChallengeTracker("turn_left", STEPS);
    // 7.5 s finding the face is still inside find (8 s) ...
    expect(t.update(sample(LOST), 0).step).toBe("find_face");
    expect(t.update(sample(open), 7500).step).toBe("hold_still");
    const go = t.update(sample(open), 7600);
    expect(go.step).toBe("do_action");
    expect(go.budget).toBe("action");
    expect(go.remainingMs).toBe(10000);
    // ... and the action then gets its full 10 s, not what was left of one shared 8 s budget
    expect(t.update(sample(open), 17000).step).toBe("do_action");
    expect(t.update(sample(landmarks(6, 55)), 17500).capture).toBe(1);
    // return: 5 s from the moment the person starts facing back
    expect(t.update(sample(landmarks(6, 40)), 17600).step).toBe("return");
    expect(t.update(sample(landmarks(6, 40)), 22500).step).toBe("return");
    expect(t.update(sample(landmarks(6, 40)), 22700).step).toBe("timeout");
    expect(t.reached).toBe("return");
  });

  it("times out in find when no face shows up, and reports where", () => {
    const t = new ChallengeTracker("blink", STEPS);
    expect(t.update(sample(LOST), 0).step).toBe("find_face");
    expect(t.update(sample(LOST), 8100).step).toBe("timeout");
    expect(t.reached).toBe("find_face");
  });

  it("does not count the time before the first sample (engine warm-up)", () => {
    const t = new ChallengeTracker("blink", STEPS);
    // the tracker was made at 0, warm-up took 6 s: the find budget starts at the first sample
    const first = t.update(sample(open), 6000);
    expect(first.remainingMs).toBe(8000);
    expect(t.update(sample(open), 13900).step).toBe("do_action");
  });

  it.each([300, 400, 500])("completes blink and both turns with slow %i ms ticks", (tick) => {
    const blink = run(new ChallengeTracker("blink", STEPS), [open, open, open, landmarks(2), open], tick);
    expect(blink.last?.step).toBe("done");
    const left = run(new ChallengeTracker("turn_left", STEPS), [open, open, landmarks(6, 30), landmarks(6, 55), open], tick);
    expect(left.last?.step).toBe("done");
    expect(left.captures).toEqual([0, 1, 2]);
    const right = run(new ChallengeTracker("turn_right", STEPS), [open, open, landmarks(6, -55), open], tick);
    expect(right.last?.step).toBe("done");
  });

  it("a face lost for 2 ticks during a turn still completes", () => {
    const t = new ChallengeTracker("turn_left", STEPS);
    const r = run(t, [open, open, landmarks(6, 30), LOST, LOST, landmarks(6, 55), LOST, LOST, open]);
    expect(r.captures).toEqual([0, 1, 2]);
    expect(r.last?.step).toBe("done");
    // a brief loss does not nag
    expect(r.hints.slice(3, 5)).not.toContain(HINT.look);
  });

  it("a long face loss during the action asks to look at the camera but resets nothing", () => {
    const t = new ChallengeTracker("turn_left", STEPS);
    const r = run(t, [open, open, LOST, LOST, LOST, LOST, landmarks(6, 55), open]);
    expect(r.hints[5]).toBe(HINT.look);
    expect(r.captures).toEqual([0, 1, 2]);
  });

  it("a smaller box while turned still registers the turn", () => {
    const t = new ChallengeTracker("turn_left", STEPS);
    t.update(sample(open), 100);
    t.update(sample(open), 200);
    // 123 px: below the neutral minimum (125), above the action-frame minimum
    expect(t.update(sample(landmarks(6, 55), 1, 123), 300).capture).toBe(1);
    // the final neutral frame is strict again
    expect(t.update(sample(open, 1, 110), 400).hint).toBe(HINT.closer);
    expect(t.update(sample(open), 500).capture).toBe(2);
  });

  it("keeps the deepest blink and checks it against the last frame", () => {
    const t = new ChallengeTracker("blink", STEPS);
    const r = run(t, [open, open, landmarks(3.6), landmarks(5.0)]);
    // EAR 0.18 vs base 0.30 captured; last frame 0.25: 0.18 > 0.69 * 0.25, the server would refuse it
    expect(r.captures).toEqual([0, 1]);
    expect(t.step).toBe("return");
    expect(r.hints.at(-1)).toBe(HINT.openEyes);
    expect(run(t, [open], 100, 2000).captures).toEqual([2]);
  });

  it("holds after two steady frames", () => {
    const t = new ChallengeTracker("blink", STEPS);
    expect(t.update(sample(open), 100).hint).toBe(HINT.hold);
    expect(t.update(sample(open), 200).capture).toBe(0);
  });
});

describe("issued actions", () => {
  it("asks only for head turns, never a blink", () => {
    expect([...ISSUED_ACTIONS]).toEqual(["turn_left", "turn_right"]);
    expect(ISSUED_ACTIONS).not.toContain("blink");
  });
});
