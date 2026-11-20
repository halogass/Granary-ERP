"use client";

import { useCallback, useEffect, useRef, useState } from "react";

/** Front camera for the clock-in screens: start / stop, with readable errors. */
export function useCamera(active = true) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const start = useCallback(async () => {
    if (streamRef.current) return;
    setError(null);
    const attach = (stream: MediaStream) => {
      streamRef.current = stream;
      if (videoRef.current) {
        videoRef.current.srcObject = stream;
        videoRef.current.play().catch(() => { /* autoplay policy: plays on the next user gesture */ });
      }
      setReady(true);
    };
    try {
      attach(await navigator.mediaDevices.getUserMedia({
        video: { facingMode: "user", width: { ideal: 640 }, height: { ideal: 480 } }, audio: false,
      }));
    } catch (err) {
      const name = (err as DOMException)?.name ?? "";
      if (name === "OverconstrainedError" || name === "ConstraintNotSatisfiedError") {
        try {
          attach(await navigator.mediaDevices.getUserMedia({ video: true, audio: false }));
          return;
        } catch { /* fall through */ }
      }
      setError(
        name === "NotAllowedError" || name === "PermissionDeniedError"
          ? "Camera permission denied. Allow the camera in your browser settings, then retry."
          : name === "NotReadableError" || name === "AbortError"
          ? "The camera is in use by another app or tab. Close it, then retry."
          : name === "NotFoundError" || name === "DevicesNotFoundError"
          ? "No camera found on this device."
          : `Camera unavailable (${name || "unknown error"}). Please retry.`,
      );
    }
  }, []);

  const stop = useCallback(() => {
    streamRef.current?.getTracks().forEach((t) => t.stop());
    streamRef.current = null;
    setReady(false);
  }, []);

  useEffect(() => {
    if (!active) return;
    start();
    return () => stop();
  }, [active, start, stop]);

  return { videoRef, ready, error, retry: () => { stop(); start(); }, stop };
}

/** Copy the current video frame (un-mirrored) into a canvas. */
export function grabFrame(video: HTMLVideoElement, into?: HTMLCanvasElement): HTMLCanvasElement {
  const canvas = into ?? document.createElement("canvas");
  canvas.width = video.videoWidth || 640;
  canvas.height = video.videoHeight || 480;
  canvas.getContext("2d")?.drawImage(video, 0, 0, canvas.width, canvas.height);
  return canvas;
}

/**
 * A canvas whose 2-D context is CPU-backed (willReadFrequently): no GPU surface, so capture canvases never compete
 * with the face engine's WebGL context for GPU memory on a low-end phone.
 */
export function cpuCanvas(): HTMLCanvasElement {
  const canvas = document.createElement("canvas");
  try {
    canvas.getContext("2d", { willReadFrequently: true });
  } catch {
    /* no 2-D context (tests): grabFrame copes */
  }
  return canvas;
}

/** The longest the video is given to deliver a fresh frame after being resumed. */
export const VIDEO_RESUME_MS = 2000;

/**
 * Make sure the camera video is playing and delivering frames before a check reads it: when it is paused or has no
 * current frame (Chrome may pause it while a full-screen overlay covers it), play it and wait for one new frame,
 * at most `maxMs`. Returns whether it had to be resumed.
 */
export async function ensureVideoPlaying(video: HTMLVideoElement, maxMs = VIDEO_RESUME_MS): Promise<boolean> {
  if (!video.paused && video.readyState >= 2) return false;
  try {
    await Promise.race([video.play(), new Promise((r) => setTimeout(r, maxMs))]);
  } catch {
    /* autoplay refused: the check still runs on whatever the video shows */
  }
  await new Promise<void>((resolve) => {
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      video.removeEventListener("timeupdate", finish);
      resolve();
    };
    const timer = setTimeout(finish, maxMs);
    const v = video as HTMLVideoElement & { requestVideoFrameCallback?: (cb: () => void) => number };
    if (typeof v.requestVideoFrameCallback === "function") v.requestVideoFrameCallback(finish);
    else video.addEventListener("timeupdate", finish);
  });
  return true;
}

export function canvasToJpeg(canvas: HTMLCanvasElement, quality = 0.9): Promise<Blob> {
  return new Promise((resolve, reject) =>
    canvas.toBlob((b) => (b ? resolve(b) : reject(new Error("capture failed"))), "image/jpeg", quality));
}
