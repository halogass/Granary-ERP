"use client";
import { useRef, useEffect, useState } from "react";
import { Eraser, Upload } from "lucide-react";
import { useFileDrop, isImage } from "@/hooks/useFileDrop";
import { Button } from "@/components/ui";
import { cn } from "@/lib/utils";

/** Draw or upload a signature image. Emits a PNG data URL (or null when empty). */
export default function SignaturePad({ value, onChange, height = 140 }: {
  value?: string | null;
  onChange: (dataUrl: string | null) => void;
  height?: number;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const drawing = useRef(false);
  const inked = useRef(false);
  const fileRef = useRef<HTMLInputElement>(null);
  const [, force] = useState(0);

  // Render an incoming value (saved/uploaded image) onto the canvas.
  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    if (value && value.startsWith("data:image")) {
      const img = new Image();
      img.onload = () => { ctx.drawImage(img, 0, 0, canvas.width, canvas.height); };
      img.src = value;
      inked.current = true;
    } else {
      inked.current = false;
    }
    force((n) => n + 1);
  }, [value]);

  function point(e: React.MouseEvent | React.TouchEvent) {
    const canvas = canvasRef.current!;
    const rect = canvas.getBoundingClientRect();
    const t = "touches" in e ? e.touches[0] : e;
    return {
      x: ((t.clientX - rect.left) * canvas.width) / rect.width,
      y: ((t.clientY - rect.top) * canvas.height) / rect.height,
    };
  }
  function start(e: React.MouseEvent | React.TouchEvent) {
    drawing.current = true;
    const ctx = canvasRef.current!.getContext("2d")!;
    const p = point(e);
    ctx.beginPath();
    ctx.moveTo(p.x, p.y);
  }
  function move(e: React.MouseEvent | React.TouchEvent) {
    if (!drawing.current) return;
    if ("touches" in e) e.preventDefault();
    const ctx = canvasRef.current!.getContext("2d")!;
    const p = point(e);
    ctx.lineTo(p.x, p.y);
    ctx.strokeStyle = "#111";
    ctx.lineWidth = 2;
    ctx.lineCap = "round";
    ctx.stroke();
    if (!inked.current) { inked.current = true; force((n) => n + 1); }
  }
  function end() {
    if (!drawing.current) return;
    drawing.current = false;
    const c = canvasRef.current!;
    onChange(inked.current ? c.toDataURL("image/png") : null);
  }
  function clear() {
    const c = canvasRef.current!;
    c.getContext("2d")!.clearRect(0, 0, c.width, c.height);
    inked.current = false;
    force((n) => n + 1);
    onChange(null);
  }
  function upload(file: File) {
    const reader = new FileReader();
    reader.onload = () => onChange(String(reader.result));   // the effect redraws it
    reader.readAsDataURL(file);
  }

  const { dragActive, dropHandlers } = useFileDrop((files) => upload(files[0]), { accept: isImage, single: true });

  return (
    <div>
      <canvas
        ref={canvasRef} width={560} height={height * 2}
        onMouseDown={start} onMouseMove={move} onMouseUp={end} onMouseLeave={end}
        onTouchStart={start} onTouchMove={move} onTouchEnd={end}
        {...dropHandlers}
        aria-label="Signature pad: draw your signature, or drop an image"
        className={cn("w-full cursor-crosshair touch-none rounded-lg border-2 border-dashed transition-colors",
          dragActive ? "border-brand" : "border-line-strong")}
        // Paper stays white in both themes: the ink is dark and the image lands on a white PDF.
        style={{ height, backgroundColor: "white" }}
      />
      <div className="flex items-center gap-2 mt-2">
        <Button size="sm" onClick={clear} disabled={!inked.current}>
          <Eraser aria-hidden /> Clear
        </Button>
        <Button size="sm" onClick={() => fileRef.current?.click()}>
          <Upload aria-hidden /> Upload image
        </Button>
        <input ref={fileRef} type="file" accept="image/*" className="hidden"
          onChange={(e) => { const f = e.target.files?.[0]; if (f) upload(f); e.target.value = ""; }} />
      </div>
    </div>
  );
}
