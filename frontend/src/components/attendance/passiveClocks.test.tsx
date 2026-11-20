/**
 * Release 2: passive liveness (colour flash + anti-spoof) on both clocks, with the camera, the face engine,
 * the flash capture and the ONNX models mocked. The flash judgement itself runs for real on synthetic pixels.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

import type { ChallengeResult } from "@/lib/faceChallenge";

type Rgb = [number, number, number];
type Px = { data: Uint8ClampedArray; width: number; height: number };

const runChallenge = vi.fn();
const loadFaceEngine = vi.fn();
const describeFaces = vi.fn();
const faceChallenge = vi.fn();
const clockWithFrames = vi.fn();
const clockPassive = vi.fn();
const reportFaceAttempt = vi.fn();
const runFlash = vi.fn();
const spoofScore = vi.fn();
const loadAntiSpoof = vi.fn();
/** Synthetic pixels per canvas: the mocked canvasPixels reads these (jsdom has no 2-D canvas). */
const pixels = new Map<HTMLCanvasElement, Px>();

const video = document.createElement("video");
const camera = { videoRef: { current: video }, ready: true, error: null, retry: vi.fn() };

vi.mock("@/hooks/useCamera", () => ({
  useCamera: () => camera,
  canvasToJpeg: () => Promise.resolve(new Blob(["x"], { type: "image/jpeg" })),
  grabFrame: vi.fn(),
  cpuCanvas: () => document.createElement("canvas"),
}));
vi.mock("@/lib/faceEngine", () => ({
  loadFaceEngine: (...a: unknown[]) => loadFaceEngine(...a),
  unloadFaceEngine: () => Promise.resolve(),
  warmUpEngine: () => Promise.resolve(),
  engineBackend: () => "webgl",
  describeFaces: (...a: unknown[]) => describeFaces(...a),
}));
vi.mock("@/lib/faceChallenge", () => ({ runChallenge: (...a: unknown[]) => runChallenge(...a) }));
vi.mock("@/lib/antiSpoof", () => ({
  loadAntiSpoof: (...a: unknown[]) => loadAntiSpoof(...a),
  unloadAntiSpoof: () => Promise.resolve(),
}));
vi.mock("@/lib/faceFlash", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/faceFlash")>()),
  NOTICE_MS: 0,
  runFlash: (...a: unknown[]) => runFlash(...a),
  canvasPixels: (c: HTMLCanvasElement) => pixels.get(c) ?? null,
}));
vi.mock("@/lib/api", () => ({
  accessToken: () => Promise.resolve("user-token"),
  hrApi: {
    faceChallenge: (...a: unknown[]) => faceChallenge(...a),
    clockWithFrames: (...a: unknown[]) => clockWithFrames(...a),
    clockPassive: (...a: unknown[]) => clockPassive(...a),
    reportFaceAttempt: (...a: unknown[]) => reportFaceAttempt(...a),
    listOfficeLocations: () => Promise.resolve({ data: [] }),
    registerKiosk: vi.fn(),
  },
}));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: null }) }));

import PersonalClock from "./PersonalClock";
import KioskClock from "./KioskClock";

const STEPS = { find: 8000, action: 10000, return: 5000 };
const PALETTE: Rgb[] = [[255, 120, 120], [120, 255, 140], [120, 150, 255], [255, 230, 120]];
const PASSIVE = {
  enabled: true, flash_count: 3, flash_ms: 400, settle_ms: 340, palette: PALETTE,
  flash_min_delta: 1.5, flash_min_corr: 0.6, spoof_min_real: 0.7,
};
const DESCRIPTOR = Float32Array.from({ length: 128 }, (_, i) => (i % 7) / 10);

function uniform(rgb: Rgb): Px {
  const data = new Uint8ClampedArray(10 * 10 * 4);
  for (let i = 0; i < 100; i++) data.set([Math.round(rgb[0]), Math.round(rgb[1]), Math.round(rgb[2]), 255], i * 4);
  return { data, width: 10, height: 10 };
}

