"use client";

import { useEffect, useRef, useState, useCallback } from "react";
import { ImageIcon, PenLine, Type, X } from "lucide-react";
import { Button, Skeleton } from "@/components/ui";
import { cn } from "@/lib/utils";

export type FieldType = "signature" | "initials" | "text" | "image";

export interface FieldWidget {
  id: string;
  field_type: FieldType;
  page_number: number;
  x: number;      // % from left
  y: number;      // % from top
  width: number;  // % of page width
  height: number; // % of page height
  signer_id: string | null;
  label: string;
  value?: string; // uploaded image data URL for "image" fields
}

interface Signer {
  id: string;
  name: string;
  email: string;
  sign_order: number;
}

interface Props {
  pdfUrl: string;
  authToken?: string;
  signers: Signer[];
  fields: FieldWidget[];
  onFieldsChange: (fields: FieldWidget[]) => void;
}

// Theme tokens, so the marks keep their contrast in light and dark. The signer's order number is
// shown on each field too, so colour is never the only cue.
const SIGNER_TONES = ["--tone-blue", "--tone-violet", "--tone-orange", "--tone-neutral"];
const toneColor = (i: number, alpha = 1) => `rgb(var(${SIGNER_TONES[i % SIGNER_TONES.length]}) / ${alpha})`;
const UNASSIGNED = "--fg-subtle";

const FIELD_ICONS: Record<FieldType, React.ElementType> = { signature: PenLine, initials: Type, text: Type, image: ImageIcon };

const FIELD_DEFAULTS: Record<FieldType, { width: number; height: number; label: string }> = {
  signature: { width: 22, height: 7, label: "Signature" },
  initials:  { width: 12, height: 6, label: "Initials" },
  text:      { width: 20, height: 4, label: "Text Field" },
  image:     { width: 15, height: 10, label: "Image / Seal" },
};

function nanoid() {
  return Math.random().toString(36).slice(2, 10);
}

