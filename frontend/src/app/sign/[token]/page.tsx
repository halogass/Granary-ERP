"use client";

import { useState, useEffect, useRef } from "react";
import {
  CheckCircle2, XCircle, Pen, Type, AlertTriangle, Download, FileText, Loader2, Check, PenLine, CaseSensitive,
  TextCursorInput, CalendarDays, SquareCheck,
} from "lucide-react";
import SignaturePad from "@/components/sign/SignaturePad";
import {
  Button, Card, CardBody, CardHeader, Checkbox, Dialog, Field as UiField, Input, Pill, Select, Textarea, TONE_CLASSES, type Tone,
} from "@/components/ui";
import { PublicPage, PublicNotice } from "@/components/portal/PublicPage";
import { FormError } from "@/components/portal/AuthLayout";
import { cn } from "@/lib/utils";

const SIGN_REASONS = ["Approved by", "Authored by", "Authorized by", "Reviewed by"];

interface Field {
  id: string; field_type: string; page_number: number;
  x: number | null; y: number | null; width: number | null; height: number | null;
  is_required: boolean; label: string | null; value: string | null;
}
interface DocInfo {
  document_id: string; document_title: string; document_message: string | null;
  document_status: string; signer_name: string; signer_email: string;
  signer_status: string; signer_role: string;
  pdf_url: string; fields: Field[];
  already_signed: boolean; already_declined: boolean;
  saved_signature?: string | null;
  disclosure_text?: string; disclosure_version?: string;
  signature_level?: string; signature_level_label?: string;
}

type DrawMode = "draw" | "type" | "saved";

// Signature fields "need you" (brand red); the rest are told apart by tone and icon.
const FIELD_TYPE_META: Record<string, { icon: React.ElementType; label: string; tone: Tone }> = {
  signature: { icon: PenLine,         label: "Sign here",  tone: "red" },
  initials:  { icon: CaseSensitive,   label: "Initials",   tone: "violet" },
  text:      { icon: TextCursorInput, label: "Enter text", tone: "orange" },
  date:      { icon: CalendarDays,    label: "Date",       tone: "blue" },
  checkbox:  { icon: SquareCheck,     label: "Check",      tone: "neutral" },
};

const FIELD_BOX: Record<Tone, string> = {
  red: "border-tone-red bg-tone-red/10 text-tone-red",
  violet: "border-tone-violet bg-tone-violet/10 text-tone-violet",
  orange: "border-tone-orange bg-tone-orange/10 text-tone-orange",
  blue: "border-tone-blue bg-tone-blue/10 text-tone-blue",
  neutral: "border-tone-neutral bg-tone-neutral/10 text-tone-neutral",
};

