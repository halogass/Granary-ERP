"use client";

import type { Rgb } from "@/lib/faceFlash";

/**
 * The passive check's colour flash: the whole screen in one soft colour, so its light reaches the face.
 * Rendered only while a colour is up; the camera keeps running underneath.
 */
export default function FlashOverlay({ color }: { color: Rgb | null }) {
  if (!color) return null;
  return (
    <div data-testid="flash-overlay" className="fixed inset-0 z-[100] flex items-center justify-center"
      style={{ backgroundColor: `rgb(${color[0]}, ${color[1]}, ${color[2]})` }}>
      <p className="text-lg font-semibold text-black/60">Hold still</p>
    </div>
  );
}
