/**
 * The active check's runner after a passive flash (6 Nov 2026, the Owner's Android 10 phone: one tracker tick in
 * 14-18 s right after the flash): the video is resumed and the engine re-warmed first, and a stalled detection
 * moves the engine to the CPU once and the check goes on.
 */
import { describe, expect, it, vi } from "vitest";

import { ensureVideoPlaying } from "@/hooks/useCamera";
import { runChallenge, type ChallengeDeps } from "@/lib/faceChallenge";
import type { DetectedFace } from "@/lib/faceEngine";

const STEPS = { find: 8000, action: 10000, return: 5000 };
const never = () => new Promise<never>(() => {});

/** deps whose detections return no face, plus the given overrides; aborts after `ticks` progress calls. */
function setup(overrides: ChallengeDeps = {}, ticks = 3) {
  const ctl = new AbortController();
  let seen = 0;
  const onProgress = vi.fn(() => { seen += 1; if (seen >= ticks) ctl.abort(); });
  const deps: ChallengeDeps = {
    track: vi.fn(async () => [] as DetectedFace[]),
    grab: vi.fn(),
    ensureVideo: vi.fn(async () => false),
    warm: vi.fn(async () => {}),
    rewarm: vi.fn(async () => {}),
    toCpu: vi.fn(async () => true),
    contextLost: vi.fn(() => false),
    stallMs: 30,
    ...overrides,
  };
  return { ctl, onProgress, deps };
}

describe("runChallenge after a passive flash", () => {
  it("resumes the video and re-warms the engine before the first tick", async () => {
    const order: string[] = [];
    const { ctl, onProgress, deps } = setup({
      ensureVideo: vi.fn(async () => { order.push("video"); return true; }),
      rewarm: vi.fn(async () => { order.push("rewarm"); }),
      track: vi.fn(async () => { order.push("tick"); return []; }),
    }, 1);
    const r = await runChallenge({}, document.createElement("video"), "turn_left", STEPS, onProgress, ctl.signal,
      { afterFlash: true }, deps);
    expect(order.slice(0, 3)).toEqual(["video", "rewarm", "tick"]);
    expect(deps.warm).not.toHaveBeenCalled();
    expect(r.outcome).toBe("aborted");
    expect(r.note).toBe("video_resumed");
    expect(r.ticks).toBe(1);
    expect(r.firstTickMs).not.toBeNull();
  });

  it("without a flash only the usual one-time warm-up runs", async () => {
    const { ctl, onProgress, deps } = setup({}, 1);
    const r = await runChallenge({}, document.createElement("video"), "turn_right", STEPS, onProgress, ctl.signal, {}, deps);
    expect(deps.warm).toHaveBeenCalledTimes(1);
    expect(deps.ensureVideo).not.toHaveBeenCalled();
    expect(deps.rewarm).not.toHaveBeenCalled();
    expect(r.note).toBe("");
  });

  it("a stalled tick moves the engine to the CPU once and the check goes on", async () => {
    let calls = 0;
    const track = vi.fn(() => (++calls === 1 ? never() : Promise.resolve([] as DetectedFace[])));
    const { ctl, onProgress, deps } = setup({ track, contextLost: vi.fn(() => true) }, 4);
    const r = await runChallenge({}, document.createElement("video"), "turn_left", STEPS, onProgress, ctl.signal,
      { afterFlash: true }, deps);
    expect(deps.toCpu).toHaveBeenCalledTimes(1);
    expect(track.mock.calls.length).toBeGreaterThanOrEqual(4);
    expect(r.note).toBe("stall,webgl_lost,cpu_fallback");
    expect(r.ticks).toBeGreaterThanOrEqual(4);
    expect(r.firstTickMs).toBeGreaterThanOrEqual(25);
    // the stalled tick still counts against the steps (a frame without a face)
    expect(onProgress).toHaveBeenCalledTimes(4);
  });

  it("a stalled re-warm switches to the CPU before the person's steps start", async () => {
    const { ctl, onProgress, deps } = setup({ rewarm: vi.fn(never) }, 1);
    const r = await runChallenge({}, document.createElement("video"), "turn_left", STEPS, onProgress, ctl.signal,
      { afterFlash: true }, deps);
    expect(deps.toCpu).toHaveBeenCalledTimes(1);
    expect(r.note).toBe("stall,cpu_fallback");
    expect(r.ticks).toBe(1);
  });

  it("when the CPU switch fails the watchdog stays on, so the steps can still time out", async () => {
    const track = vi.fn(never);
    const { ctl, onProgress, deps } = setup({ track, toCpu: vi.fn(async () => false) }, 3);
    const r = await runChallenge({}, document.createElement("video"), "turn_left", STEPS, onProgress, ctl.signal, {}, deps);
    expect(deps.toCpu).toHaveBeenCalledTimes(1);
    expect(r.ticks).toBe(3);
    expect(r.note).toBe("stall");
  });

  it("an abort during a stuck detection ends the check at once", async () => {
    const ctl = new AbortController();
    const { onProgress, deps } = setup({ track: vi.fn(never), stallMs: 60_000 });
    setTimeout(() => ctl.abort(), 20);
    const r = await runChallenge({}, document.createElement("video"), "turn_left", STEPS, onProgress, ctl.signal, {}, deps);
    expect(r.outcome).toBe("aborted");
    expect(r.ticks).toBe(0);
  });
});

describe("ensureVideoPlaying", () => {
  it("leaves a playing video alone", async () => {
    const v = document.createElement("video");
    Object.defineProperty(v, "paused", { value: false });
    Object.defineProperty(v, "readyState", { value: 4 });
    const play = vi.fn();
    v.play = play;
    expect(await ensureVideoPlaying(v)).toBe(false);
    expect(play).not.toHaveBeenCalled();
  });

  it("plays a paused video and waits for a new frame", async () => {
    const v = document.createElement("video") as HTMLVideoElement & { requestVideoFrameCallback: (cb: () => void) => number };
    Object.defineProperty(v, "paused", { value: true });
    v.play = vi.fn(() => Promise.resolve());
    const frame = vi.fn((cb: () => void) => { setTimeout(cb, 5); return 1; });
    v.requestVideoFrameCallback = frame;
    expect(await ensureVideoPlaying(v, 1000)).toBe(true);
    expect(v.play).toHaveBeenCalledTimes(1);
    expect(frame).toHaveBeenCalledTimes(1);
  });

  it("gives up waiting after the limit", async () => {
    const v = document.createElement("video");
    Object.defineProperty(v, "paused", { value: true });
    v.play = vi.fn(() => Promise.reject(new Error("NotAllowedError")));
    const t0 = Date.now();
    expect(await ensureVideoPlaying(v, 30)).toBe(true);
    expect(Date.now() - t0).toBeLessThan(1000);
  });
});
