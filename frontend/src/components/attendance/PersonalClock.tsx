"use client";

import { useEffect, useRef, useState } from "react";
import { AlertTriangle, Camera, CheckCircle2, Loader2, MapPin, RefreshCw, ScanFace, XCircle } from "lucide-react";
import { accessToken, hrApi, type ActiveChallenge, type FaceChallenge } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { canvasToJpeg, useCamera } from "@/hooks/useCamera";
import { engineBackend, loadFaceEngine, warmUpEngine, type FaceApi } from "@/lib/faceEngine";
import { runChallenge, type ChallengeProgress } from "@/lib/faceChallenge";
import { attemptReport, fireAndForget, timeoutMessage } from "@/lib/faceAttempts";
import { ACTION_PROMPT, type LivenessAction } from "@/lib/faceLiveness";
import { FLASH_NOTICE, NOTICE_MS, releaseCapture, runFlash, type Rgb } from "@/lib/faceFlash";
import { Button } from "@/components/ui";
import StepRing from "@/components/attendance/StepRing";
import FlashOverlay from "@/components/attendance/FlashOverlay";
import { cn } from "@/lib/utils";

export interface ClockResult {
  status: "success" | "pending_override";
  message: string;
  time?: string;
  employee_name?: string;
  office_location?: string | null;
}

type Phase = "loading" | "ready" | "flash" | "challenge" | "sending" | "success" | "pending" | "error";

interface Geo { latitude: number; longitude: number; accuracy: number }

/**
 * Logged-in clock-in / clock-out on the employee's own phone or laptop. The device only guides and picks
 * key frames; the server re-detects, judges liveness and matches the face to this account.
 */
