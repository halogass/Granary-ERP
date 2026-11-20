"use client";

import { useState } from "react";
import Link from "next/link";
import { Download, FileText, Film, Image as ImageIcon } from "lucide-react";
import { Button, Pill, toast } from "@/components/ui";
import { extractApiError } from "@/lib/apiError";
import { cn } from "@/lib/utils";
import {
  audienceLabel, docLibApi, downloadStamped, fileKind, formatDuration, formatFileSize, languageLabel, type LibraryDocument,
} from "@/lib/documentLibrary";

/** Download the current live version, stamped for the signed-in user. */
export function DownloadButton({ doc, size = "sm", variant = "secondary", label = "Download", fetch }: {
  doc: Pick<LibraryDocument, "id" | "title" | "current_version">;
  size?: "sm" | "md";
  variant?: "secondary" | "primary" | "ghost";
  label?: string;
  /** Another endpoint (the partner portal's); defaults to the staff download. */
  fetch?: () => ReturnType<typeof docLibApi.download>;
}) {
  const [busy, setBusy] = useState(false);
  if (!doc.current_version) return null;
  async function go() {
    setBusy(true);
    try {
      await downloadStamped(fetch ?? (() => docLibApi.download(doc.id)), doc.current_version?.file_name ?? `${doc.title}.pdf`);
    } catch (e) {
      toast.error(extractApiError(e, "The download failed."));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Button size={size} variant={variant} loading={busy} onClick={go} aria-label={`${label} ${doc.title}`}>
      <Download aria-hidden /> {label}
    </Button>
  );
}

/** One document as a card: title, current version, language, audiences; open and download. */
export function DocumentCard({ doc, href, showAudiences = true, className, children }: {
  doc: LibraryDocument;
  /** Where the title links (the record page); none in the partner portal. */
  href?: string;
  showAudiences?: boolean;
  className?: string;
  children?: React.ReactNode;
}) {
  const v = doc.current_version;
  const k = fileKind(v?.file_name, v?.file_mime);
  const Icon = k === "image" ? ImageIcon : k === "video" ? Film : FileText;
  return (
    <div className={cn("flex h-full items-start gap-3 rounded-lg border border-line bg-surface p-3", className)}>
      <span className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-surface-2 text-fg-muted">
        <Icon className="size-5" aria-hidden />
      </span>
      <div className="min-w-0 flex-1">
        {href
          ? <Link href={href} className="block truncate text-sm font-medium text-fg hover:text-brand-ink">{doc.title}</Link>
          : <p className="truncate text-sm font-medium text-fg">{doc.title}</p>}
        <p className="mt-0.5 text-xs text-fg-muted tabular">
          {[v?.label ?? "No live version", languageLabel(doc.language), doc.category?.name, v?.page_count ? `${v.page_count} pages` : null,
            k === "video" ? (formatDuration(v?.duration_seconds) ? `Video ${formatDuration(v?.duration_seconds)}` : "Video") : null,
            v?.file_size ? formatFileSize(v.file_size) : null].filter(Boolean).join(" · ")}
        </p>
        {v?.change_note && <p className="mt-1 line-clamp-2 text-[13px] text-fg-muted">{v.change_note}</p>}
        {showAudiences && doc.audiences.length > 0 && (
          <div className="mt-1.5 flex flex-wrap gap-1">
            {doc.audiences.map((a) => <Pill key={a} tone={a === "internal" ? "neutral" : a === "partner" ? "violet" : "blue"}>{audienceLabel(a)}</Pill>)}
          </div>
        )}
        {children && <div className="mt-2 flex flex-wrap gap-2">{children}</div>}
      </div>
    </div>
  );
}