export default function PDFFieldEditor({ pdfUrl, authToken, signers, fields, onFieldsChange }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const canvasRefs = useRef<Map<number, HTMLCanvasElement>>(new Map());
  const [numPages, setNumPages] = useState(0);
  const [pageHeights, setPageHeights] = useState<number[]>([]);
  const [activeTool, setActiveTool] = useState<FieldType>("signature");
  const [activeSignerId, setActiveSignerId] = useState<string | null>(signers[0]?.id ?? null);
  const [dragging, setDragging] = useState<{ fieldId: string; startX: number; startY: number; origX: number; origY: number } | null>(null);
  const [resizing, setResizing] = useState<{ fieldId: string; startX: number; startY: number; origW: number; origH: number } | null>(null);
  // Tracks whether mouse actually moved during a drag/resize so the overlay onClick
  // doesn't misfire as a "place new field" click when the user releases after dragging.
  const hasDraggedRef = useRef(false);
  const [pdfLoaded, setPdfLoaded] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  // Load PDF pages using PDF.js
  useEffect(() => {
    if (!pdfUrl) return;
    let cancelled = false;

    async function loadPdf() {
      try {
        const pdfjsLib = await import("pdfjs-dist");
        pdfjsLib.GlobalWorkerOptions.workerSrc = "/pdf.worker.min.mjs";

        const loadingTask = pdfjsLib.getDocument(
          authToken
            ? { url: pdfUrl, httpHeaders: { Authorization: `Bearer ${authToken}` } }
            : pdfUrl
        );
        const pdf = await loadingTask.promise;
        if (cancelled) return;
        setNumPages(pdf.numPages);

        const heights: number[] = [];
        for (let pageNum = 1; pageNum <= pdf.numPages; pageNum++) {
          const page = await pdf.getPage(pageNum);
          if (cancelled) return;
          const viewport = page.getViewport({ scale: 1.5 });
          heights.push(viewport.height);

          const canvas = canvasRefs.current.get(pageNum);
          if (!canvas) continue;
          canvas.width = viewport.width;
          canvas.height = viewport.height;
          const ctx = canvas.getContext("2d");
          if (!ctx) continue;
          await page.render({ canvasContext: ctx, viewport }).promise;
        }
        if (!cancelled) {
          setPageHeights(heights);
          setPdfLoaded(true);
        }
      } catch (err) {
        if (!cancelled) setLoadError("Failed to load PDF. Please ensure the file is uploaded.");
      }
    }

    loadPdf();
    return () => { cancelled = true; };
  }, [pdfUrl]);

  const getPageRect = useCallback((pageNum: number) => {
    const canvas = canvasRefs.current.get(pageNum);
    if (!canvas) return null;
    return canvas.getBoundingClientRect();
  }, []);

  function addField(pageNum: number, clickX: number, clickY: number, value?: string) {
    const rect = getPageRect(pageNum);
    if (!rect) return;
    const defaults = FIELD_DEFAULTS[activeTool];
    const xPct = ((clickX - rect.left) / rect.width) * 100 - defaults.width / 2;
    const yPct = ((clickY - rect.top) / rect.height) * 100 - defaults.height / 2;
    const newField: FieldWidget = {
      id: nanoid(),
      field_type: activeTool,
      page_number: pageNum,
      x: Math.max(0, Math.min(100 - defaults.width, xPct)),
      y: Math.max(0, Math.min(100 - defaults.height, yPct)),
      width: defaults.width,
      height: defaults.height,
      signer_id: activeTool === "image" ? null : activeSignerId,   // image/seal isn't signer-bound
      label: defaults.label,
      value,
    };
    onFieldsChange([...fields, newField]);
  }

  // For image/seal fields: a click opens a file picker, then places the field with the image.
  const imageInputRef = useRef<HTMLInputElement>(null);
  const pendingPlace = useRef<{ pageNum: number; x: number; y: number } | null>(null);
  function handlePlaceClick(pageNum: number, x: number, y: number) {
    if (activeTool === "image") {
      pendingPlace.current = { pageNum, x, y };
      imageInputRef.current?.click();
      return;
    }
    addField(pageNum, x, y);
  }
  function onImagePicked(file: File) {
    const place = pendingPlace.current;
    if (!place) return;
    const reader = new FileReader();
    reader.onload = () => addField(place.pageNum, place.x, place.y, String(reader.result));
    reader.readAsDataURL(file);
    pendingPlace.current = null;
  }

  function removeField(id: string) {
    onFieldsChange(fields.filter(f => f.id !== id));
  }

  function handleMouseDown(e: React.PointerEvent, fieldId: string) {
    e.stopPropagation();
    const field = fields.find(f => f.id === fieldId);
    if (!field) return;
    setDragging({ fieldId, startX: e.clientX, startY: e.clientY, origX: field.x, origY: field.y });
  }

  function handleResizeMouseDown(e: React.PointerEvent, fieldId: string) {
    e.stopPropagation();
    e.preventDefault();
    const field = fields.find(f => f.id === fieldId);
    if (!field) return;
    setResizing({ fieldId, startX: e.clientX, startY: e.clientY, origW: field.width, origH: field.height });
  }

  function handleMouseMove(e: React.PointerEvent<HTMLDivElement>) {
    if (dragging) {
      hasDraggedRef.current = true;
      const field = fields.find(f => f.id === dragging.fieldId);
      if (!field) return;
      const rect = getPageRect(field.page_number);
      if (!rect) return;
      const dx = ((e.clientX - dragging.startX) / rect.width) * 100;
      const dy = ((e.clientY - dragging.startY) / rect.height) * 100;
      onFieldsChange(fields.map(f =>
        f.id === dragging.fieldId
          ? { ...f, x: Math.max(0, Math.min(100 - f.width, dragging.origX + dx)), y: Math.max(0, Math.min(100 - f.height, dragging.origY + dy)) }
          : f
      ));
    }
    if (resizing) {
      hasDraggedRef.current = true;
      const field = fields.find(f => f.id === resizing.fieldId);
      if (!field) return;
      const rect = getPageRect(field.page_number);
      if (!rect) return;
      const dw = ((e.clientX - resizing.startX) / rect.width) * 100;
      const dh = ((e.clientY - resizing.startY) / rect.height) * 100;
      onFieldsChange(fields.map(f =>
        f.id === resizing.fieldId
          ? { ...f, width: Math.max(5, resizing.origW + dw), height: Math.max(3, resizing.origH + dh) }
          : f
      ));
    }
  }

  function handleMouseUp() {
    setDragging(null);
    setResizing(null);
  }

  const signerIndex = Object.fromEntries(signers.map((s, i) => [s.id, i]));
  const signerOrder = Object.fromEntries(signers.map((s) => [s.id, s.sign_order]));

  return (
    <div className="flex flex-col h-full" onPointerMove={handleMouseMove} onPointerUp={handleMouseUp} onPointerCancel={handleMouseUp}>
      {/* Toolbar */}
      <div className="flex flex-wrap items-center gap-2 border-b border-line bg-surface px-4 py-2.5">
        <span className="text-xs font-medium text-fg-muted">Field type</span>
        {(["signature", "initials", "text", "image"] as FieldType[]).map(type => (
          <Button key={type} size="sm" variant={activeTool === type ? "primary" : "secondary"} aria-pressed={activeTool === type}
            onClick={() => setActiveTool(type)}>
            {type === "image" ? "Image / Seal" : type.charAt(0).toUpperCase() + type.slice(1)}
          </Button>
        ))}
        <input ref={imageInputRef} type="file" accept="image/*" className="hidden"
          onChange={(e) => { const f = e.target.files?.[0]; if (f) onImagePicked(f); e.target.value = ""; }} />
        {activeTool === "image" && <span className="text-xs text-tone-orange">Click the PDF, then pick an image</span>}

        <span className="mx-1 h-5 w-px bg-line" aria-hidden />

        <span className="text-xs font-medium text-fg-muted">Assign to</span>
        {signers.map((s, i) => {
          const on = activeSignerId === s.id;
          return (
            <button key={s.id} type="button" onClick={() => setActiveSignerId(s.id)} aria-pressed={on}
              style={{ borderColor: toneColor(i), backgroundColor: on ? toneColor(i, 0.16) : "transparent" }}
              className={cn("inline-flex min-h-11 items-center rounded-lg border-2 px-2.5 text-xs font-medium transition-colors md:min-h-7",
                on ? "text-fg" : "text-fg-muted hover:text-fg")}>
              {s.name} (#{s.sign_order})
            </button>
          );
        })}

        <span className="mx-1 h-5 w-px bg-line" aria-hidden />
        <span className="text-xs text-fg-subtle">Click on the PDF to place a field</span>
      </div>

      {/* PDF + overlay area */}
      <div className="flex-1 space-y-4 overflow-y-auto bg-surface-2 p-4" ref={containerRef}>
        {loadError && (
          <div className="rounded-lg bg-tone-orange-soft p-4 text-center text-sm text-tone-orange" role="alert">
            {loadError}
          </div>
        )}

        {!pdfLoaded && !loadError && (
          <Skeleton className="mx-auto h-[60vh] max-w-2xl" aria-label="Loading PDF" />
        )}

        {Array.from({ length: Math.max(numPages, 1) }, (_, i) => i + 1).map(pageNum => (
          <div key={pageNum} className="relative mx-auto overflow-hidden rounded-lg shadow-pop"
            style={{ display: pdfLoaded ? "block" : (pageNum === 1 ? "block" : "none"), width: "fit-content", maxWidth: "100%" }}>

            {/* PDF canvas */}
            <canvas
              ref={(el) => { if (el) canvasRefs.current.set(pageNum, el); }}
              className="block h-auto max-w-full"
            />

            {/* Annotation overlay: click to add a field */}
            {pdfLoaded && (
              <div
                className="absolute inset-0 cursor-crosshair"
                style={{ zIndex: 10 }}
                onClick={(e) => {
                  // hasDraggedRef is set during mousemove; by the time onClick fires
                  // after mouseup, the dragging/resizing state is already null, so we
                  // use the ref instead to reliably suppress spurious field creation.
                  if (hasDraggedRef.current) { hasDraggedRef.current = false; return; }
                  handlePlaceClick(pageNum, e.clientX, e.clientY);
                }}
              >
                {/* Render field widgets for this page */}
                {fields.filter(f => f.page_number === pageNum).map(field => {
                  const idx = field.signer_id != null ? signerIndex[field.signer_id] : undefined;
                  const color = idx != null ? toneColor(idx) : `rgb(var(${UNASSIGNED}))`;
                  const tint = idx != null ? toneColor(idx, 0.13) : `rgb(var(${UNASSIGNED}) / 0.13)`;
                  const Icon = FIELD_ICONS[field.field_type];
                  return (
                    <div
                      key={field.id}
                      className="group absolute cursor-move select-none rounded border-2"
                      style={{
                        left: `${field.x}%`, top: `${field.y}%`,
                        width: `${field.width}%`, height: `${field.height}%`,
                        borderColor: color,
                        backgroundColor: tint,
                        zIndex: 20,
                        touchAction: "none",
                      }}
                      onPointerDown={(e) => handleMouseDown(e, field.id)}
                      onClick={(e) => e.stopPropagation()}
                    >
                      {/* Label / thumbnail */}
                      {field.field_type === "image" && field.value ? (
                        // eslint-disable-next-line @next/next/no-img-element
                        <img src={field.value} alt="seal" className="absolute inset-0 w-full h-full object-contain pointer-events-none p-0.5" />
                      ) : (
                        <span className="pointer-events-none absolute inset-0 flex items-center justify-center gap-1 text-[10px] font-semibold"
                          style={{ color }}>
                          <Icon className="size-3 shrink-0" aria-hidden />
                          {field.label}
                          {field.signer_id && signerOrder[field.signer_id] != null && <span>#{signerOrder[field.signer_id]}</span>}
                        </span>
                      )}

                      {/* Delete button */}
                      <button type="button" aria-label={`Remove ${field.label}`} title="Remove field"
                        className="absolute -right-3 -top-3 flex size-5 items-center justify-center rounded-full bg-brand text-brand-on transition-opacity hover:bg-brand-hover [@media(hover:hover)]:opacity-0 [@media(hover:hover)]:group-hover:opacity-100 [@media(hover:hover)]:focus-within:opacity-100 [@media(hover:hover)]:focus-visible:opacity-100 [@media(pointer:coarse)]:-right-4 [@media(pointer:coarse)]:-top-4 [@media(pointer:coarse)]:size-8"
                        onClick={(e) => { e.stopPropagation(); removeField(field.id); }}
                        style={{ zIndex: 30 }}>
                        <X className="size-3" aria-hidden />
                      </button>

                      {/* Resize handle */}
                      <div
                        className="absolute bottom-0 right-0 w-3 h-3 cursor-se-resize rounded-tl [@media(pointer:coarse)]:size-5"
                        style={{ backgroundColor: color, touchAction: "none" }}
                        onPointerDown={(e) => handleResizeMouseDown(e, field.id)}
                      />
                    </div>
                  );
                })}
              </div>
            )}

            {/* Page label */}
            <div className="pointer-events-none absolute bottom-1 right-2 text-[10px] text-fg-subtle">
              Page {pageNum}
            </div>
          </div>
        ))}
      </div>

      {/* Footer summary */}
      <div className="flex flex-wrap items-center gap-4 border-t border-line bg-surface px-4 py-2 text-xs text-fg-muted">
        <span className="tabular">{fields.length} field{fields.length !== 1 ? "s" : ""} placed</span>
        {signers.map((s, i) => (
          <span key={s.id} className="flex items-center gap-1.5 tabular">
            <span className="size-2 rounded-full" style={{ backgroundColor: toneColor(i) }} aria-hidden />
            {s.name}: {fields.filter(f => f.signer_id === s.id).length}
          </span>
        ))}
      </div>
    </div>
  );
}
