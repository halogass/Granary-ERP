"use client";

import { useRef, useState } from "react";
import { CalendarCheck2, CheckCircle2, Download, FileAudio, FileText, Link2, Loader2, Mic, TriangleAlert, Upload } from "lucide-react";
import { Button, Card, CardBody, CardHeader, EmptyState, Field, Input, StatusPill } from "@/components/ui";
import { safeHref } from "@/lib/sanitize";

/** Nary meetings (Phase 3): the Google Calendar connection and the meetings Nary proposed or booked. */

export interface GoogleStatus {
  account: string | null; status: string; last_error: string | null; credentials_set: boolean; redirect_uri: string;
}
export interface MeetingRow {
  id: string; status: string; title: string | null; chat_id: string | null; channel: string;
  deal_id: string | null; lead_id: string | null; starts_at: string | null; meet_link: string | null;
  attendee_email: string | null; customer_tz: string | null; picked: number | null;
  slots: { start: string; company?: string; customer?: string; extended?: boolean }[]; created_at: string;
}

/** A recording or notes file Nary transcribes / reads in the background (batch 3). */
export interface MeetingFileRow {
  id: string; kind: "notes" | "recording"; meeting_id: string | null; deal_id: string | null; filename: string;
  size_bytes: number; source: string; status: string; progress: number; error: string | null;
  duration_seconds: number | null; has_transcript: boolean; summary: string | null; created_at: string;
  finished_at: string | null;
}

const FILE_STATUS: Record<string, string> = {
  stored: "Waiting for its meeting", queued: "Waiting to be transcribed", transcribing: "Transcribing",
  analysing: "Nary is reading the transcript", done: "Done: results are on the deal", failed: "Failed",
};
export const FILE_WORKING = ["queued", "transcribing", "analysing"];

const STATUS: Record<string, string> = {
  proposed: "Slots waiting for your approval", slots_sent: "Times sent, waiting for the customer", booked: "Booked",
  done: "Done", no_show: "No-show", cancelled: "Cancelled",
};
const when = (iso: string | null) => (iso ? new Date(iso).toLocaleString() : "");

export function GoogleConnect({ status, canManage, onSave, onConnect }: {
  status: GoogleStatus; canManage: boolean;
  onSave: (clientId: string, clientSecret: string) => void; onConnect: () => void;
}) {
  const [clientId, setClientId] = useState("");
  const [secret, setSecret] = useState("");
  const connected = status.status === "connected";
  return (
    <Card>
      <CardHeader
        title={
          <span className="inline-flex items-center gap-2">
            {connected ? <CheckCircle2 className="size-4 text-tone-blue" aria-hidden /> : <TriangleAlert className="size-4 text-tone-orange" aria-hidden />}
            Google Calendar {connected ? `connected (${status.account})` :
              status.status === "needs_reconnect" ? "needs to be connected again" : "is not connected"}
          </span>
        } />
      <CardBody className="space-y-3">
        {status.last_error && !connected && <p className="text-xs text-tone-red">{status.last_error}</p>}
        <p className="text-xs text-fg-muted">
          Nary reads only when the calendar is busy, never event titles, and books the times you approved, with a Meet link.
          Redirect URI for the Google Cloud OAuth client: <code className="break-all font-mono text-fg">{status.redirect_uri}</code>
        </p>
        {canManage && (
          <div className="flex flex-wrap items-end gap-2">
            <Field label="OAuth client ID" className="min-w-[14rem] flex-1">
              <Input value={clientId} onChange={(e) => setClientId(e.target.value)} autoComplete="off"
                placeholder={status.credentials_set ? "saved (type to replace)" : "...apps.googleusercontent.com"} />
            </Field>
            <Field label="Client secret" className="min-w-[14rem] flex-1">
              <Input value={secret} onChange={(e) => setSecret(e.target.value)} type="password" autoComplete="off"
                placeholder={status.credentials_set ? "saved (type to replace)" : ""} />
            </Field>
            <Button disabled={!clientId.trim()} onClick={() => { onSave(clientId.trim(), secret.trim()); setSecret(""); }}>Save</Button>
            <Button variant="primary" disabled={!status.credentials_set} onClick={onConnect}>
              <Link2 aria-hidden /> {connected ? "Reconnect Google" : "Connect Google"}
            </Button>
          </div>
        )}
      </CardBody>
    </Card>
  );
}

function FileLine({ f, onDownload }: { f: MeetingFileRow; onDownload: (f: MeetingFileRow) => void }) {
  const Icon = f.kind === "recording" ? FileAudio : FileText;
  const working = FILE_WORKING.includes(f.status);
  return (
    <li className="flex flex-wrap items-center gap-2 text-xs text-fg-muted">
      <Icon className="size-3.5 shrink-0 text-fg-subtle" aria-hidden />
      <span className="max-w-[14rem] truncate text-fg" title={f.filename}>{f.filename}</span>
      <StatusPill family="meeting_file" status={f.status}
        label={`${FILE_STATUS[f.status] ?? f.status}${f.status === "transcribing" ? ` ${f.progress}%` : ""}`} />
      {f.status === "failed" && f.error && <span className="text-tone-red">{f.error}</span>}
      {working && <Loader2 className="size-3 animate-spin text-fg-subtle" aria-hidden />}
      {f.status === "transcribing" && (
        <span className="h-1.5 w-20 overflow-hidden rounded-full bg-surface-2" aria-hidden>
          <span className="block h-full bg-brand" style={{ width: `${f.progress}%` }} />
        </span>
      )}
      {f.has_transcript && (
        <Button size="sm" variant="ghost" className="ml-auto" onClick={() => onDownload(f)}>
          <Download aria-hidden /> Transcript
        </Button>
      )}
    </li>
  );
}

