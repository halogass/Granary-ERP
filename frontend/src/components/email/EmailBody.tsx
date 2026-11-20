"use client";

import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { ImageOff, Image as ImageIcon } from "lucide-react";
import { Button } from "@/components/ui";
import { emailImageSendersApi } from "@/lib/api";
import { normalizeSender, prepareEmailHtml } from "@/lib/emailFrame";
import { cn } from "@/lib/utils";

/*
 * A received email body (security triage 18, 27 Oct 2026).
 *
 * The body is rendered in an iframe with srcdoc and sandbox="allow-same-origin allow-popups
 * allow-popups-to-escape-sandbox":
 *  - no allow-scripts: nothing in the email can run (no <script>, no on* handlers, no javascript: links),
 *    on top of DOMPurify removing them;
 *  - allow-same-origin is safe because no script can run inside the frame to use that origin; it is there so
 *    this page can read the frame's document height and size the frame to fit (an opaque-origin frame could
 *    only be measured by a script inside it, which we do not allow);
 *  - the email's <style> tags style only the frame's own document, never the ERP;
 *  - links open in a new tab (<base target="_blank">, rel="noopener noreferrer"); allow-popups-to-escape-sandbox
 *    lets that tab be a normal page; no allow-top-navigation and no allow-forms.
 *
 * Remote images stay hidden until the user clicks "Show images" or "Always show images from <sender>"; the
 * second is kept per user on the server (GET/POST/DELETE /api/v1/email/image-senders), so it holds on every
 * device.
 */

// The logged-in user's always-show senders, loaded once and shared by every EmailBody on the page.
let senders: ReadonlySet<string> = new Set();
let loaded = false;
let loading: Promise<void> | null = null;
const listeners = new Set<() => void>();

function emit() {
  listeners.forEach((l) => l());
}
function subscribe(l: () => void) {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
}
function loadSenders() {
  if (loaded || loading) return;
  loading = emailImageSendersApi
    .list()
    .then((r) => {
      senders = new Set((r.data?.senders ?? []).map(normalizeSender));
    })
    .catch(() => {
      /* images just stay hidden until asked for */
    })
    .finally(() => {
      loaded = true;
      loading = null;
      emit();
    });
}

/** For tests: forget the loaded sender list. */
export function resetImageSenders() {
  senders = new Set();
  loaded = false;
  loading = null;
  emit();
}

function useImageSenders() {
  return useSyncExternalStore(subscribe, () => senders, () => senders);
}

export interface EmailBodyProps {
  html: string;
  /** The sender's address: offered in "Always show images from <sender>". */
  sender?: string | null;
  /** Mail we sent ourselves: its images show without asking. */
  trusted?: boolean;
  className?: string;
  /** Accessible name of the frame. */
  title?: string;
}

export default function EmailBody({ html, sender, trusted = false, className, title = "Email message" }: EmailBodyProps) {
  const from = normalizeSender(sender);
  const list = useImageSenders();
  const [showOnce, setShowOnce] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const remembered = !!from && list.has(from);
  const showImages = trusted || showOnce || remembered;

  const prepared = useMemo(() => prepareEmailHtml(html, showImages), [html, showImages]);
  const hasRemote = prepared.blocked > 0;

  // Learn the always-show senders only when an email actually holds remote images.
  useEffect(() => {
    if (hasRemote && !trusted && from) loadSenders();
  }, [hasRemote, trusted, from]);

  // A new email starts hidden again.
  useEffect(() => {
    setShowOnce(false);
    setError("");
  }, [html]);

  const frame = useRef<HTMLIFrameElement>(null);
  const [height, setHeight] = useState(80);

  const measure = useCallback(() => {
    const doc = frame.current?.contentDocument;
    if (!doc?.documentElement) return;
    const h = Math.max(doc.documentElement.scrollHeight, doc.body?.scrollHeight ?? 0);
    if (h > 0) setHeight(h);
  }, []);

  // The frame's document is same-origin (no script runs in it), so its height is read from here.
  const observer = useRef<ResizeObserver | null>(null);
  const onFrameLoad = useCallback(() => {
    measure();
    observer.current?.disconnect();
    const body = frame.current?.contentDocument?.body;
    if (body && typeof ResizeObserver !== "undefined") {
      observer.current = new ResizeObserver(measure);
      observer.current.observe(body);
    }
  }, [measure]);
  useEffect(() => () => observer.current?.disconnect(), []);

  async function alwaysShow() {
    if (!from) return;
    setSaving(true);
    setError("");
    try {
      await emailImageSendersApi.add(from);
      senders = new Set([...senders, from]);
      loaded = true;
      emit();
    } catch {
      setError("Could not save this choice. Try again.");
    } finally {
      setSaving(false);
    }
  }

  async function stopAlwaysShowing() {
    if (!from) return;
    setSaving(true);
    setError("");
    try {
      await emailImageSendersApi.remove(from);
      const next = new Set(senders);
      next.delete(from);
      senders = next;
      setShowOnce(false);
      emit();
    } catch {
      setError("Could not save this choice. Try again.");
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className={cn("min-w-0", className)} data-email-body>
      {hasRemote && !trusted && !showImages && (
        <div role="status" data-image-bar
          className="mb-2 flex flex-wrap items-center gap-2 rounded-lg border border-line bg-surface-2 px-3 py-2 text-xs text-fg-muted">
          <ImageOff className="size-3.5 shrink-0" aria-hidden />
          <span className="min-w-0 flex-1">Images are hidden to protect your privacy.</span>
          <Button size="sm" variant="secondary" onClick={() => setShowOnce(true)}>Show images</Button>
          {from && (
            <Button size="sm" variant="ghost" loading={saving} onClick={alwaysShow} className="max-w-full">
              <span className="truncate">Always show images from {from}</span>
            </Button>
          )}
        </div>
      )}
      {hasRemote && remembered && !trusted && (
        <div className="mb-2 flex flex-wrap items-center gap-2 text-xs text-fg-muted">
          <ImageIcon className="size-3.5 shrink-0" aria-hidden />
          <span className="min-w-0">Images from {from} are always shown.</span>
          <button type="button" onClick={stopAlwaysShowing} disabled={saving}
            className="underline transition-colors hover:text-fg disabled:opacity-50">
            Stop
          </button>
        </div>
      )}
      {error && <p role="alert" className="mb-2 text-xs text-tone-red">{error}</p>}
      <div className="overflow-hidden rounded-md bg-white">
        <iframe
          ref={frame}
          title={title}
          srcDoc={prepared.doc}
          sandbox="allow-same-origin allow-popups allow-popups-to-escape-sandbox"
          referrerPolicy="no-referrer"
          onLoad={onFrameLoad}
          className="block w-full border-0"
          style={{ height }}
        />
      </div>
    </div>
  );
}
