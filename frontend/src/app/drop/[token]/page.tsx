"use client";

/**
 * 5 Nov 2026: the drop page of a one-time upload link (your AI client or Nary made it with create_upload_link). No login:
 * the token is the key. Drop files or pick them (several at once, works on a phone); each goes up as a raw PUT
 * with its own progress bar, one after another; Done closes the link.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { AlertTriangle, CheckCircle2, FileUp, Loader2, UploadCloud, XCircle } from "lucide-react";
import { Button, Card, CardBody } from "@/components/ui";
import { PublicPage, PublicNotice } from "@/components/portal/PublicPage";
import { cn } from "@/lib/utils";
import { type DropStatus, type UploadResult, humanSize, precheck, putFile, reasonOf, slotsLeft } from "./drop";

interface Item {
  id: number;
  file: File;
  status: "queued" | "uploading" | "done" | "failed";
  progress: number;
  error?: string;
  result?: UploadResult;
}

type LoadError = "not_found" | "rate_limited" | "failed";

const fmtTime = (iso: string) =>
  new Date(iso).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });

export default function DropPage({ params }: { params: { token: string } }) {
  const token = params.token;
  const [status, setStatus] = useState<DropStatus | null>(null);
  const [loadError, setLoadError] = useState<LoadError | null>(null);
  const [items, setItems] = useState<Item[]>([]);
  const [dragging, setDragging] = useState(false);
  const [closed, setClosed] = useState(false);
  const [closing, setClosing] = useState(false);
  const [closeError, setCloseError] = useState<string | null>(null);
  const busy = useRef(false);
  const nextId = useRef(1);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    (async () => {
      try {
        const res = await fetch(`/api/v1/public/drop/${encodeURIComponent(token)}`, { cache: "no-store" });
        if (res.ok) setStatus(await res.json());
        else setLoadError(res.status === 404 ? "not_found" : res.status === 429 ? "rate_limited" : "failed");
      } catch {
        setLoadError("failed");
      }
    })();
  }, [token]);

  const patch = (id: number, p: Partial<Item>) =>
    setItems((list) => list.map((it) => (it.id === id ? { ...it, ...p } : it)));

  /** Sends the queued files one at a time (a phone on a slow line does better with one stream). */
  const run = useCallback(async (queue: Item[]) => {
    if (busy.current) return;
    busy.current = true;
    for (const it of queue) {
      patch(it.id, { status: "uploading", progress: 0 });
      try {
        const result = await putFile(token, it.file, (f) => patch(it.id, { progress: f }));
        patch(it.id, { status: "done", progress: 1, result });
        setStatus((s) => (s ? { ...s, files_used: result.files_used } : s));
      } catch (e) {
        patch(it.id, { status: "failed", error: e instanceof Error ? e.message : String(e) });
      }
    }
    busy.current = false;
  }, [token]);

  function add(files: FileList | File[]) {
    if (!status) return;
    const pending = items.filter((i) => i.status === "queued" || i.status === "uploading").length;
    let room = slotsLeft(status, pending);
    const fresh: Item[] = [];
    for (const file of Array.from(files)) {
      const why = precheck(file, status);
      const id = nextId.current++;
      if (why) { fresh.push({ id, file, status: "failed", progress: 0, error: why }); continue; }
      if (room <= 0) {
        fresh.push({ id, file, status: "failed", progress: 0, error: `This link takes ${status.max_files} files; it is full.` });
        continue;
      }
      room -= 1;
      fresh.push({ id, file, status: "queued", progress: 0 });
    }
    setItems((list) => [...list, ...fresh]);
    const queue = fresh.filter((i) => i.status === "queued");
    if (queue.length) void run(queue);
  }

  async function done() {
    setClosing(true);
    try {
      const res = await fetch(`/api/v1/public/drop/${encodeURIComponent(token)}/close`, { method: "POST" });
      if (!res.ok) throw new Error(reasonOf(res.status, await res.text()));
      setClosed(true);
    } catch (e) {
      setCloseError(e instanceof Error ? e.message : String(e));
    } finally {
      setClosing(false);
    }
  }

  const shell = (children: React.ReactNode) => (
    <PublicPage subtitle="Granary file drop">
      <div className="space-y-4">{children}</div>
    </PublicPage>
  );

  if (loadError) {
    const copy = {
      not_found: { title: "Link not found", body: "This upload link does not exist. Ask your AI client or Nary for a new one." },
      rate_limited: { title: "Too many requests", body: "Wait a minute and open the link again." },
      failed: { title: "Could not open the link", body: "Try again in a moment." },
    }[loadError];
    return shell(
      <PublicNotice icon={AlertTriangle} tone={loadError === "not_found" ? "red" : "orange"} title={copy.title}>
        <p>{copy.body}</p>
      </PublicNotice>
    );
  }
  if (!status) return shell(
    <div role="status" aria-label="Loading" className="flex justify-center py-20">
      <Loader2 className="size-6 animate-spin text-fg-subtle" aria-hidden />
    </div>
  );

  const uploaded = items.filter((i) => i.status === "done");
  if (closed) return shell(
    <PublicNotice icon={CheckCircle2} tone="blue" title="All set">
      <p>{uploaded.length + status.files.length} file{uploaded.length + status.files.length === 1 ? "" : "s"} went to {status.target_label}. The link is closed; you can close this page.</p>
    </PublicNotice>
  );
  if (status.state !== "open" && uploaded.length === 0) return shell(
    <PublicNotice icon={XCircle} tone="orange" title="This link takes no more files">
      <p>{status.reason ?? "Ask your AI client or Nary for a new link."}</p>
    </PublicNotice>
  );

  const uploading = items.some((i) => i.status === "uploading" || i.status === "queued");
  const left = slotsLeft(status, items.filter((i) => i.status === "queued" || i.status === "uploading").length);

  return shell(
    <>
      <Card>
        <CardBody className="sm:p-6">
          <p className="text-xs font-medium text-fg-muted">Files go to</p>
          <h1 className="mt-1 break-words text-lg font-semibold text-fg">{status.target_label}</h1>
          <p className="mt-1 text-sm text-fg-muted">
            Works until {fmtTime(status.expires_at)}. {left} of {status.max_files} files left.
          </p>
        </CardBody>
      </Card>

      <label
        htmlFor="drop-input"
        onDragOver={(e) => { e.preventDefault(); setDragging(true); }}
        onDragLeave={() => setDragging(false)}
        onDrop={(e) => { e.preventDefault(); setDragging(false); add(e.dataTransfer.files); }}
        className={cn(
          "flex cursor-pointer flex-col items-center justify-center gap-2 rounded-lg border-2 border-dashed px-4 py-10 text-center transition-colors",
          dragging ? "border-brand bg-tone-blue-soft" : "border-line bg-surface hover:border-fg-subtle",
          left <= 0 && "pointer-events-none opacity-50",
        )}
      >
        <UploadCloud className="size-8 text-fg-subtle" aria-hidden />
        <span className="text-sm font-medium text-fg">Drop files here or tap to choose</span>
        <span className="max-w-sm text-xs text-fg-muted">
          {status.allowed}. Up to {status.file_max_label} a file, {status.video_max_label} a video.
        </span>
        <input
          id="drop-input"
          ref={inputRef}
          data-testid="drop-input"
          type="file"
          multiple
          accept={status.accept}
          className="sr-only"
          onChange={(e) => { if (e.target.files) add(e.target.files); e.target.value = ""; }}
        />
      </label>

      {(items.length > 0 || status.files.length > 0) && (
        <Card>
          <CardBody className="divide-y divide-line-soft p-0">
            {status.files.map((f, i) => (
              <div key={`old-${i}`} className="flex items-center gap-3 px-4 py-3">
                <CheckCircle2 className="size-4 shrink-0 text-tone-blue" aria-hidden />
                <span className="min-w-0 flex-1 truncate text-sm text-fg">{f.name}</span>
                <span className="text-xs text-fg-muted tabular">{f.size_label}</span>
              </div>
            ))}
            {items.map((it) => (
              <div key={it.id} className="px-4 py-3" data-testid="drop-item">
                <div className="flex items-center gap-3">
                  {it.status === "done" ? <CheckCircle2 className="size-4 shrink-0 text-tone-blue" aria-hidden />
                    : it.status === "failed" ? <XCircle className="size-4 shrink-0 text-tone-red" aria-hidden />
                    : <FileUp className="size-4 shrink-0 text-fg-subtle" aria-hidden />}
                  <span className="min-w-0 flex-1 truncate text-sm text-fg">{it.result?.name ?? it.file.name}</span>
                  <span className="text-xs text-fg-muted tabular">
                    {it.status === "uploading" ? `${Math.round(it.progress * 100)}%`
                      : it.status === "done" ? [it.result?.size_label, it.result?.duration_label].filter(Boolean).join(", ")
                      : humanSize(it.file.size)}
                  </span>
                </div>
                {it.status === "uploading" && (
                  <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-line" role="progressbar"
                    aria-valuenow={Math.round(it.progress * 100)} aria-valuemin={0} aria-valuemax={100}>
                    <div className="h-full bg-brand transition-[width]" style={{ width: `${it.progress * 100}%` }} />
                  </div>
                )}
                {it.error && <p className="mt-1 text-xs text-tone-red">{it.error}</p>}
              </div>
            ))}
          </CardBody>
        </Card>
      )}

      {closeError && <p className="text-right text-xs text-tone-red">{closeError}</p>}
      <div className="flex justify-end">
        <Button variant="primary" onClick={done} disabled={uploading || closing}>
          {closing ? "Closing..." : "Done"}
        </Button>
      </div>
    </>
  );
}
