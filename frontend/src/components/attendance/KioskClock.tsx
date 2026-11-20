"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { useSession } from "next-auth/react";
import { AlertTriangle, CheckCircle2, Clock, Loader2, Lock, RefreshCw, Tablet, XCircle } from "lucide-react";
import { hrApi, type FaceAttemptReport } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { useCamera } from "@/hooks/useCamera";
import {
  describeFaces, engineBackend, loadFaceEngine, unloadFaceEngine, warmUpEngine, type DetectedFace, type FaceApi,
} from "@/lib/faceEngine";
import { loadAntiSpoof, unloadAntiSpoof, type AntiSpoof } from "@/lib/antiSpoof";
import {
  FLASH_NOTICE, NOTICE_MS, cryptoRandomInt, pickColors, releaseCapture, runFlash, type PassiveSettings, type Rgb,
} from "@/lib/faceFlash";
import { judgeCapture, type PassiveMetrics } from "@/lib/facePassive";
import { runChallenge, type ChallengeProgress, type ChallengeResult } from "@/lib/faceChallenge";
import { attemptReport, fireAndForget, timeoutMessage } from "@/lib/faceAttempts";
import {
  ACTION_PROMPT, CLIENT, ISSUED_ACTIONS, eyeAspectRatio, yaw, type LivenessAction, type Point, type StepTimeouts,
} from "@/lib/faceLiveness";
import { Button, Field, Input, Select } from "@/components/ui";
import StepRing from "@/components/attendance/StepRing";
import FlashOverlay from "@/components/attendance/FlashOverlay";
import { cn } from "@/lib/utils";

const TOKEN_KEY = "granary_kiosk_token";
const API = "/api/v1/hr";
const GALLERY_REFRESH_MS = 10 * 60 * 1000;
const SESSION_RENEW_MS = 11 * 60 * 60 * 1000;
const SAME_FACE = 0.45;

interface Person { employee_id: string; name: string; descriptors: Float32Array[] }
interface Gallery { version: string; matchDistance: number; margin: number; people: Person[] }
type Mode = "boot" | "unregistered" | "loading" | "idle" | "flash" | "challenge" | "matching" | "result";
interface Outcome { kind: "success" | "pending" | "error"; title: string; detail?: string; retry?: "clock_in" | "clock_out" }

function readToken(): string | null {
  try { return localStorage.getItem(TOKEN_KEY); } catch { return null; }
}

function distance(a: Float32Array, b: Float32Array): number {
  let sum = 0;
  for (let i = 0; i < a.length; i++) sum += (a[i] - b[i]) ** 2;
  return Math.sqrt(sum);
}

/** Best and runner-up employee for the query descriptors, as the server applies it. */
function match(gallery: Gallery, queries: Float32Array[]) {
  const ranked = gallery.people.map((p) => ({
    person: p,
    d: Math.min(...p.descriptors.flatMap((ref) => queries.map((q) => distance(ref, q)))),
  })).sort((a, b) => a.d - b.d);
  const [best, second] = ranked;
  const accepted = !!best && best.d <= gallery.matchDistance && (!second || second.d - best.d >= gallery.margin);
  return { best, accepted };
}

/**
 * A registered, dedicated clock-in tablet. It authenticates first (device token, then kiosk session); only
 * then does it receive the face engine and the descriptor gallery, kept in memory only. Detection,
 * liveness and matching run here; the server re-checks the submitted embedding with its own search.
 */
