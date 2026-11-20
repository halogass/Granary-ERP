import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

import type { ChallengeResult } from "@/lib/faceChallenge";

const runChallenge = vi.fn();
const loadFaceEngine = vi.fn();
const faceChallenge = vi.fn();
const clockWithFrames = vi.fn();
const reportFaceAttempt = vi.fn();

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
  describeFaces: vi.fn(),
}));
vi.mock("@/lib/faceChallenge", () => ({ runChallenge: (...a: unknown[]) => runChallenge(...a) }));
vi.mock("@/lib/api", () => ({
  accessToken: () => Promise.resolve("user-token"),
  hrApi: {
    faceChallenge: (...a: unknown[]) => faceChallenge(...a),
    clockWithFrames: (...a: unknown[]) => clockWithFrames(...a),
    reportFaceAttempt: (...a: unknown[]) => reportFaceAttempt(...a),
    listOfficeLocations: () => Promise.resolve({ data: [] }),
    registerKiosk: vi.fn(),
  },
}));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: null }) }));

import PersonalClock from "./PersonalClock";
import KioskClock from "./KioskClock";

const STEPS = { find: 8000, action: 10000, return: 5000 };
const timeout: ChallengeResult = { outcome: "timeout", frames: null, step: "do_action", durationMs: 12500, avgTickMs: 310 };
const aborted: ChallengeResult = { outcome: "aborted", frames: null, step: "hold_still", durationMs: 900, avgTickMs: 120 };
const finished = (): ChallengeResult => ({
  outcome: "done", frames: [0, 1, 2].map(() => document.createElement("canvas")), step: "done", durationMs: 4200, avgTickMs: 150,
});

beforeEach(() => {
  loadFaceEngine.mockResolvedValue({ engine: true });
  reportFaceAttempt.mockResolvedValue({ status: 201 });
  Object.defineProperty(navigator, "geolocation", {
    configurable: true,
    value: {
      getCurrentPosition: (ok: (p: unknown) => void) => ok({ coords: { latitude: 35.68, longitude: 139.76, accuracy: 12 } }),
    },
  });
});

describe("PersonalClock", () => {
  it("reports a timeout, then Try again re-runs the check without reloading the engine", async () => {
    faceChallenge.mockResolvedValue({ data: { nonce: "n1", action: "blink", expires_in: 60, step_timeouts_ms: STEPS } });
    runChallenge.mockResolvedValueOnce(timeout).mockResolvedValueOnce(finished());
    clockWithFrames.mockResolvedValue({ status: 200, data: { status: "success", message: "Clocked in at 08:59" } });

    render(<PersonalClock eventType="clock_in" />);
    const button = await screen.findByRole("button", { name: /clock in/i });
    await waitFor(() => expect(button).not.toBeDisabled());
    fireEvent.click(button);

    expect(await screen.findByText(/The check timed out/)).toBeInTheDocument();
    expect(runChallenge.mock.calls[0][3]).toEqual(STEPS);   // the server's per-step budgets reach the tracker
    expect(reportFaceAttempt).toHaveBeenCalledWith({
      action: "blink", outcome: "timeout", step_reached: "do_action", duration_ms: 12500, avg_tick_ms: 310, backend: "webgl",
    });

    fireEvent.click(screen.getByRole("button", { name: /try again: clock in/i }));
    expect(await screen.findByText("Clocked in at 08:59")).toBeInTheDocument();
    expect(runChallenge).toHaveBeenCalledTimes(2);
    expect(loadFaceEngine).toHaveBeenCalledTimes(1);
    expect(reportFaceAttempt).toHaveBeenCalledTimes(2);
    expect(reportFaceAttempt.mock.calls[1][0]).toMatchObject({ outcome: "done", step_reached: "done" });
  });

  it("never lets a failed report break the clock-in", async () => {
    faceChallenge.mockResolvedValue({ data: { nonce: "n2", action: "turn_left", expires_in: 60, step_timeouts_ms: STEPS } });
    runChallenge.mockResolvedValueOnce(finished());
    reportFaceAttempt.mockRejectedValue(new Error("offline"));
    clockWithFrames.mockResolvedValue({ status: 200, data: { status: "success", message: "Clocked in" } });

    render(<PersonalClock eventType="clock_in" />);
    const button = await screen.findByRole("button", { name: /clock in/i });
    await waitFor(() => expect(button).not.toBeDisabled());
    fireEvent.click(button);
    expect(await screen.findByText("Clocked in")).toBeInTheDocument();
    expect(clockWithFrames).toHaveBeenCalledTimes(1);
  });
});

describe("KioskClock", () => {
  it("reports every outcome on the kiosk route and retries without reloading", async () => {
    localStorage.setItem("granary_kiosk_token", "kiosk_abc");
    const fetchMock = vi.fn(async (url: string) => {
      if (url.endsWith("/kiosk/session")) {
        return new Response(JSON.stringify({
          session: "kiosk-session", kiosk: { name: "Lobby", office_location: "HQ" }, step_timeouts_ms: STEPS,
        }), { status: 200 });
      }
      if (url.endsWith("/kiosk/gallery")) {
        return new Response(JSON.stringify({ version: "v1", match_distance: 0.5, ambiguity_margin: 0.05, employees: [] }), { status: 200 });
      }
      return new Response(JSON.stringify({ id: "x" }), { status: 201 });
    });
    vi.stubGlobal("fetch", fetchMock);
    runChallenge.mockResolvedValueOnce(timeout).mockResolvedValueOnce(aborted);

    render(<KioskClock />);
    const clockIn = await screen.findByRole("button", { name: /clock in/i });
    await waitFor(() => expect(clockIn).not.toBeDisabled());
    fireEvent.click(clockIn);

    expect(await screen.findByText("Timed out")).toBeInTheDocument();
    expect(runChallenge.mock.calls[0][3]).toEqual(STEPS);
    const reports = () => fetchMock.mock.calls.filter(([u]) => String(u).endsWith("/kiosk/face-attempts"));
    await waitFor(() => expect(reports()).toHaveLength(1));
    const [, init] = reports()[0] as unknown as [string, RequestInit];
    expect((init.headers as Record<string, string>).Authorization).toBe("Bearer kiosk-session");
    expect(JSON.parse(init.body as string)).toMatchObject({ outcome: "timeout", step_reached: "do_action", backend: "webgl" });

    fireEvent.click(screen.getByRole("button", { name: /try again/i }));
    await waitFor(() => expect(reports()).toHaveLength(2));
    expect(JSON.parse((reports()[1] as unknown as [string, RequestInit])[1].body as string)).toMatchObject({ outcome: "aborted" });
    expect(runChallenge).toHaveBeenCalledTimes(2);
    expect(loadFaceEngine).toHaveBeenCalledTimes(1);
    vi.unstubAllGlobals();
    localStorage.clear();
  });
});