/** 68 landmarks: skin region x 2..6, y 2..6; eyes open (EAR 0.4) or closed (0.1); nose at `noseX` (50: facing the camera). */
function landmarks(eyesOpen: boolean, noseX = 50): [number, number][] {
  const lm = Array.from({ length: 68 }, () => [5, 5] as [number, number]);
  lm[3] = [2, 5]; lm[13] = [6, 5]; lm[28] = [4, 2]; lm[33] = [4, 6];
  lm[0] = [0, 0]; lm[16] = [100, 0]; lm[30] = [noseX, 0];
  const h = eyesOpen ? 2 : 0.5;
  for (const i of [36, 42]) {
    lm[i] = [0, 0]; lm[i + 3] = [10, 0];
    lm[i + 1] = [3, -h]; lm[i + 5] = [3, h]; lm[i + 2] = [7, -h]; lm[i + 4] = [7, h];
  }
  return lm;
}
const BOX = { x: 1, y: 1, width: 160, height: 170 };
const face = (eyesOpen = true, noseX = 50) => ({
  score: 0.99, box: BOX, landmarks: landmarks(eyesOpen, noseX), descriptor: DESCRIPTOR,
});

/** The flash mock: frames whose skin region reflects `gain` x each screen colour. */
function flashWith(gain: number) {
  runFlash.mockImplementation(async (_v: unknown, colors: Rgb[], _f: number, _s: number, show: (c: Rgb | null) => void) => {
    show(colors[0]);
    show(null);
    const neutral = document.createElement("canvas");
    pixels.set(neutral, uniform([100, 100, 100]));
    const flashes = colors.map((c) => {
      const canvas = document.createElement("canvas");
      pixels.set(canvas, uniform([100 + gain * c[0], 100 + gain * c[1], 100 + gain * c[2]]));
      return canvas;
    });
    return { neutral, flashes, colors, captureAtMs: [150, 450, 750], durationMs: 900, aborted: false };
  });
}

/** A head turn to the left the active check accepts: the middle frame (marked) has the nose turned (yaw +0.3). */
function activeTurnPasses() {
  describeFaces.mockImplementation(async (_api: unknown, c: HTMLCanvasElement) => [face(true, c.dataset.turned ? 80 : 50)]);
  runChallenge.mockImplementation(async (): Promise<ChallengeResult> => {
    const frames = [0, 1, 2].map(() => document.createElement("canvas"));
    frames[1].dataset.turned = "1";
    return { outcome: "done", frames, step: "done", durationMs: 4000, avgTickMs: 150, firstTickMs: 700, ticks: 9, note: "" };
  });
}

function kioskFetch(passive: unknown) {
  return vi.fn(async (url: string, _init?: RequestInit) => {
    if (url.endsWith("/kiosk/session")) {
      return new Response(JSON.stringify({
        session: "kiosk-session", kiosk: { name: "Lobby", office_location: "HQ" }, step_timeouts_ms: STEPS, passive,
      }), { status: 200 });
    }
    if (url.endsWith("/kiosk/gallery")) {
      return new Response(JSON.stringify({
        version: "v1", match_distance: 0.5, ambiguity_margin: 0.05,
        employees: [{ employee_id: "emp-1", name: "Rachel", descriptors: [Array.from(DESCRIPTOR)] }],
      }), { status: 200 });
    }
    if (url.endsWith("/kiosk/clock")) {
      return new Response(JSON.stringify({ employee_name: "Rachel", message: "Clocked in at 08:01" }), { status: 200 });
    }
    return new Response(JSON.stringify({ id: "x" }), { status: 201 });
  });
}

const bodies = (f: ReturnType<typeof kioskFetch>, suffix: string) =>
  f.mock.calls.filter(([u]) => String(u).endsWith(suffix)).map(([, init]) => JSON.parse(String(init?.body)));

async function startKiosk(passive: unknown) {
  localStorage.setItem("granary_kiosk_token", "kiosk_abc");
  const fetchMock = kioskFetch(passive);
  vi.stubGlobal("fetch", fetchMock);
  render(<KioskClock />);
  const clockIn = await screen.findByRole("button", { name: /clock in/i });
  await waitFor(() => expect(clockIn).not.toBeDisabled());
  fireEvent.click(clockIn);
  return fetchMock;
}