export default function KioskClock() {
  const { data: session } = useSession();
  const [mode, setMode] = useState<Mode>("boot");
  const [kioskName, setKioskName] = useState("");
  const [office, setOffice] = useState("");
  const [bootError, setBootError] = useState<string | null>(null);
  const [action, setAction] = useState<LivenessAction | null>(null);
  const [hint, setHint] = useState<string | undefined>();
  const [progress, setProgress] = useState<ChallengeProgress | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const [flashColor, setFlashColor] = useState<Rgb | null>(null);
  const [stepNote, setStepNote] = useState<string | undefined>();
  const [now, setNow] = useState(new Date());
  const cam = useCamera(mode !== "unregistered" && mode !== "boot");

  const sessionRef = useRef<string | null>(null);
  const engineRef = useRef<FaceApi | null>(null);
  const galleryRef = useRef<Gallery | null>(null);
  const stepsRef = useRef<StepTimeouts | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const passiveRef = useRef<PassiveSettings | null>(null);
  const antiSpoofRef = useRef<AntiSpoof | null>(null);

  useEffect(() => () => abortRef.current?.abort(), []);
  useEffect(() => {
    const t = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(t);
  }, []);

  /** Forget everything face-related: the device is no longer trusted. */
  const wipe = useCallback(async (reason?: string) => {
    sessionRef.current = null;
    galleryRef.current = null;
    engineRef.current = null;
    antiSpoofRef.current = null;
    passiveRef.current = null;
    await unloadFaceEngine();
    await unloadAntiSpoof();
    setBootError(reason ?? null);
    setMode("unregistered");
  }, []);

  const authed = useCallback(async (path: string, init?: RequestInit) => {
    const res = await fetch(`${API}${path}`, {
      ...init, headers: { ...(init?.headers as Record<string, string>), Authorization: `Bearer ${sessionRef.current}` },
    });
    if (res.status === 401) {
      await wipe("This kiosk was revoked or its session ended. Register it again.");
      throw new Error("unauthorized");
    }
    return res;
  }, [wipe]);

  const openSession = useCallback(async (): Promise<boolean> => {
    const token = readToken();
    if (!token) { setMode("unregistered"); return false; }
    const res = await fetch(`${API}/kiosk/session`, { method: "POST", headers: { "X-Kiosk-Token": token } });
    if (!res.ok) {
      const detail = res.status === 400 ? (await res.json().catch(() => ({}))).detail : undefined;
      await wipe(res.status === 401 ? "This device is not registered as a kiosk (or was revoked)."
        : typeof detail === "string" ? detail : "Could not reach the server.");
      return false;
    }
    const body = await res.json();
    sessionRef.current = body.session;
    stepsRef.current = body.step_timeouts_ms;
    // Passive liveness settings (Release 2); absent or disabled -> the active challenge only.
    passiveRef.current = body.passive?.enabled ? body.passive : null;
    setKioskName(body.kiosk.name);
    setOffice(body.kiosk.office_location ?? "");
    return true;
  }, [wipe]);

  const loadGallery = useCallback(async () => {
    const res = await authed("/kiosk/gallery", {
      headers: galleryRef.current ? { "If-None-Match": galleryRef.current.version } : {},
    });
    if (res.status === 304) return;
    if (!res.ok) throw new Error("gallery");
    const body = await res.json();
    galleryRef.current = {
      version: body.version, matchDistance: body.match_distance, margin: body.ambiguity_margin,
      people: body.employees.map((e: { employee_id: string; name: string; descriptors: number[][] }) => ({
        employee_id: e.employee_id, name: e.name, descriptors: e.descriptors.map((d) => Float32Array.from(d)),
      })),
    };
  }, [authed]);

  /** Load the anti-spoof models when the passive check is on (once; a failure leaves the active check only). */
  const ensureAntiSpoof = useCallback(async (): Promise<AntiSpoof | null> => {
    if (!passiveRef.current || !sessionRef.current) return null;
    if (antiSpoofRef.current) return antiSpoofRef.current;
    try {
      antiSpoofRef.current = await loadAntiSpoof(`${window.location.origin}${API}/kiosk/engine`, sessionRef.current);
    } catch {
      antiSpoofRef.current = null;
    }
    return antiSpoofRef.current;
  }, []);

  const boot = useCallback(async () => {
    setBootError(null);
    setMode("boot");
    try {
      if (!(await openSession())) return;
      setMode("loading");
      engineRef.current = await loadFaceEngine(`${window.location.origin}${API}/kiosk/engine`, sessionRef.current!, "full");
      await warmUpEngine(engineRef.current);
      await ensureAntiSpoof();
      await loadGallery();
      setMode("idle");
    } catch (e) {
      if ((e as Error).message !== "unauthorized") {
        setBootError("The face engine could not be loaded. Check the connection and retry.");
        setMode("unregistered");
      }
    }
  }, [openSession, loadGallery, ensureAntiSpoof]);

  useEffect(() => { boot(); }, [boot]);

  // Keep the gallery fresh and the session alive while the kiosk runs.
  useEffect(() => {
    if (mode === "unregistered" || mode === "boot") return;
    const g = setInterval(() => { loadGallery().catch(() => {}); }, GALLERY_REFRESH_MS);
    const s = setInterval(() => { openSession().catch(() => {}); }, SESSION_RENEW_MS);
    return () => { clearInterval(g); clearInterval(s); };
  }, [mode, loadGallery, openSession]);

  // Results stay on screen for 5 s (10 s when "Try again" is offered), then the kiosk is ready for the next person.
  const retryable = outcome?.kind === "error" && !!outcome.retry;
  useEffect(() => {
    if (mode !== "result") return;
    const t = setTimeout(() => { setOutcome(null); setMode("idle"); }, retryable ? 10000 : 5000);
    return () => clearTimeout(t);
  }, [mode, retryable]);

  async function clock(eventType: "clock_in" | "clock_out") {
    const api = engineRef.current;
    const gallery = galleryRef.current;
    const video = cam.videoRef.current;
    const steps = stepsRef.current;
    if (!api || !gallery || !video || !steps) return;
    // Only head turns (6 Nov 2026: a blink was never reliably detected).
    const act = ISSUED_ACTIONS[cryptoRandomInt(ISSUED_ACTIONS.length)];
    setAction(act);
    setHint(undefined);
    setProgress(null);
    setOutcome(null);
    setStepNote(undefined);
    abortRef.current = new AbortController();
    const signal = abortRef.current.signal;
    const sendAttempt = (report: FaceAttemptReport) => {
      const session = sessionRef.current;
      if (!session) return;
      const body = JSON.stringify(report);
      fireAndForget(() => fetch(`${API}/kiosk/face-attempts`, {
        method: "POST", headers: { "Content-Type": "application/json", Authorization: `Bearer ${session}` }, body,
      }));
    };
    const report = (r: ChallengeResult, employeeId?: string) => sendAttempt(attemptReport(act, r, engineBackend(api), employeeId));
    const done = (o: Outcome) => { setOutcome(o); setMode("result"); };
    const onlyOne = () => done({ kind: "error", title: "Only one person, please", detail: "Stand alone in front of the camera and try again.", retry: eventType });

    /** Match the faces to the gallery and post the clock with this liveness. */
    const submit = async (faces: DetectedFace[], liveness: Record<string, unknown>, onMatched?: (employeeId?: string) => void) => {
      const { best, accepted } = match(gallery, faces.map((f) => f.descriptor!));
      if (!best || !accepted) {
        onMatched?.();
        return done({ kind: "error", title: "Not recognised", detail: "Try again, or ask HR to record your attendance.", retry: eventType });
      }
      onMatched?.(best.person.employee_id);
      const nearest = (f: DetectedFace) => best.person.descriptors.reduce((acc, ref) => Math.min(acc, distance(ref, f.descriptor!)), Infinity);
      const probe = faces.reduce((a, b) => (nearest(b) < nearest(a) ? b : a));
      try {
        const res = await authed("/kiosk/clock", {
          method: "POST", headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            event_type: eventType, employee_id: best.person.employee_id, descriptor: Array.from(probe.descriptor!),
            distance: Number(best.d.toFixed(4)), action: act, captured_at: new Date().toISOString(), liveness,
          }),
        });
        const body = await res.json().catch(() => ({}));
        if (res.status === 200) return done({ kind: "success", title: body.employee_name, detail: body.message });
        if (res.status === 202) return done({ kind: "pending", title: best.person.name, detail: body.message });
        const detail = typeof body.detail === "string" ? body.detail : body.detail?.message;
        return done({ kind: "error", title: res.status === 409 ? best.person.name : "Not recorded", detail: detail ?? "Please try again." });
      } catch (e) {
        if ((e as Error).message !== "unauthorized") done({ kind: "error", title: "Network error", detail: "Please try again." });
      }
    };

    // 1. Passive first (colour flash + anti-spoof model), when the setting is on and the models loaded.
    let passiveMetrics: PassiveMetrics | undefined;
    let afterFlash = false;
    const passive = passiveRef.current;
    const spoof = passive ? await ensureAntiSpoof() : null;
    if (passive && spoof) {
      setMode("flash");
      afterFlash = true;
      const began = performance.now();
      await new Promise((r) => setTimeout(r, NOTICE_MS));
      if (signal.aborted) { setMode("idle"); return; }
      const colors = pickColors(passive.palette, passive.flash_count);
      const capture = await runFlash(video, colors, passive.flash_ms, passive.settle_ms, setFlashColor, signal);
      setFlashColor(null);
      if (capture.aborted) { setMode("idle"); return; }
      setMode("matching");
      const faces = await describeFaces(api, capture.neutral);
      // several faces on the neutral frame: not sure (no_face), the active check (one person only) follows
      const face = faces.length === 1 && faces[0].descriptor ? faces[0] : null;
      const judged = await judgeCapture(capture, face, passive, spoof);
      releaseCapture(capture);
      if (judged.verdict.passed && face) {
        return submit([face], { passed: true, method: "passive", metrics: { passive: judged.metrics } });
      }
      passiveMetrics = judged.metrics;
      const known = face ? match(gallery, [face.descriptor!]) : null;
      sendAttempt({
        action: act,
        outcome: judged.verdict.passed ? "passive_failed" : judged.verdict.outcome,
        step_reached: face ? "done" : "find_face",
        duration_ms: Math.max(0, Math.min(600000, Math.round(performance.now() - began))),
        avg_tick_ms: null,
        backend: engineBackend(api),
        ...(known?.accepted && known.best ? { employee_id: known.best.person.employee_id } : {}),
        ...(judged.verdict.passed ? {} : { reason: judged.verdict.reason }),
        corr: judged.metrics.corr, delta: judged.metrics.delta, spoof_real: judged.metrics.spoof_real,
        ...(judged.metrics.lag !== undefined ? { lag: judged.metrics.lag } : {}),
      });
      setStepNote("One more step");
    }

    // 2. The active challenge (Release 1): always when passive is off, as the fallback when it did not pass.
    setMode("challenge");
    const result = await runChallenge(api, video, act, steps, (p) => { setHint(p.hint); setProgress(p); }, signal, { afterFlash });
    const frames = result.frames;
    if (result.outcome === "aborted") { report(result); setMode("idle"); return; }
    if (!frames) {
      report(result);
      return done({ kind: "error", title: "Timed out", detail: timeoutMessage(result.step), retry: eventType });
    }

    setMode("matching");
    const described = await Promise.all(frames.map((f) => describeFaces(api, f)));
    // The check itself finished: count it as done, with the employee once the tablet knows who it was.
    let reported = false;
    const reportDone = (employeeId?: string) => { if (!reported) { reported = true; report(result, employeeId); } };
    if (described.some((faces) => faces.length !== 1 || !faces[0].descriptor)) {
      reportDone();
      return onlyOne();
    }
    const [f0, f1, f2] = described.map((faces) => faces[0]);
    const lms = [f0, f1, f2].map((f) => f.landmarks as Point[]);
    const ears = lms.map(eyeAspectRatio);
    const yaws = lms.map(yaw);
    const same = [distance(f0.descriptor!, f2.descriptor!), Math.max(distance(f0.descriptor!, f1.descriptor!), distance(f2.descriptor!, f1.descriptor!))];
    const performed = act === "blink" ? ears[1] <= CLIENT.blinkRatio * Math.min(ears[0], ears[2])
      : act === "turn_left" ? yaws[1] - yaws[0] >= CLIENT.turnMinDelta : yaws[0] - yaws[1] >= CLIENT.turnMinDelta;
    const passed = performed && same[0] <= SAME_FACE && same[1] <= 0.55;
    if (!passed) {
      reportDone();
      return done({ kind: "error", title: "Liveness check failed", detail: "Look at the camera and follow the instruction.", retry: eventType });
    }
    const metrics = {
      ear: ears.map((e) => +e.toFixed(3)), yaw: yaws.map((y) => +y.toFixed(3)), same_face: same.map((d) => +d.toFixed(3)),
      ...(passiveMetrics ? { passive: passiveMetrics } : {}),
    };
    return submit([f0, f2], { passed, ...(passiveMetrics ? { method: "active" } : {}), metrics }, reportDone);
  }

  if (mode === "boot") {
    return <Screen><div className="flex justify-center py-16"><Loader2 className="size-8 animate-spin text-fg-muted" aria-label="Loading" /></div></Screen>;
  }
  if (mode === "unregistered") {
    return <Register error={bootError} signedIn={!!(session as { access_token?: string } | null)?.access_token} onRegistered={boot} />;
  }

  const timeStr = now.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit", second: "2-digit" });
  return (
    <Screen>
      <div className="bg-brand px-6 py-5 text-center text-brand-on">
        <div className="text-xs font-semibold uppercase tracking-widest opacity-80">{office || "Granary"}</div>
        <div className="text-5xl font-semibold tracking-wider tabular">{timeStr}</div>
        <div className="mt-1 text-sm opacity-80">
          {now.toLocaleDateString("en-GB", { weekday: "long", day: "numeric", month: "long", year: "numeric" })}
        </div>
      </div>
      {/* The camera frame stays black in both themes; overlays sit on the video. */}
      <div className="relative aspect-[4/3] overflow-hidden bg-black">
        <video ref={cam.videoRef} autoPlay playsInline muted className="h-full w-full scale-x-[-1] object-cover" />
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
          <div className={cn("h-52 w-40 rounded-full border-2 border-dashed",
            mode === "challenge" || mode === "flash" ? "border-tone-blue" : "border-white/30")} />
        </div>
        {mode === "flash" && (
          <div className="absolute inset-x-0 bottom-0 bg-black/70 px-4 py-4 text-center" aria-live="polite">
            <p className="text-xl font-semibold text-white">Look at the screen and hold still</p>
            <p className="mt-0.5 text-base text-white/80">{FLASH_NOTICE}</p>
          </div>
        )}
        <FlashOverlay color={flashColor} />
        {mode === "challenge" && action && (
          <>
            <div className="absolute right-3 top-3">
              <StepRing budget={progress?.budget} budgetMs={progress?.budgetMs} remainingMs={progress?.remainingMs} size={56} />
            </div>
            <div className="absolute inset-x-0 bottom-0 bg-black/70 px-4 py-4 text-center" aria-live="polite">
              {stepNote && <p className="text-sm font-medium uppercase tracking-wide text-white/70">{stepNote}</p>}
              <p className="text-xl font-semibold text-white">{ACTION_PROMPT[action]}</p>
              {hint && <p className="mt-0.5 text-base text-white/80">{hint}</p>}
            </div>
          </>
        )}
        {(mode === "loading" || mode === "matching") && (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 bg-black/70" aria-live="polite">
            <Loader2 className="size-10 animate-spin text-white" aria-hidden />
            <p className="text-base font-medium text-white">{mode === "loading" ? "Loading the face engine..." : "Recognising..."}</p>
          </div>
        )}
        {mode === "result" && outcome && (
          <div role="status" className={cn("absolute inset-0 flex flex-col items-center justify-center gap-2 p-4 text-center",
            outcome.kind === "success" ? "bg-tone-blue-soft" : outcome.kind === "pending" ? "bg-tone-orange-soft" : "bg-tone-red-soft")}>
            {outcome.kind === "success" ? <CheckCircle2 className="size-14 text-tone-blue" aria-hidden />
              : outcome.kind === "pending" ? <AlertTriangle className="size-14 text-tone-orange" aria-hidden />
              : <XCircle className="size-14 text-tone-red" aria-hidden />}
            <p className="text-xl font-semibold text-fg">{outcome.title}</p>
            {outcome.detail && <p className="text-base text-fg-muted">{outcome.detail}</p>}
            {outcome.kind === "error" && outcome.retry && (
              <Button variant="primary" size="lg" className="mt-2" onClick={() => clock(outcome.retry!)} disabled={!cam.ready}>
                <RefreshCw aria-hidden /> Try again
              </Button>
            )}
          </div>
        )}
        {cam.error && (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 bg-surface-2 p-4 text-center">
            <p className="text-base text-fg">{cam.error}</p>
            <Button onClick={cam.retry}>Retry camera</Button>
          </div>
        )}
      </div>
      <div className="space-y-3 p-4">
        <Button variant="primary" onClick={() => clock("clock_in")} disabled={mode !== "idle" || !cam.ready}
          className="h-16 w-full gap-2.5 text-lg md:h-16 [&_svg]:size-6">
          <Clock aria-hidden /> Clock in
        </Button>
        <Button onClick={() => clock("clock_out")} disabled={mode !== "idle" || !cam.ready}
          className="h-16 w-full gap-2.5 text-lg md:h-16 [&_svg]:size-6">
          <Clock aria-hidden /> Clock out
        </Button>
      </div>
      <p className="px-4 pb-4 text-center text-xs text-fg-subtle">Kiosk: {kioskName}. Photos are never stored.</p>
    </Screen>
  );
}

