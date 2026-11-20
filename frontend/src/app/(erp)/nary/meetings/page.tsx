"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { naryApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { usePermissions } from "@/hooks/usePermissions";
import NaryTabs from "@/components/nary/NaryTabs";
import { Skeleton } from "@/components/ui";
import {
  FILE_WORKING, GoogleConnect, MeetingsList, type GoogleStatus, type MeetingFileRow, type MeetingRow,
} from "@/components/nary/Meetings";

export default function NaryMeetingsPage() {
  const { can } = usePermissions();
  const [google, setGoogle] = useState<GoogleStatus | null>(null);
  const [rows, setRows] = useState<MeetingRow[] | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [files, setFiles] = useState<MeetingFileRow[]>([]);
  const [uploading, setUploading] = useState<Record<string, number>>({});

  const loadFiles = useCallback(async () => {
    try { setFiles((await naryApi.meetingFiles()).data.items); } catch { /* the list stays as it was */ }
  }, []);
  const load = useCallback(async () => {
    try {
      const [g, m] = await Promise.all([naryApi.google(), naryApi.meetings()]);
      setGoogle(g.data);
      setRows(m.data.items);
    } catch (e) { setNotice(extractApiError(e)); }
    loadFiles();
  }, [loadFiles]);

  // While Nary transcribes or reads a file, its progress refreshes.
  const working = files.some((f) => FILE_WORKING.includes(f.status));
  useEffect(() => {
    if (!working) return;
    const t = setInterval(loadFiles, 15000);
    return () => clearInterval(t);
  }, [working, loadFiles]);
  const byMeeting = useMemo(() => {
    const out: Record<string, MeetingFileRow[]> = {};
    for (const f of files) if (f.meeting_id) (out[f.meeting_id] ??= []).push(f);
    return out;
  }, [files]);

  const uploadRecording = async (id: string, file: File) => {
    setNotice(null);
    setUploading((u) => ({ ...u, [id]: 0 }));
    try {
      await naryApi.meetingRecording(id, file, (p) => setUploading((u) => ({ ...u, [id]: p })));
      setNotice("Uploaded. Nary transcribes the recording on the server (a long one takes a while), then puts the results on the deal and tells you on WhatsApp.");
      await loadFiles();
    } catch (e) { setNotice(extractApiError(e)); }
    setUploading((u) => { const n = { ...u }; delete n[id]; return n; });
  };
  const downloadTranscript = async (f: MeetingFileRow) => {
    try {
      const r = await naryApi.meetingFileTranscript(f.id);
      const url = URL.createObjectURL(r.data as Blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `${f.filename.replace(/\.[^.]+$/, "")}-transcript.txt`;
      a.click();
      URL.revokeObjectURL(url);
    } catch (e) { setNotice(extractApiError(e)); }
  };
  useEffect(() => {
    const p = new URLSearchParams(window.location.search);
    if (p.get("google") === "connected") setNotice("Google Calendar is connected.");
    if (p.get("google") === "error") setNotice(`Google didn't connect: ${p.get("reason") || "unknown error"}`);
    load();
  }, [load]);

  const run = async (fn: () => Promise<unknown>, done?: string) => {
    setNotice(null);
    try { await fn(); if (done) setNotice(done); await load(); } catch (e) { setNotice(extractApiError(e)); }
  };

  return (
    <div className="max-w-5xl space-y-4">
      <NaryTabs subtitle="Calls with customers: the times Nary proposed for your approval, bookings with Google Meet, and what came out of each call." />
      {notice && <p role="status" className="rounded-lg border border-line bg-surface-2 px-3 py-2 text-sm text-fg">{notice}</p>}
      {google === null ? <Skeleton className="h-28 w-full" /> : (
        <GoogleConnect status={google} canManage={can("nary:manage")}
          onSave={(id, secret) => run(() => naryApi.googleCredentials({ client_id: id, client_secret: secret || undefined }), "Saved.")}
          onConnect={() => run(async () => { window.location.href = (await naryApi.googleConnect()).data.url; })} />
      )}
      {rows === null ? <Skeleton className="h-40 w-full" /> : (
        <MeetingsList rows={rows} canAct={can("nary:approve")}
          onDebrief={(id, f) => run(() => naryApi.meetingDebrief(id, f), "Nary read it: reminders set, the summary email is drafted for your approval.")}
          onNoShow={(id) => run(() => naryApi.meetingNoShow(id), "Marked as a no-show; Nary proposes new times.")}
          files={byMeeting} uploading={uploading} onRecording={uploadRecording} onDownload={downloadTranscript} />
      )}
    </div>
  );
}