export default function SignPage({ params }: { params: { token: string } }) {
  const [info, setInfo] = useState<DocInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [consent, setConsent] = useState(false);
  const [intent, setIntent] = useState(false);
  const [showDisclosure, setShowDisclosure] = useState(false);
  const [drawMode, setDrawMode] = useState<DrawMode>("draw");
  const [typedName, setTypedName] = useState("");
  const [initialsData, setInitialsData] = useState<string | null>(null);
  const [signReason, setSignReason] = useState("Authorized by");
  const [fieldValues, setFieldValues] = useState<Record<string, string>>({});
  const [submitting, setSubmitting] = useState(false);
  const [done, setDone] = useState(false);
  const [documentCompleted, setDocumentCompleted] = useState(false);
  const [declined, setDeclined] = useState(false);
  const [declineReason, setDeclineReason] = useState("");
  const [showDecline, setShowDecline] = useState(false);
  const [declining, setDeclining] = useState(false);

  // PDF rendering state
  const [numPages, setNumPages] = useState(0);
  const [pdfLoaded, setPdfLoaded] = useState(false);
  const [pdfError, setPdfError] = useState(false);
  const canvasRefs = useRef<Map<number, HTMLCanvasElement>>(new Map());

  // Signature canvas
  const sigCanvasRef = useRef<HTMLCanvasElement>(null);
  const isDrawing = useRef(false);
  const [hasDrawnSig, setHasDrawnSig] = useState(false);

  // Load document info
  useEffect(() => {
    fetch(`/api/v1/sign/public/${params.token}`)
      .then(r => { if (!r.ok) throw new Error("not_found"); return r.json(); })
      .then(data => {
        setInfo(data);
        if (data.saved_signature) setDrawMode("saved");   // default to the user's saved signature
        const today = new Date().toLocaleDateString("en-GB", { day: "2-digit", month: "long", year: "numeric" });
        const vals: Record<string, string> = {};
        (data.fields as Field[]).forEach(f => { if (f.field_type === "date") vals[f.id] = today; });
        setFieldValues(vals);
      })
      .catch(() => setError("This signing link is invalid or has expired."))
      .finally(() => setLoading(false));
  }, [params.token]);

  // Load and render PDF with PDF.js
  useEffect(() => {
    if (!info?.pdf_url) return;
    let cancelled = false;

    async function loadPdf() {
      try {
        const pdfjsLib = await import("pdfjs-dist");
        pdfjsLib.GlobalWorkerOptions.workerSrc = "/pdf.worker.min.mjs";
        const pdf = await pdfjsLib.getDocument({ url: info!.pdf_url }).promise;
        if (cancelled) return;
        setNumPages(pdf.numPages);

        for (let pageNum = 1; pageNum <= pdf.numPages; pageNum++) {
          const page = await pdf.getPage(pageNum);
          if (cancelled) return;
          const viewport = page.getViewport({ scale: 1.5 });
          const canvas = canvasRefs.current.get(pageNum);
          if (!canvas) continue;
          canvas.width = viewport.width;
          canvas.height = viewport.height;
          const ctx = canvas.getContext("2d");
          if (!ctx) continue;
          await page.render({ canvasContext: ctx, viewport }).promise;
        }
        if (!cancelled) setPdfLoaded(true);
      } catch {
        if (!cancelled) setPdfError(true);
      }
    }

    loadPdf();
    return () => { cancelled = true; };
  }, [info?.pdf_url]);

  // ─── Signature canvas drawing ───

  function startDraw(e: React.MouseEvent<HTMLCanvasElement>) {
    isDrawing.current = true;
    const canvas = sigCanvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const rect = canvas.getBoundingClientRect();
    ctx.beginPath();
    ctx.moveTo(e.clientX - rect.left, e.clientY - rect.top);
  }
  function draw(e: React.MouseEvent<HTMLCanvasElement>) {
    if (!isDrawing.current) return;
    const canvas = sigCanvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const rect = canvas.getBoundingClientRect();
    ctx.lineTo(e.clientX - rect.left, e.clientY - rect.top);
    ctx.strokeStyle = "#111";
    ctx.lineWidth = 2;
    ctx.lineCap = "round";
    ctx.stroke();
    setHasDrawnSig(true);
  }
  function stopDraw() { isDrawing.current = false; }

  function getTouchPos(e: React.TouchEvent<HTMLCanvasElement>) {
    const canvas = sigCanvasRef.current!;
    const rect = canvas.getBoundingClientRect();
    return { x: e.touches[0].clientX - rect.left, y: e.touches[0].clientY - rect.top };
  }
  function startDrawTouch(e: React.TouchEvent<HTMLCanvasElement>) {
    e.preventDefault();
    isDrawing.current = true;
    const { x, y } = getTouchPos(e);
    const ctx = sigCanvasRef.current?.getContext("2d");
    if (!ctx) return;
    ctx.beginPath();
    ctx.moveTo(x, y);
  }
  function drawTouch(e: React.TouchEvent<HTMLCanvasElement>) {
    e.preventDefault();
    if (!isDrawing.current) return;
    const { x, y } = getTouchPos(e);
    const ctx = sigCanvasRef.current?.getContext("2d");
    if (!ctx) return;
    ctx.lineTo(x, y);
    ctx.strokeStyle = "#111";
    ctx.lineWidth = 2;
    ctx.lineCap = "round";
    ctx.stroke();
    setHasDrawnSig(true);
  }

  function clearCanvas() {
    const canvas = sigCanvasRef.current;
    if (!canvas) return;
    canvas.getContext("2d")?.clearRect(0, 0, canvas.width, canvas.height);
    setHasDrawnSig(false);
  }

  function getSignatureData(): string | null {
    if (drawMode === "saved") {
      return info?.saved_signature ?? null;
    }
    if (drawMode === "type") {
      if (!typedName.trim()) return null;
      const offscreen = document.createElement("canvas");
      offscreen.width = 400; offscreen.height = 100;
      const ctx = offscreen.getContext("2d")!;
      ctx.fillStyle = "white";
      ctx.fillRect(0, 0, 400, 100);
      ctx.font = "italic 42px Georgia, serif";
      ctx.fillStyle = "#111";
      ctx.fillText(typedName.trim(), 20, 70);
      return offscreen.toDataURL("image/png");
    }
    const canvas = sigCanvasRef.current;
    if (!canvas) return null;
    const data = canvas.getContext("2d")!.getImageData(0, 0, canvas.width, canvas.height).data;
    const hasContent = Array.from(data).some((v, i) => i % 4 !== 3 && v !== 0);
    return hasContent ? canvas.toDataURL("image/png") : null;
  }

  function isFieldFilled(f: Field): boolean {
    if (f.field_type === "initials") return !!initialsData;
    if (f.field_type === "signature") {
      if (drawMode === "saved") return !!info?.saved_signature;
      return drawMode === "type" ? !!typedName.trim() : hasDrawnSig;
    }
    return !!fieldValues[f.id];
  }

  function fieldClass(f: Field): string {
    if (isFieldFilled(f)) return FIELD_BOX.blue;
    return FIELD_BOX[(FIELD_TYPE_META[f.field_type] ?? FIELD_TYPE_META.text).tone];
  }

  async function submit() {
    if (!info || !consent || !intent) return;
    const sigData = getSignatureData();
    const needsSig = (info.fields || []).some(f => f.field_type === "signature");
    const needsInitials = (info.fields || []).some(f => f.field_type === "initials");
    if (needsSig && !sigData) {
      setError("Please provide your signature before submitting.");
      return;
    }
    if (needsInitials && !initialsData) {
      setError("Please provide your initials before submitting.");
      return;
    }
    setSubmitting(true);
    setError("");
    try {
      const res = await fetch(`/api/v1/sign/public/${params.token}/sign`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          signature_data: sigData,
          initials_data: initialsData,
          sign_reason: signReason,
          consent: true,
          field_values: Object.entries(fieldValues).map(([field_id, value]) => ({ field_id, value })),
        }),
      });
      if (!res.ok) {
        const d = await res.json();
        setError(d.detail ?? "Submission failed.");
        return;
      }
      const result = await res.json();
      setDocumentCompleted(result.document_completed ?? false);
      setDone(true);
    } catch { setError("Network error. Please try again."); }
    finally { setSubmitting(false); }
  }

  async function decline() {
    if (!declineReason.trim()) return;
    setDeclining(true);
    try {
      const res = await fetch(`/api/v1/sign/public/${params.token}/decline?reason=${encodeURIComponent(declineReason)}`, { method: "POST" });
      if (!res.ok) throw new Error();
      setDeclined(true);
      setShowDecline(false);
    } catch { setError("Failed to decline."); }
    finally { setDeclining(false); }
  }

  // ─── Loading / error states ───

  if (loading) return (
    <PublicPage subtitle="Document signing">
      <div role="status" aria-label="Loading document" className="flex justify-center py-20"><Loader2 className="size-6 animate-spin text-fg-subtle" aria-hidden /></div>
    </PublicPage>
  );

  if (error && !info) return (
    <PublicPage subtitle="Document signing" width="sm" center>
      <PublicNotice icon={AlertTriangle} tone="orange" title="Link unavailable"><p>{error}</p></PublicNotice>
    </PublicPage>
  );

  if (done) return (
    <PublicPage subtitle="Document signing" width="sm" center>
      <PublicNotice icon={CheckCircle2} tone="blue" title="Document signed"
        action={
          <div className="flex w-full flex-col gap-2">
            {documentCompleted && (
              <Button variant="primary" size="lg" className="w-full" asChild>
                <a href={`/api/v1/sign/public/${params.token}/certificate`} target="_blank" rel="noopener noreferrer">
                  <FileText aria-hidden /> Download signing certificate
                </a>
              </Button>
            )}
            <Button size="lg" className="w-full" asChild>
              <a href={`/api/v1/sign/public/${params.token}/pdf`} target="_blank" rel="noopener noreferrer">
                <Download aria-hidden /> Download original document
              </a>
            </Button>
          </div>
        }>
        <p>You have signed <strong className="text-fg">{info?.document_title}</strong>.</p>
        {documentCompleted ? (
          <p className="mt-1 text-xs font-medium text-tone-blue">All parties have signed. The document is complete.</p>
        ) : (
          <p className="mt-1 text-xs">Waiting for the other signers.</p>
        )}
        <p className="mt-4 text-xs text-fg-subtle">Legally binding under the US ESIGN Act and the EU eIDAS Regulation.</p>
      </PublicNotice>
    </PublicPage>
  );

  if (declined) return (
    <PublicPage subtitle="Document signing" width="sm" center>
      <PublicNotice icon={XCircle} tone="red" title="Signing declined">
        <p>You have declined to sign this document. The sender has been notified.</p>
      </PublicNotice>
    </PublicPage>
  );

  if (!info) return null;

  if (info.already_signed) return (
    <PublicPage subtitle="Document signing" width="sm" center>
      <PublicNotice icon={CheckCircle2} tone="blue" title="Already signed"
        action={
          <Button size="lg" asChild>
            <a href={`/api/v1/sign/public/${params.token}/pdf`} target="_blank" rel="noopener noreferrer">
              <Download aria-hidden /> Download original document
            </a>
          </Button>
        }>
        <p>You have already signed <strong className="text-fg">{info.document_title}</strong>.</p>
      </PublicNotice>
    </PublicPage>
  );

  const hasSigField = (info.fields || []).some(f => f.field_type === "signature");
  const hasInitialsField = (info.fields || []).some(f => f.field_type === "initials");
  const textFields = (info.fields || []).filter(f => f.field_type === "text" || f.field_type === "checkbox");
  const modeBtn = (on: boolean) => cn("flex min-h-11 flex-1 items-center justify-center gap-1 rounded-lg text-xs font-medium transition-colors md:min-h-8",
    on ? "bg-brand text-brand-on" : "bg-surface-2 text-fg-muted hover:text-fg");

  return (
    <PublicPage subtitle="Document signing" width="xl" className="py-6"
      aside={
        <div className="hidden min-w-0 text-right sm:block">
          <p className="truncate text-sm font-medium text-fg">{info.signer_name}</p>
          <p className="truncate text-xs text-fg-muted">{info.signer_email}</p>
        </div>
      }>
      <div className="flex flex-col gap-5 lg:flex-row">

        {/* ─── Left: PDF with field overlays ─── */}
        <div className="min-w-0 flex-1">
          <Card className="overflow-hidden">
            <div className="flex flex-wrap items-center justify-between gap-2 border-b border-line bg-surface-2 px-4 py-2.5">
              <span className="truncate text-[13px] font-medium text-fg">{info.document_title}</span>
              {info.fields && info.fields.length > 0 && (
                <div className="flex flex-wrap items-center gap-1.5">
                  {Object.entries(FIELD_TYPE_META)
                    .filter(([type]) => (info.fields || []).some(f => f.field_type === type))
                    .map(([type, meta]) => (
                      <Pill key={type} tone={meta.tone} icon={meta.icon}>
                        <span className="tabular">{(info.fields || []).filter(f => f.field_type === type).length}</span> {type}
                      </Pill>
                    ))}
                </div>
              )}
            </div>

            <div className="space-y-4 overflow-y-auto bg-surface-2 p-4" style={{ maxHeight: "78vh" }}>
              {pdfError && (
                <div role="alert" className="rounded-lg border border-tone-orange/30 bg-tone-orange-soft p-4 text-center text-sm text-tone-orange">
                  Failed to load the PDF. Please check your connection and try again.
                </div>
              )}

              {!pdfLoaded && !pdfError && (
                <div role="status" className="flex h-64 flex-col items-center justify-center gap-3">
                  <Loader2 className="size-5 animate-spin text-fg-subtle" aria-hidden />
                  <p className="text-xs text-fg-muted">Loading document...</p>
                </div>
              )}

              {Array.from({ length: numPages }, (_, i) => i + 1).map(pageNum => (
                // The page is paper: light tokens inside, in both themes.
                <div key={pageNum} className="light relative mx-auto overflow-hidden rounded shadow-pop"
                  style={{ width: "fit-content" }}>
                  <canvas
                    ref={(el) => { if (el) canvasRefs.current.set(pageNum, el); }}
                    className="block"
                  />

                  {pdfLoaded && (
                    <div className="pointer-events-none absolute inset-0" style={{ zIndex: 10 }}>
                      {(info.fields || []).filter(f => f.page_number === pageNum && f.x !== null && f.y !== null).map(field => {
                        const meta = FIELD_TYPE_META[field.field_type] ?? FIELD_TYPE_META.text;
                        const filled = isFieldFilled(field);
                        const Icon = filled ? Check : meta.icon;
                        const isPulse = !filled && field.is_required;
                        return (
                          <div
                            key={field.id}
                            className={cn("absolute flex items-center justify-center rounded border-2", fieldClass(field), isPulse && "animate-pulse")}
                            style={{
                              left: `${field.x}%`,
                              top: `${field.y}%`,
                              width: `${field.width}%`,
                              height: `${field.height}%`,
                            }}
                          >
                            <div className="flex items-center gap-1 overflow-hidden px-1">
                              <Icon className="size-3 shrink-0" aria-hidden />
                              <span className="hidden truncate text-[9px] font-semibold sm:block">
                                {filled ? "Done" : (field.label || meta.label)}
                              </span>
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  )}

                  <div className="pointer-events-none absolute bottom-1 right-2 z-20 text-[10px] text-fg-subtle">
                    Page {pageNum}
                  </div>
                </div>
              ))}
            </div>
          </Card>
        </div>

        {/* ─── Right: Signing panel ─── */}
        <div className="flex w-full shrink-0 flex-col gap-4 lg:w-[340px]">
          <Card className="p-4">
            <h1 className="text-base font-semibold text-fg">{info.document_title}</h1>
            {info.document_message && <p className="mt-1 whitespace-pre-wrap text-sm text-fg-muted">{info.document_message}</p>}
            <p className="mt-2 text-xs text-fg-muted">Signing as <strong className="font-medium text-fg">{info.signer_name}</strong>
              <span className="sm:hidden"> ({info.signer_email})</span></p>
          </Card>

          {info.fields && info.fields.length > 0 && (
            <Card>
              <CardHeader title="Fields to complete" />
              <CardBody className="space-y-2 py-3">
                {info.fields.map(f => {
                  const meta = FIELD_TYPE_META[f.field_type] ?? FIELD_TYPE_META.text;
                  const filled = isFieldFilled(f);
                  const Icon = filled ? Check : meta.icon;
                  return (
                    <div key={f.id} className="flex items-center gap-2">
                      <span className={cn("flex size-5 shrink-0 items-center justify-center rounded-full transition-colors",
                        filled ? "bg-tone-blue text-surface" : TONE_CLASSES[meta.tone])}>
                        <Icon className="size-3" aria-hidden />
                      </span>
                      <span className="text-xs text-fg">
                        {f.label || meta.label}
                        {f.page_number > 1 && <span className="ml-1 text-fg-muted">· p.{f.page_number}</span>}
                        {filled && <span className="sr-only"> (done)</span>}
                      </span>
                      {f.is_required && !filled && <span className="ml-auto text-[11px] font-medium text-brand-ink">Required</span>}
                    </div>
                  );
                })}
              </CardBody>
            </Card>
          )}

          {textFields.length > 0 && (
            <Card>
              <CardHeader title="Fill in fields" />
              <CardBody className="space-y-3">
                {textFields.map(f => f.field_type === "checkbox" ? (
                  <Checkbox key={f.id} label={f.label ?? "Check to confirm"} checked={!!fieldValues[f.id]}
                    onChange={e => setFieldValues({ ...fieldValues, [f.id]: e.target.checked ? "true" : "false" })} />
                ) : (
                  <UiField key={f.id} label={f.label ?? f.field_type}>
                    <Input value={fieldValues[f.id] ?? ""} onChange={e => setFieldValues({ ...fieldValues, [f.id]: e.target.value })} />
                  </UiField>
                ))}
              </CardBody>
            </Card>
          )}

          {hasSigField && (
            <Card>
              <CardHeader title="Your signature" />
              <CardBody className="space-y-3">
                <UiField label="Reason">
                  <Select value={signReason} onChange={e => setSignReason(e.target.value)} options={SIGN_REASONS.map(r => ({ value: r, label: r }))} />
                </UiField>
                <div className="flex gap-2" role="group" aria-label="How to sign">
                  {info.saved_signature && (
                    <button type="button" aria-pressed={drawMode === "saved"} onClick={() => setDrawMode("saved")} className={modeBtn(drawMode === "saved")}>
                      <CheckCircle2 className="size-3.5" aria-hidden /> Saved
                    </button>
                  )}
                  <button type="button" aria-pressed={drawMode === "draw"} onClick={() => { setDrawMode("draw"); setHasDrawnSig(false); clearCanvas(); }} className={modeBtn(drawMode === "draw")}>
                    <Pen className="size-3.5" aria-hidden /> Draw
                  </button>
                  <button type="button" aria-pressed={drawMode === "type"} onClick={() => setDrawMode("type")} className={modeBtn(drawMode === "type")}>
                    <Type className="size-3.5" aria-hidden /> Type
                  </button>
                </div>
                {drawMode === "saved" ? (
                  // eslint-disable-next-line @next/next/no-img-element
                  <img src={info.saved_signature!} alt="Your saved signature"
                    className="h-[100px] w-full rounded-lg border-2 border-tone-blue object-contain"
                    style={{ backgroundColor: "white" }} />
                ) : drawMode === "draw" ? (
                  <div>
                    {/* Paper stays white in both themes: the ink is dark and the image lands on a white PDF. */}
                    <canvas ref={sigCanvasRef} width={280} height={100} aria-label="Draw your signature"
                      onMouseDown={startDraw} onMouseMove={draw} onMouseUp={stopDraw} onMouseLeave={stopDraw}
                      onTouchStart={startDrawTouch} onTouchMove={drawTouch} onTouchEnd={stopDraw}
                      className={cn("w-full cursor-crosshair rounded-lg border-2 border-dashed transition-colors",
                        hasDrawnSig ? "border-tone-blue" : "border-line-strong")}
                      style={{ touchAction: "none", backgroundColor: "white" }} />
                    <Button size="sm" variant="ghost" className="mt-1.5" onClick={clearCanvas}>Clear</Button>
                  </div>
                ) : (
                  <input value={typedName} onChange={e => setTypedName(e.target.value)} aria-label="Type your name"
                    className="w-full rounded-lg border-2 border-dashed border-line-strong px-4 py-4 text-center focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/20"
                    style={{ fontFamily: "Georgia, serif", fontSize: "24px", fontStyle: "italic", color: "#111", backgroundColor: "white" }}
                    placeholder="Type your name" />
                )}
              </CardBody>
            </Card>
          )}

          {hasInitialsField && (
            <Card>
              <CardHeader title="Your initials" />
              <CardBody><SignaturePad value={initialsData} onChange={setInitialsData} height={90} /></CardBody>
            </Card>
          )}

          <Card>
            <CardBody className="space-y-4">
              <div className="rounded-lg border border-line bg-surface-2 p-3.5">
                <div className="flex items-center justify-between gap-2">
                  <p className="text-xs font-semibold text-fg">Electronic records and signature disclosure</p>
                  <button type="button" onClick={() => setShowDisclosure(s => !s)} aria-expanded={showDisclosure}
                    className="shrink-0 text-xs font-medium text-brand-ink hover:underline">
                    {showDisclosure ? "Hide" : "Read disclosure"}
                  </button>
                </div>
                {showDisclosure && (
                  <pre className="mt-3 max-h-56 overflow-auto whitespace-pre-wrap font-sans text-[11px] leading-relaxed text-fg-muted">
{info?.disclosure_text || "By signing electronically you agree to use electronic records and signatures, which are legally binding."}
                  </pre>
                )}
                <p className="mt-2 text-[11px] text-fg-muted">
                  Valid under US ESIGN Act &amp; UETA and EU eIDAS (Reg. 910/2014).
                  The completed document is sealed with a tamper-evident digital signature{info?.signature_level_label ? ` (${info.signature_level_label})` : ""}.
                </p>
              </div>

              <Checkbox checked={consent} onChange={e => setConsent(e.target.checked)} className="text-xs leading-relaxed"
                label={<>I consent to use <strong className="font-semibold">electronic records and electronic signatures</strong> for this transaction, and I have read the disclosure above.</>} />
              <Checkbox checked={intent} onChange={e => setIntent(e.target.checked)} className="text-xs leading-relaxed"
                label={<>I <strong className="font-semibold">intend to sign</strong> this document and I am authorised to do so. My electronic signature is legally binding.</>} />

              {error && <FormError>{error}</FormError>}

              <Button variant="primary" size="lg" className="w-full" onClick={submit} loading={submitting} disabled={!consent || !intent}>
                {submitting ? "Signing..." : "Sign document"}
              </Button>

              <Button variant="ghost" size="sm" className="w-full text-fg-muted hover:text-tone-red" onClick={() => setShowDecline(true)}>
                Decline to sign
              </Button>
            </CardBody>
          </Card>
        </div>
      </div>

      <Dialog open={showDecline} onOpenChange={setShowDecline} title="Decline to sign" size="sm" dismissable={!declining}
        description="Please give a reason for declining. The sender will be notified."
        footer={<>
          <Button onClick={() => setShowDecline(false)}>Cancel</Button>
          <Button variant="danger" onClick={decline} loading={declining} disabled={!declineReason.trim()}>
            {declining ? "Declining..." : "Confirm decline"}
          </Button>
        </>}>
        <UiField label="Reason">
          <Textarea value={declineReason} onChange={e => setDeclineReason(e.target.value)} rows={3} placeholder="Reason for declining..." />
        </UiField>
      </Dialog>
    </PublicPage>
  );
}