export default function PersonalClock({ eventType, onDone }: {
  eventType: "clock_in" | "clock_out";
  onDone?: (result: ClockResult) => void;
}) {
  const cam = useCamera(true);
  const [engine, setEngine] = useState<FaceApi | null>(null);
  const [engineError, setEngineError] = useState<string | null>(null);
  const [geo, setGeo] = useState<Geo | null>(null);
  const [geoError, setGeoError] = useState<string | null>(null);
  const [phase, setPhase] = useState<Phase>("loading");
  const [action, setAction] = useState<LivenessAction | null>(null);
  const [hint, setHint] = useState<string | undefined>();
  const [progress, setProgress] = useState<ChallengeProgress | null>(null);
  const [message, setMessage] = useState<string>("");
  const [flashColor, setFlashColor] = useState<Rgb | null>(null);
  const [stepNote, setStepNote] = useState<string | undefined>();
  const abort = useRef<AbortController | null>(null);

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const token = await accessToken();
        if (!token) throw new Error("unauthorized");
        const api = await loadFaceEngine(`${window.location.origin}/api/v1/hr/attendance/engine`, token, "guide");
        // Compile the WebGL shaders now, while the person reads the screen, not during the first step.
        await warmUpEngine(api);
        if (alive) setEngine(api);
      } catch (e) {
        if (alive) setEngineError((e as Error).message === "unauthorized"
          ? "Please sign in again to clock in." : "The face engine could not be loaded. Check your connection and retry.");
      }
    })();
    return () => { alive = false; abort.current?.abort(); };
  }, []);

  function locate() {
    setGeoError(null);
    if (!navigator.geolocation) { setGeoError("This browser cannot share its location."); return; }
    navigator.geolocation.getCurrentPosition(
      (pos) => setGeo({ latitude: pos.coords.latitude, longitude: pos.coords.longitude, accuracy: pos.coords.accuracy }),
      (err) => setGeoError(err.code === 1 ? "Location permission denied. Allow it in your browser settings."
        : err.code === 2 ? "Your location could not be determined." : "Location request timed out."),
      { enableHighAccuracy: true, timeout: 15000, maximumAge: 0 },
    );
  }
  useEffect(() => { locate(); }, []);

  useEffect(() => {
    if (phase === "loading" && engine && cam.ready) setPhase("ready");
  }, [phase, engine, cam.ready]);

  async function start() {
    if (!engine || !cam.videoRef.current || !geo) return;
    const video = cam.videoRef.current;
    setMessage("");
    setHint(undefined);
    setStepNote(undefined);
    let issued: FaceChallenge;
    try {
      issued = (await hrApi.faceChallenge(eventType)).data;
    } catch (e) {
      setMessage(extractApiError(e, "Could not start the check."));
      setPhase("error");
      return;
    }
    abort.current = new AbortController();
    const signal = abort.current.signal;
    const finish = (result: ClockResult, status: number) => {
      setMessage(result.message);
      setPhase(status === 202 || result.status === "pending_override" ? "pending" : "success");
      onDone?.(result);
    };
    const geoFields = (fd: FormData) => {
      fd.append("latitude", String(geo.latitude));
      fd.append("longitude", String(geo.longitude));
      fd.append("geo_accuracy_meters", String(geo.accuracy));
    };

    // Passive first (Release 2): flash the server's colours, the server judges the frames.
    let challenge: ActiveChallenge = issued;
    const passive = issued.passive;
    let afterFlash = false;
    if (passive?.enabled && passive.colors?.length) {
      setPhase("flash");
      afterFlash = true;
      await new Promise((r) => setTimeout(r, NOTICE_MS));
      if (signal.aborted) { setPhase("ready"); return; }
      const capture = await runFlash(video, passive.colors, passive.flash_ms, passive.settle_ms, setFlashColor, signal);
      setFlashColor(null);
      if (capture.aborted) { setPhase("ready"); return; }
      setPhase("sending");
      const fd = new FormData();
      fd.append("nonce", issued.nonce);
      fd.append("mode", "passive");
      try {
        fd.append("frame_neutral", await canvasToJpeg(capture.neutral), "frame_neutral.jpg");
        for (const [i, c] of capture.flashes.entries()) fd.append(`flash_${i}`, await canvasToJpeg(c), `flash_${i}.jpg`);
      } finally {
        // free the six capture canvases before the face engine runs again
        releaseCapture(capture);
      }
      geoFields(fd);
      try {
        const res = await hrApi.clockPassive(eventType, fd);
        const body = res.data;
        if (!body.active_required) return finish(body as unknown as ClockResult, res.status);
        challenge = body.challenge;
        setStepNote("One more step");
      } catch (e) {
        setMessage(extractApiError(e, "Clock-in failed. Please try again."));
        setPhase("error");
        return;
      }
    }

    setAction(challenge.action);
    setProgress(null);
    setPhase("challenge");
    const result = await runChallenge(engine, video, challenge.action, challenge.step_timeouts_ms,
      (p) => { setHint(p.hint); setProgress(p); }, signal, { afterFlash });
    const report = attemptReport(challenge.action, result, engineBackend(engine));
    fireAndForget(() => hrApi.reportFaceAttempt(report));
    const frames = result.frames;
    if (result.outcome === "aborted") { setPhase("ready"); return; }
    if (!frames) {
      setMessage(`The check timed out. ${timeoutMessage(result.step)}`);
      setPhase("error");
      return;
    }
    setPhase("sending");
    const fd = new FormData();
    fd.append("nonce", challenge.nonce);
    for (const [i, c] of frames.entries()) fd.append(`frame_${i}`, await canvasToJpeg(c), `frame_${i}.jpg`);
    geoFields(fd);
    try {
      const res = await hrApi.clockWithFrames(eventType, fd);
      finish(res.data as ClockResult, res.status);
    } catch (e) {
      setMessage(extractApiError(e, "Clock-in failed. Please try again."));
      setPhase("error");
    }
  }

  const label = eventType === "clock_in" ? "Clock in" : "Clock out";
  const busy = phase === "flash" || phase === "challenge" || phase === "sending";

  return (
    <div className="space-y-3">
      {/* The camera frame stays black in both themes; overlays sit on the video. */}
      <div className="relative aspect-[4/3] overflow-hidden rounded-lg bg-black">
        {cam.error ? (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 bg-surface-2 p-4 text-center">
            <Camera className="size-8 text-tone-red" aria-hidden />
            <p className="text-[13px] text-fg">{cam.error}</p>
            <Button size="sm" onClick={cam.retry}><RefreshCw aria-hidden /> Retry camera</Button>
          </div>
        ) : (
          <>
            <video ref={cam.videoRef} className="h-full w-full scale-x-[-1] object-cover" muted playsInline autoPlay />
            <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
              <div className={cn("h-44 w-36 rounded-full border-2",
                phase === "challenge" || phase === "flash" ? "border-tone-blue" : "border-white/40")} />
            </div>
            {phase === "flash" && (
              <div className="absolute inset-x-0 bottom-0 bg-black/70 px-4 py-3 text-center" aria-live="polite">
                <p className="text-base font-semibold text-white">Look at the screen and hold still</p>
                <p className="mt-0.5 text-[13px] text-white/80">{FLASH_NOTICE}</p>
              </div>
            )}
            <FlashOverlay color={flashColor} />
            {phase === "challenge" && action && (
              <>
                <div className="absolute right-3 top-3">
                  <StepRing budget={progress?.budget} budgetMs={progress?.budgetMs} remainingMs={progress?.remainingMs} />
                </div>
                <div className="absolute inset-x-0 bottom-0 bg-black/70 px-4 py-3 text-center" aria-live="polite">
                  {stepNote && <p className="text-[11px] font-medium uppercase tracking-wide text-white/70">{stepNote}</p>}
                  <p className="text-base font-semibold text-white">{ACTION_PROMPT[action]}</p>
                  {hint && <p className="mt-0.5 text-[13px] text-white/80">{hint}</p>}
                </div>
              </>
            )}
            {(phase === "loading" || phase === "sending") && (
              <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 bg-black/60" aria-live="polite">
                <Loader2 className="size-8 animate-spin text-white" aria-hidden />
                <p className="text-[13px] text-white">{phase === "loading" ? "Preparing the camera..." : "Verifying..."}</p>
              </div>
            )}
            {(phase === "success" || phase === "pending") && (
              <div role="status" className={cn("absolute inset-0 flex flex-col items-center justify-center gap-2 p-4 text-center",
                phase === "success" ? "bg-tone-blue-soft" : "bg-tone-orange-soft")}>
                {phase === "success"
                  ? <CheckCircle2 className="size-12 text-tone-blue" aria-hidden />
                  : <AlertTriangle className="size-12 text-tone-orange" aria-hidden />}
                <p className={cn("font-semibold", phase === "success" ? "text-tone-blue" : "text-sm text-tone-orange")}>{message}</p>
              </div>
            )}
          </>
        )}
      </div>

      <div className="flex items-center justify-between text-xs">
        <span className="flex items-center gap-1.5 text-fg-muted">
          <MapPin className={cn("size-3.5", geo ? "text-tone-blue" : geoError ? "text-tone-red" : "text-tone-orange")} aria-hidden />
          {geo ? <span className="tabular">Location ±{Math.round(geo.accuracy)} m</span> : geoError ?? "Getting your location..."}
        </span>
        {geoError && (
          <button type="button" onClick={locate} className="font-medium text-brand-ink hover:underline">Retry location</button>
        )}
      </div>

      {(engineError || (phase === "error" && message)) && (
        <div role="alert" className="flex items-start gap-1.5 text-xs text-tone-red">
          <XCircle className="mt-0.5 size-3.5 shrink-0" aria-hidden /> {engineError ?? message}
        </div>
      )}

      {phase !== "success" && phase !== "pending" && (
        <Button variant="primary" size="lg" className="w-full" onClick={start}
          disabled={busy || phase === "loading" || !geo || !!engineError || !!cam.error}>
          {busy ? <Loader2 className="animate-spin" aria-hidden /> : <ScanFace aria-hidden />}
          {phase === "error" ? `Try again: ${label}` : label}
        </Button>
      )}
      <p className="text-center text-[11px] text-fg-subtle">
        The screen may flash soft colours, and you may be asked to turn your head. Your photo is not stored.
      </p>
    </div>
  );
}