function Screen({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex min-h-screen flex-col items-center justify-center bg-canvas p-4">
      <div className="w-full max-w-md overflow-hidden rounded-lg border border-line bg-surface shadow-pop">{children}</div>
    </div>
  );
}

/** Registration happens on the tablet itself, signed in as HR; the device token is shown to nobody. */
function Register({ error, signedIn, onRegistered }: { error: string | null; signedIn: boolean; onRegistered: () => void }) {
  const [offices, setOffices] = useState<{ id: string; name: string }[]>([]);
  const [name, setName] = useState("");
  const [officeId, setOfficeId] = useState("");
  const [busy, setBusy] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);

  useEffect(() => {
    if (!signedIn) return;
    hrApi.listOfficeLocations().then((r) => {
      const list = r.data.items ?? r.data ?? [];
      setOffices(list);
      if (list[0]) setOfficeId(list[0].id);
    }).catch(() => setOffices([]));
  }, [signedIn]);

  async function register() {
    setBusy(true);
    setFormError(null);
    try {
      const res = await hrApi.registerKiosk({ name: name.trim(), office_location_id: officeId });
      localStorage.setItem(TOKEN_KEY, res.data.token);
      onRegistered();
    } catch (e) {
      setFormError(extractApiError(e, "Could not register this device."));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Screen>
      <div className="space-y-4 p-6 text-center">
        <div className="mx-auto flex size-14 items-center justify-center rounded-lg bg-surface-2">
          {signedIn ? <Tablet className="size-7 text-fg-muted" aria-hidden /> : <Lock className="size-7 text-fg-muted" aria-hidden />}
        </div>
        <h1 className="text-lg font-semibold text-fg">This device isn&apos;t registered as a kiosk</h1>
        {error && <p role="alert" className="text-sm text-tone-orange">{error}</p>}
        {!signedIn ? (
          <>
            <p className="text-sm text-fg-muted">An HR admin signs in on this tablet once to register it.</p>
            <Button variant="primary" size="lg" asChild>
              <a href="/login?callbackUrl=%2Fclock%3Fkiosk%3D1">Sign in as HR</a>
            </Button>
          </>
        ) : (
          <div className="space-y-3 text-left">
            <Field label="Tablet name">
              <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. Lobby tablet" autoComplete="off" />
            </Field>
            <Field label="Office">
              <Select value={officeId} onChange={(e) => setOfficeId(e.target.value)}
                options={offices.map((o) => ({ value: o.id, label: o.name }))} />
            </Field>
            {formError && <p role="alert" className="text-xs text-tone-red">{formError}</p>}
            <Button variant="primary" size="lg" className="w-full" onClick={register} loading={busy} disabled={!name.trim() || !officeId}>
              {!busy && <RefreshCw aria-hidden />} Register this device
            </Button>
            <p className="text-xs text-fg-subtle">After registering, sign out of your HR account on this tablet.</p>
          </div>
        )}
      </div>
    </Screen>
  );
}
