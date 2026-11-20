"use client";

import { useEffect, useRef, useState, useCallback } from "react";
import { Paperclip, Trash2, Download, FileText, Film, Image, Loader2, Upload } from "lucide-react";
import DocAnalyzeButton from "@/components/ai/DocAnalyzeButton";
import { attachmentApi } from "@/lib/api";
import { cn } from "@/lib/utils";
import { isSafeImageType, openOrSaveFile, saveFile } from "@/lib/safeFile";
import AuthImage from "./AuthImage";
import { IconButton } from "./Button";

interface AttachmentItem {
  id: string;
  file_name: string;
  file_size: number;
  mime_type: string;
  file_type: "image" | "video" | "file";
  uploaded_by_name?: string;
  created_at: string;
  /** Set when an MCP client or Nary stored the file (upload_file, a saved preview). */
  uploaded_via?: "mcp" | "nary" | "upload_link" | null;
}

/** "via MCP" / "via Nary" for a file an AI stored; nothing for a file uploaded here. */
export function viaLabel(via?: string | null): string | null {
  if (via === "mcp") return "via MCP";
  if (via === "nary") return "via Nary";
  if (via === "upload_link") return "via upload link";
  return null;
}

interface Props {
  entityType: string;
  entityId: string;
  canUpload?: boolean;
}

function fmtSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function FileIcon({ mime }: { mime: string }) {
  if (mime.startsWith("image/")) return <Image className="size-4 text-tone-blue" aria-hidden />;
  if (mime.startsWith("video/")) return <Film className="size-4 text-tone-violet" aria-hidden />;
  return <FileText className="size-4 text-fg-subtle" aria-hidden />;
}

export default function AttachmentPanel({ entityType, entityId, canUpload = true }: Props) {
  const [items, setItems] = useState<AttachmentItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [uploading, setUploading] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const load = useCallback(async () => {
    try {
      const res = await attachmentApi.list(entityType, entityId);
      setItems(res.data ?? []);
    } catch {
      setItems([]);
    } finally {
      setLoading(false);
    }
  }, [entityType, entityId]);

  useEffect(() => { load(); }, [load]);

  async function handleFiles(files: FileList | null) {
    if (!files || files.length === 0) return;
    setUploading(true);
    try {
      for (const file of Array.from(files)) {
        await attachmentApi.upload(entityType, entityId, file);
      }
      await load();
    } finally {
      setUploading(false);
      if (inputRef.current) inputRef.current.value = "";
    }
  }

  async function handleDelete(id: string) {
    setDeletingId(id);
    try {
      await attachmentApi.delete(id);
      setItems((prev) => prev.filter((a) => a.id !== id));
    } finally {
      setDeletingId(null);
    }
  }

  // Fetch the file through the authenticated proxy, then save or open the resulting blob.
  // Uploaded files are untrusted: only safe types open in a tab, the rest is saved.
  async function downloadFile(item: AttachmentItem) {
    const res = await attachmentApi.download(item.id);
    saveFile(res.data, item.file_name, item.mime_type);
  }

  async function openFile(item: AttachmentItem) {
    const res = await attachmentApi.download(item.id);
    openOrSaveFile(res.data, item.file_name, item.mime_type);
  }

  return (
    <div className="space-y-3">
      {/* Upload zone */}
      {canUpload && (
        <>
          <button
            type="button"
            onDragOver={(e) => { e.preventDefault(); setDragOver(true); }}
            onDragLeave={() => setDragOver(false)}
            onDrop={(e) => { e.preventDefault(); setDragOver(false); handleFiles(e.dataTransfer.files); }}
            onClick={() => inputRef.current?.click()}
            className={cn(
              "flex min-h-11 w-full items-center gap-3 rounded-lg border-2 border-dashed px-4 py-3 text-left transition-colors",
              "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[rgb(var(--focus)/0.7)]",
              dragOver ? "border-brand/50 bg-brand/[0.05]" : "border-line-strong hover:bg-surface-2",
            )}
          >
            {uploading ? (
              <Loader2 className="size-4 shrink-0 animate-spin text-brand-ink" aria-hidden />
            ) : (
              <Upload className="size-4 shrink-0 text-fg-subtle" aria-hidden />
            )}
            <span className="text-xs text-fg-muted">
              {uploading ? "Uploading..." : "Drop files here or click to upload"}
            </span>
          </button>
          <input
            ref={inputRef}
            type="file"
            multiple
            className="hidden"
            onChange={(e) => handleFiles(e.target.files)}
          />
        </>
      )}

      {/* File list */}
      {loading ? (
        <div className="flex justify-center py-4">
          <Loader2 className="size-4 animate-spin text-fg-subtle" aria-label="Loading attachments" />
        </div>
      ) : items.length === 0 ? (
        <div className="flex flex-col items-center py-5 text-center">
          <Paperclip className="mb-2 size-6 text-fg-subtle" aria-hidden />
          <p className="text-xs text-fg-muted">No attachments yet</p>
        </div>
      ) : (
        <div className="space-y-1.5">
          {items.map((item) => (
            <div key={item.id} className="group flex items-center gap-3 rounded-lg bg-surface-2 px-3 py-2 transition-colors hover:bg-surface-2/70">
              {item.file_type === "image" && isSafeImageType(item.mime_type) ? (
                <button type="button" onClick={() => openFile(item)} className="shrink-0" title="Open" aria-label={`Open ${item.file_name}`}>
                  <AuthImage id={item.id} alt={item.file_name} mime={item.mime_type} className="size-8 rounded-lg object-cover" />
                </button>
              ) : (
                <button type="button" onClick={() => openFile(item)} aria-label={`Open ${item.file_name}`}
                  className="flex size-8 shrink-0 items-center justify-center rounded-lg border border-line bg-surface" title="Open">
                  <FileIcon mime={item.mime_type} />
                </button>
              )}

              <button type="button" onClick={() => openFile(item)} className="min-w-0 flex-1 text-left" title="Open">
                <p className="truncate text-xs font-medium text-fg">{item.file_name}</p>
                <p className="text-[11px] text-fg-muted tabular">
                  {fmtSize(item.file_size)}
                  {viaLabel(item.uploaded_via) && (
                    <span className="ml-1.5 rounded bg-tone-violet-soft px-1 py-px text-[10px] font-medium text-tone-violet"
                      title={item.uploaded_by_name ? `Uploaded by ${item.uploaded_by_name}` : undefined}>
                      {viaLabel(item.uploaded_via)}
                    </span>
                  )}
                </p>
              </button>

              <div className="flex items-center gap-1 transition-opacity [@media(hover:hover)]:opacity-0 [@media(hover:hover)]:focus-within:opacity-100 [@media(hover:hover)]:group-hover:opacity-100">
                <DocAnalyzeButton
                  body={{ source: "attachment", attachment_id: item.id }}
                  filename={item.file_name}
                  mime={item.mime_type}
                />
                <IconButton size="sm" icon={Download} label="Download" onClick={() => downloadFile(item)} />
                {canUpload && (
                  <IconButton size="sm" icon={deletingId === item.id ? Loader2 : Trash2} label="Delete attachment"
                    onClick={() => handleDelete(item.id)} disabled={deletingId === item.id}
                    className={cn("hover:text-brand-ink", deletingId === item.id && "[&_svg]:animate-spin")} />
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
