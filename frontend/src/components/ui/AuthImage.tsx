"use client";

import { useEffect, useState } from "react";
import { attachmentApi } from "@/lib/api";
import { cn } from "@/lib/utils";
import { safeObjectUrl } from "@/lib/safeFile";

/** Image thumbnail loaded through the authenticated download proxy (no public MinIO needed). */
export default function AuthImage({ id, alt, mime, className }: { id: string; alt: string; mime: string; className?: string }) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    let objUrl: string | null = null;
    attachmentApi.download(id)
      .then((res) => { if (alive) { objUrl = safeObjectUrl(res.data, mime); setSrc(objUrl); } })
      .catch(() => {});
    return () => { alive = false; if (objUrl) URL.revokeObjectURL(objUrl); };
  }, [id, mime]);
  if (!src) return <div role="img" aria-label={alt} className={cn(className, "animate-pulse bg-surface-2")} />;
  // eslint-disable-next-line @next/next/no-img-element
  return <img src={src} alt={alt} className={className} />;
}
