"use client";

import { useEffect, useRef, useState } from "react";
import { FileText, Loader2, RefreshCw } from "lucide-react";
import { Button, EmptyState } from "@/components/ui";
import { extractApiError } from "@/lib/apiError";
import { docLibApi, fileKind } from "@/lib/documentLibrary";
import { isSafeImageType, isSafeVideoType, safeObjectUrl } from "@/lib/safeFile";

/**
 * The browser viewer: the staff-stamped copy from /view, PDF pages drawn with pdf.js (the same way
 * the verification page shows a PDF, so it works on phones too), images inline, videos in a player
 * (5 Nov 2026: a video is logged but never watermarked). Each view is logged as a staff download
 * with its own fingerprint, so it only loads when shown.
 */
export default function DocumentViewer({ documentId, versionId, fileName, mime, title }: {
  documentId: string;
  /** Omit for the current live version. */
  versionId?: string;
  fileName?: string;
  mime?: string;
  title: string;
}) {
  const pagesRef = useRef<HTMLDivElement>(null);
  const [state, setState] = useState<"loading" | "shown" | "failed">("loading");
  const [error, setError] = useState("");
  const [imageUrl, setImageUrl] = useState<string | null>(null);
  const [videoUrl, setVideoUrl] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const kind = fileKind(fileName, mime);

  useEffect(() => {
    let alive = true;
    let objectUrl: string | null = null;
    setState("loading");
    setError("");
    setImageUrl(null);
    setVideoUrl(null);
    (async () => {
      try {
        const res = await docLibApi.view(documentId, versionId);
        if (!alive) return;
        const blob = res.data;
        const isPdf = kind === "pdf" || blob.type === "application/pdf";
        const type = blob.type || mime;
        if (!isPdf && (kind === "video" || isSafeVideoType(type))) {
          if (!isSafeVideoType(type)) {
            setError("This video type can't be played in the browser. Download it instead.");
            setState("failed");
            return;
          }
          objectUrl = safeObjectUrl(blob, type);
          setVideoUrl(objectUrl);
          setState("shown");
          return;
        }
        if (!isPdf) {
          // Only plain raster images render inline (never SVG or HTML from the file's own type).
          if (!isSafeImageType(blob.type || mime)) {
            setError("This file type can't be shown in the browser. Download it instead.");
            setState("failed");
            return;
          }
          objectUrl = safeObjectUrl(blob, blob.type || mime);
          setImageUrl(objectUrl);
          setState("shown");
          return;
        }
        const buf = await blob.arrayBuffer();
        const pdfjsLib = await import("pdfjs-dist");
        pdfjsLib.GlobalWorkerOptions.workerSrc = "/pdf.worker.min.mjs";
        const doc = await pdfjsLib.getDocument({ data: new Uint8Array(buf) }).promise;
        const container = pagesRef.current;
        if (!alive || !container) return;
        container.innerHTML = "";
        for (let i = 1; i <= doc.numPages; i++) {
          const page = await doc.getPage(i);
          if (!alive) return;
          const viewport = page.getViewport({ scale: 1.6 });
          const canvas = document.createElement("canvas");
          canvas.width = viewport.width;
          canvas.height = viewport.height;
          canvas.className = "h-auto w-full rounded-lg border border-line shadow-card";
          canvas.setAttribute("aria-label", `Page ${i} of ${doc.numPages}`);
          canvas.setAttribute("role", "img");
          const ctx = canvas.getContext("2d");
          if (!ctx) continue;
          await page.render({ canvasContext: ctx, viewport }).promise;
          container.appendChild(canvas);
          // Show the first page as soon as it is drawn; the rest follow.
          if (i === 1 && alive) setState("shown");
        }
        if (alive) setState("shown");
      } catch (e) {
        if (!alive) return;
        setError(extractApiError(e, "The document could not be shown."));
        setState("failed");
      }
    })();
    return () => {
      alive = false;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [documentId, versionId, kind, mime, attempt]);

  return (
    <div className="relative min-h-[240px]" aria-busy={state === "loading"}>
      {state === "loading" && (
        <div role="status" className="flex items-center justify-center gap-2 py-16 text-sm text-fg-muted">
          <Loader2 className="size-4 animate-spin" aria-hidden /> Preparing your stamped copy
        </div>
      )}
      {state === "failed" && (
        <EmptyState icon={FileText} title="The document could not be shown" description={error}
          action={<Button onClick={() => setAttempt((a) => a + 1)}><RefreshCw aria-hidden /> Try again</Button>} />
      )}
      {videoUrl && state === "shown" && (
        <video src={videoUrl} controls preload="metadata" aria-label={title} data-testid="document-video"
          className="mx-auto max-h-[70vh] w-full rounded-lg border border-line bg-black" />
      )}
      {imageUrl && state === "shown" && (
        // eslint-disable-next-line @next/next/no-img-element
        <img src={imageUrl} alt={title} className="mx-auto h-auto max-w-full rounded-lg border border-line" />
      )}
      <div ref={pagesRef} className={state === "failed" || imageUrl || videoUrl ? "hidden" : "space-y-3"} aria-label={`Pages of ${title}`} />
    </div>
  );
}