beforeEach(() => {
  for (const m of [runChallenge, describeFaces, runFlash, clockPassive, clockWithFrames, faceChallenge]) m.mockReset();
  pixels.clear();
  loadFaceEngine.mockResolvedValue({ engine: true });
  reportFaceAttempt.mockResolvedValue({ status: 201 });
  spoofScore.mockReset().mockResolvedValue(0.92);
  loadAntiSpoof.mockReset().mockResolvedValue({ score: spoofScore, release: () => Promise.resolve() });
  // The kiosk draws its action with crypto: all zeros -> "turn_left" (only head turns are asked for).
  vi.spyOn(crypto, "getRandomValues").mockImplementation(((a: ArrayBufferView) => {
    new Uint8Array(a.buffer, a.byteOffset, a.byteLength).fill(0);
    return a;
  }) as typeof crypto.getRandomValues);
  Object.defineProperty(navigator, "geolocation", {
    configurable: true,
    value: {
      getCurrentPosition: (ok: (p: unknown) => void) => ok({ coords: { latitude: 35.68, longitude: 139.76, accuracy: 12 } }),
    },
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  localStorage.clear();
});

describe("KioskClock passive liveness", () => {
  it("a passing flash clocks in with method passive and no active step", async () => {
    flashWith(0.1);
    describeFaces.mockResolvedValue([face()]);
    const fetchMock = await startKiosk(PASSIVE);

    expect(await screen.findByText("Clocked in at 08:01")).toBeInTheDocument();
    expect(loadAntiSpoof).toHaveBeenCalledWith(expect.stringMatching(/\/api\/v1\/hr\/kiosk\/engine$/), "kiosk-session");
    const colors = runFlash.mock.calls[0][1] as Rgb[];
    expect(colors).toHaveLength(3);
    for (const c of colors) expect(PALETTE).toContainEqual(c);
    for (let i = 1; i < colors.length; i++) expect(colors[i]).not.toEqual(colors[i - 1]);
    expect(runChallenge).not.toHaveBeenCalled();
    const [body] = bodies(fetchMock, "/kiosk/clock");
    expect(body.employee_id).toBe("emp-1");
    expect(body.liveness.passed).toBe(true);
    expect(body.liveness.method).toBe("passive");
    expect(body.liveness.metrics.passive).toMatchObject({ corr: 1, spoof_real: 0.92, lag: 0 });
    expect(body.liveness.metrics.passive.delta).toBeGreaterThan(1.5);
    expect(body.liveness.metrics.passive.fallback_reason).toBeUndefined();
    expect(spoofScore.mock.calls[0][1]).toEqual(BOX);
    expect(bodies(fetchMock, "/kiosk/face-attempts")).toHaveLength(0);
  });

  it("an inconclusive flash is reported, then the active check runs and posts method active with the reason", async () => {
    flashWith(0);   // no colour response at all
    activeTurnPasses();
    const fetchMock = await startKiosk(PASSIVE);

    expect(await screen.findByText("Clocked in at 08:01")).toBeInTheDocument();
    expect(runChallenge).toHaveBeenCalledTimes(1);
    expect(runChallenge.mock.calls[0][2]).toBe("turn_left");
    // after the flash: the video is resumed and the engine re-warmed under the watchdog
    expect(runChallenge.mock.calls[0][6]).toEqual({ afterFlash: true });
    const attempts = bodies(fetchMock, "/kiosk/face-attempts");
    expect(attempts[0]).toMatchObject({
      outcome: "passive_inconclusive", action: "turn_left", step_reached: "done", employee_id: "emp-1", backend: "webgl",
      reason: "flash_inconclusive", corr: 0, delta: 0, spoof_real: 0.92, lag: 0,
    });
    expect(attempts[1]).toMatchObject({ outcome: "done", first_tick_ms: 700, ticks: 9 });
    expect(attempts[1].note).toBeUndefined();
    const [body] = bodies(fetchMock, "/kiosk/clock");
    expect(body.action).toBe("turn_left");
    expect(body.liveness.method).toBe("active");
    expect(body.liveness.passed).toBe(true);
    expect(body.liveness.metrics.passive).toEqual({ corr: 0, delta: 0, spoof_real: 0.92, lag: 0, fallback_reason: "flash_inconclusive" });
    expect(body.liveness.metrics.yaw).toHaveLength(3);
  });

  it("a low anti-spoof score falls back with reason spoof (reported as passive_failed)", async () => {
    flashWith(0.1);
    spoofScore.mockResolvedValue(0.2);
    describeFaces.mockResolvedValue([face()]);
    runChallenge.mockResolvedValue({ outcome: "timeout", frames: null, step: "do_action", durationMs: 12000, avgTickMs: 200 });
    const fetchMock = await startKiosk(PASSIVE);

    expect(await screen.findByText("Timed out")).toBeInTheDocument();
    expect(bodies(fetchMock, "/kiosk/face-attempts")[0]).toMatchObject({ outcome: "passive_failed", reason: "spoof", spoof_real: 0.2 });
    expect(bodies(fetchMock, "/kiosk/clock")).toHaveLength(0);
  });

  it("several faces or a tiny face on the neutral frame is no_face (not sure), never spoof", async () => {
    flashWith(0.1);
    spoofScore.mockResolvedValue(0.1);
    describeFaces.mockResolvedValueOnce([face(), face()]);
    runChallenge.mockResolvedValue({ outcome: "timeout", frames: null, step: "find_face", durationMs: 8000, avgTickMs: 200 });
    const fetchMock = await startKiosk(PASSIVE);

    expect(await screen.findByText("Timed out")).toBeInTheDocument();
    expect(bodies(fetchMock, "/kiosk/face-attempts")[0]).toMatchObject({ outcome: "passive_inconclusive", reason: "no_face" });
    expect(runChallenge).toHaveBeenCalledTimes(1);
    expect(spoofScore).not.toHaveBeenCalled();
  });

  it("a face smaller than the server's minimum is no_face", async () => {
    flashWith(0.1);
    describeFaces.mockResolvedValueOnce([{ ...face(), box: { x: 1, y: 1, width: 60, height: 70 } }]);
    runChallenge.mockResolvedValue({ outcome: "timeout", frames: null, step: "find_face", durationMs: 8000, avgTickMs: 200 });
    const fetchMock = await startKiosk(PASSIVE);

    expect(await screen.findByText("Timed out")).toBeInTheDocument();
    expect(bodies(fetchMock, "/kiosk/face-attempts")[0]).toMatchObject({ outcome: "passive_inconclusive", reason: "no_face" });
    expect(spoofScore).not.toHaveBeenCalled();
  });

  it("asks only for head turns, never a blink", async () => {
    const seen = new Set<string>();
    for (const fill of [0, 1, 2, 3]) {
      vi.spyOn(crypto, "getRandomValues").mockImplementation(((a: ArrayBufferView) => {
        new Uint32Array(a.buffer, a.byteOffset, a.byteLength / 4).fill(fill);
        return a;
      }) as typeof crypto.getRandomValues);
      activeTurnPasses();
      const fetchMock = await startKiosk({ ...PASSIVE, enabled: false });
      await waitFor(() => expect(bodies(fetchMock, "/kiosk/face-attempts").length + bodies(fetchMock, "/kiosk/clock").length).toBeGreaterThan(0));
      seen.add(runChallenge.mock.calls.at(-1)![2]);
      cleanup();
      vi.unstubAllGlobals();
    }
    expect([...seen].sort()).toEqual(["turn_left", "turn_right"]);
  });

  it("no face on the neutral frame falls back with reason no_face", async () => {
    flashWith(0.1);
    describeFaces.mockResolvedValueOnce([]);
    runChallenge.mockResolvedValue({ outcome: "timeout", frames: null, step: "find_face", durationMs: 8000, avgTickMs: 200 });
    const fetchMock = await startKiosk(PASSIVE);

    expect(await screen.findByText("Timed out")).toBeInTheDocument();
    expect(bodies(fetchMock, "/kiosk/face-attempts")[0]).toMatchObject({ outcome: "passive_inconclusive", step_reached: "find_face" });
    expect(spoofScore).not.toHaveBeenCalled();
  });

  it("passive disabled keeps the old active-only flow", async () => {
    activeTurnPasses();
    const fetchMock = await startKiosk({ ...PASSIVE, enabled: false });

    expect(await screen.findByText("Clocked in at 08:01")).toBeInTheDocument();
    expect(runFlash).not.toHaveBeenCalled();
    expect(runChallenge.mock.calls[0][6]).toEqual({ afterFlash: false });
    expect(loadAntiSpoof).not.toHaveBeenCalled();
    const [body] = bodies(fetchMock, "/kiosk/clock");
    expect(body.liveness.method).toBeUndefined();
    expect(body.liveness.metrics.passive).toBeUndefined();
  });

  it("models that fail to load leave the active check only", async () => {
    loadAntiSpoof.mockRejectedValue(new Error("anti-spoof model unavailable (404)"));
    activeTurnPasses();
    const fetchMock = await startKiosk(PASSIVE);

    expect(await screen.findByText("Clocked in at 08:01")).toBeInTheDocument();
    expect(runFlash).not.toHaveBeenCalled();
    expect(bodies(fetchMock, "/kiosk/clock")[0].liveness.method).toBeUndefined();
  });
});

describe("PersonalClock passive liveness", () => {
  const SERVER_COLORS: Rgb[] = [[255, 120, 120], [120, 150, 255], [255, 120, 120]];
  const finished = (): ChallengeResult => ({
    outcome: "done", frames: [0, 1, 2].map(() => document.createElement("canvas")), step: "done", durationMs: 4200, avgTickMs: 150,
  });

  beforeEach(() => {
    flashWith(0.1);
    faceChallenge.mockResolvedValue({ data: {
      nonce: "n-passive", action: "turn_right", expires_in: 60, step_timeouts_ms: STEPS,
      passive: { enabled: true, colors: SERVER_COLORS, flash_ms: 400, settle_ms: 340 },
    } });
  });

  async function clickClockIn() {
    render(<PersonalClock eventType="clock_in" />);
    const button = await screen.findByRole("button", { name: /clock in/i });
    await waitFor(() => expect(button).not.toBeDisabled());
    fireEvent.click(button);
  }

  it("flashes the server's colours and posts the passive frames", async () => {
    clockPassive.mockResolvedValue({ status: 200, data: { status: "success", message: "Clocked in at 08:02" } });
    await clickClockIn();

    expect(await screen.findByText("Clocked in at 08:02")).toBeInTheDocument();
    expect(runFlash.mock.calls[0].slice(1, 4)).toEqual([SERVER_COLORS, 400, 340]);
    expect(clockPassive.mock.calls[0][0]).toBe("clock_in");
    const fd = clockPassive.mock.calls[0][1] as FormData;
    expect(fd.get("nonce")).toBe("n-passive");
    expect(fd.get("mode")).toBe("passive");
    expect(fd.get("frame_neutral")).toBeInstanceOf(Blob);
    for (const i of [0, 1, 2]) expect(fd.get(`flash_${i}`)).toBeInstanceOf(Blob);
    expect(fd.get("flash_3")).toBeNull();
    expect(fd.get("latitude")).toBe("35.68");
    expect(fd.get("geo_accuracy_meters")).toBe("12");
    expect(runChallenge).not.toHaveBeenCalled();
    expect(clockWithFrames).not.toHaveBeenCalled();
  });

  it("active_required runs the active step with the returned challenge", async () => {
    const steps = { find: 9000, action: 11000, return: 6000 };
    clockPassive.mockResolvedValue({ status: 200, data: {
      active_required: true, reason: "flash_inconclusive",
      challenge: { nonce: "n-active", action: "turn_left", expires_in: 60, step_timeouts_ms: steps },
    } });
    let release: (r: ChallengeResult) => void = () => {};
    runChallenge.mockImplementation(() => new Promise<ChallengeResult>((r) => { release = r; }));
    clockWithFrames.mockResolvedValue({ status: 200, data: { status: "success", message: "Clocked in at 08:03" } });
    await clickClockIn();

    expect(await screen.findByText("One more step")).toBeInTheDocument();
    expect(runChallenge.mock.calls[0][2]).toBe("turn_left");
    expect(runChallenge.mock.calls[0][3]).toEqual(steps);
    // the active step after the flash resumes the video and re-warms the engine first
    expect(runChallenge.mock.calls[0][6]).toEqual({ afterFlash: true });
    release({ ...finished(), firstTickMs: 3400, ticks: 22, note: "stall,webgl_lost,cpu_fallback" });
    expect(await screen.findByText("Clocked in at 08:03")).toBeInTheDocument();
    const fd = clockWithFrames.mock.calls[0][1] as FormData;
    expect(fd.get("nonce")).toBe("n-active");
    expect(fd.get("mode")).toBeNull();
    for (const i of [0, 1, 2]) expect(fd.get(`frame_${i}`)).toBeInstanceOf(Blob);
    expect(fd.get("longitude")).toBe("139.76");
    expect(reportFaceAttempt.mock.calls[0][0]).toMatchObject({
      action: "turn_left", outcome: "done", first_tick_ms: 3400, ticks: 22, note: "stall,webgl_lost,cpu_fallback",
    });
  });

  it("without passive in the challenge nothing flashes (today's flow)", async () => {
    faceChallenge.mockResolvedValue({ data: { nonce: "n3", action: "turn_left", expires_in: 60, step_timeouts_ms: STEPS } });
    runChallenge.mockResolvedValueOnce(finished());
    clockWithFrames.mockResolvedValue({ status: 200, data: { status: "success", message: "Clocked in" } });
    await clickClockIn();

    expect(await screen.findByText("Clocked in")).toBeInTheDocument();
    expect(runFlash).not.toHaveBeenCalled();
    expect(runChallenge.mock.calls[0][6]).toEqual({ afterFlash: false });
    expect(clockPassive).not.toHaveBeenCalled();
    expect((clockWithFrames.mock.calls[0][1] as FormData).get("nonce")).toBe("n3");
  });
});