export function MeetingsList({ rows, canAct, onDebrief, onNoShow, files = {}, uploading = {}, onRecording, onDownload }: {
  rows: MeetingRow[]; canAct: boolean; onDebrief: (id: string, data: FormData) => void; onNoShow: (id: string) => void;
  /** Recordings and notes files per meeting id. */
  files?: Record<string, MeetingFileRow[]>;
  /** Upload progress (percent) of a recording per meeting id. */
  uploading?: Record<string, number>;
  onRecording?: (id: string, file: File) => void;
  onDownload?: (f: MeetingFileRow) => void;
}) {
  const fileRefs = useRef<Record<string, HTMLInputElement | null>>({});
  const recRefs = useRef<Record<string, HTMLInputElement | null>>({});
  const [notes, setNotes] = useState<Record<string, string>>({});
  if (rows.length === 0) return <EmptyState icon={CalendarCheck2} title="No meetings yet." description="Meetings Nary proposes or books show here." />;
  return (
    <ul className="overflow-hidden rounded-lg border border-line bg-surface">
      {rows.map((m) => (
        <li key={m.id} className="space-y-2 border-b border-line-soft px-4 py-3 last:border-0 even:bg-surface-2/60">
          <div className="flex flex-wrap items-start gap-3">
            <CalendarCheck2 className="mt-0.5 size-4 text-fg-subtle" aria-hidden />
            <div className="min-w-0 flex-1">
              <p className="flex flex-wrap items-center gap-2 text-sm font-medium text-fg">
                {m.title || "Meeting"}
                <StatusPill family="meeting" status={m.status} label={STATUS[m.status] ?? m.status} />
              </p>
              <p className="text-xs text-fg-muted tabular">
                {m.starts_at ? when(m.starts_at) : m.slots.map((s, i) => `${i + 1}. ${s.company ?? s.start}${s.extended ? " (extended)" : ""}`).join("  ")}
                {m.attendee_email ? ` · invite to ${m.attendee_email}` : ""}
              </p>
            </div>
            {safeHref(m.meet_link) && <a href={safeHref(m.meet_link)} target="_blank" rel="noreferrer" className="text-[13px] font-medium text-brand-ink hover:underline">Meet</a>}
            {m.deal_id && <a href={`/crm/deals/${m.deal_id}`} className="text-[13px] font-medium text-fg-muted hover:text-fg hover:underline">Deal</a>}
          </div>
          {canAct && ["booked", "done"].includes(m.status) && (
            <div className="flex flex-wrap items-center gap-2 pl-7">
              <Input value={notes[m.id] ?? ""} onChange={(e) => setNotes({ ...notes, [m.id]: e.target.value })} autoComplete="off"
                aria-label={`Debrief for ${m.title || "the meeting"}`} placeholder="How did it go?" className="min-w-[12rem] flex-1" />
              <Button size="sm" variant="primary" disabled={!(notes[m.id] ?? "").trim()}
                onClick={() => { const f = new FormData(); f.append("text", notes[m.id].trim()); onDebrief(m.id, f); setNotes({ ...notes, [m.id]: "" }); }}>
                Send debrief
              </Button>
              <input ref={(el) => { fileRefs.current[m.id] = el; }} type="file" className="hidden" aria-label={`Debrief file for ${m.id}`}
                accept="audio/*,.ogg,.opus,.mp3,.m4a,.wav,.txt,.srt,.vtt,.docx"
                onChange={(e) => { const file = e.target.files?.[0]; if (file) { const f = new FormData(); f.append("file", file); onDebrief(m.id, f); } e.target.value = ""; }} />
              <Button size="sm" onClick={() => fileRefs.current[m.id]?.click()}>
                <Mic aria-hidden /> Voice note or transcript
              </Button>
              {onRecording && (
                <>
                  <input ref={(el) => { recRefs.current[m.id] = el; }} type="file" className="hidden" aria-label={`Recording for ${m.id}`}
                    accept="audio/*,video/mp4,video/webm,.mp3,.m4a,.wav,.ogg,.opus,.webm,.mp4"
                    onChange={(e) => { const file = e.target.files?.[0]; if (file) onRecording(m.id, file); e.target.value = ""; }} />
                  <Button size="sm" disabled={uploading[m.id] !== undefined} onClick={() => recRefs.current[m.id]?.click()}
                    title="A meeting recording of any length up to the limit in Settings: Nary transcribes it on the server, then reads it">
                    {uploading[m.id] !== undefined
                      ? <><Loader2 className="animate-spin" aria-hidden /> Uploading <span className="tabular">{uploading[m.id]}%</span></>
                      : <><Upload aria-hidden /> Upload recording</>}
                  </Button>
                </>
              )}
              <Button size="sm" variant="ghost" onClick={() => onNoShow(m.id)}>They didn&apos;t show</Button>
            </div>
          )}
          {(files[m.id] ?? []).length > 0 && (
            <ul className="ml-7 space-y-1 border-t border-line-soft pt-2">
              {(files[m.id] ?? []).map((f) => <FileLine key={f.id} f={f} onDownload={(x) => onDownload?.(x)} />)}
            </ul>
          )}
        </li>
      ))}
    </ul>
  );
}
