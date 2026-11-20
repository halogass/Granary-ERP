"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { Paperclip, Send, Smile, X, FileText, Film } from "lucide-react";
import MentionDropdown, { MentionItem } from "./MentionDropdown";
import { messagingApi } from "@/lib/api";
import { IconButton, controlClass } from "@/components/ui";
import { cn } from "@/lib/utils";

const COMMON_EMOJIS = ["👍", "❤️", "😂", "🎉", "🔥", "👏", "😍", "🚀", "💯", "✅", "😢", "🤔", "💪", "🙏", "⚡", "🎯", "💡", "📌", "⚠️", "✨"];

interface PendingFile {
  file: File;
  preview?: string;
  uploading: boolean;
  uploaded?: { url: string; file_name: string; file_size: number; mime_type: string; message_type: string };
}

interface Props {
  placeholder: string;
  onSend: (payload: {
    content: string;
    message_type: string;
    file_url?: string;
    file_name?: string;
    file_size?: number;
    mime_type?: string;
    mentions: MentionItem[];
  }) => Promise<void>;
  onTyping?: (isTyping: boolean) => void;
  disabled?: boolean;
}

export default function Composer({ placeholder, onSend, onTyping, disabled }: Props) {
  const [content, setContent] = useState("");
  const [mentions, setMentions] = useState<MentionItem[]>([]);
  const [pendingFiles, setPendingFiles] = useState<PendingFile[]>([]);
  const [showEmoji, setShowEmoji] = useState(false);
  const [mentionTrigger, setMentionTrigger] = useState<"@" | "#" | null>(null);
  const [mentionQuery, setMentionQuery] = useState("");
  const [sending, setSending] = useState(false);

  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const typingTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Auto-resize textarea
  useEffect(() => {
    const ta = textareaRef.current;
    if (!ta) return;
    ta.style.height = "auto";
    ta.style.height = Math.min(ta.scrollHeight, 160) + "px";
  }, [content]);

  const handleInput = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const val = e.target.value;
    setContent(val);

    // Typing indicator
    onTyping?.(true);
    if (typingTimer.current) clearTimeout(typingTimer.current);
    typingTimer.current = setTimeout(() => onTyping?.(false), 2000);

    // Detect mention trigger
    const cursor = e.target.selectionStart;
    const before = val.slice(0, cursor);
    const atMatch = before.match(/@(\w*)$/);
    const hashMatch = before.match(/#(\w*)$/);

    if (atMatch) {
      setMentionTrigger("@");
      setMentionQuery(atMatch[1]);
    } else if (hashMatch) {
      setMentionTrigger("#");
      setMentionQuery(hashMatch[1]);
    } else {
      setMentionTrigger(null);
      setMentionQuery("");
    }
  };

  const insertMention = useCallback((item: MentionItem) => {
    const ta = textareaRef.current;
    if (!ta) return;
    const cursor = ta.selectionStart;
    const before = content.slice(0, cursor);
    const after = content.slice(cursor);
    const trigger = mentionTrigger ?? "@";

    // Remove the partial trigger text
    const replaced = before.replace(new RegExp(`[${trigger}]\\w*$`), "");
    const token = trigger === "@"
      ? `<@user:${item.id}>`
      : `<#${item.type}:${item.id}>`;
    const displayText = trigger === "@" ? `@${item.name} ` : `#${item.name} `;

    // We store the token in a hidden way: replace with display text for UX, but track tokens via mentions state
    const newContent = replaced + displayText + after;
    setContent(newContent);
    setMentions((prev) => [...prev, item]);
    setMentionTrigger(null);
    setMentionQuery("");
    setTimeout(() => {
      ta.focus();
      const pos = (replaced + displayText).length;
      ta.setSelectionRange(pos, pos);
    }, 0);
  }, [content, mentionTrigger]);

  const buildFinalContent = () => {
    // Replace @Name and #Name display text back to tokens for storage
    let result = content;
    for (const m of mentions) {
      const display = m.type === "user" ? `@${m.name}` : `#${m.name}`;
      const token = m.type === "user" ? `<@user:${m.id}>` : `<#${m.type}:${m.id}>`;
      result = result.replace(display, token);
    }
    return result;
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && mentionTrigger === null) {
      e.preventDefault();
      handleSend();
    }
  };

  const handleSend = async () => {
    const trimmed = content.trim();
    const readyFiles = pendingFiles.filter((f) => f.uploaded);
    if (!trimmed && readyFiles.length === 0) return;
    if (disabled || sending) return;

    setSending(true);
    try {
      const finalContent = buildFinalContent();

      if (readyFiles.length > 0) {
        for (const pf of readyFiles) {
          const u = pf.uploaded!;
          await onSend({
            content: finalContent || u.file_name,
            message_type: u.message_type,
            file_url: u.url,
            file_name: u.file_name,
            file_size: u.file_size,
            mime_type: u.mime_type,
            mentions,
          });
        }
        // Send text separately if there's also text
        if (trimmed && readyFiles.length > 0) {
          // already sent content with first file; if multiple files, text goes with first only
        }
      } else {
        await onSend({ content: finalContent, message_type: "text", mentions });
      }

      setContent("");
      setMentions([]);
      setPendingFiles([]);
      onTyping?.(false);
    } finally {
      setSending(false);
    }
  };

  const handleFileChange = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const files = Array.from(e.target.files ?? []);
    e.target.value = "";
    for (const file of files) {
      const pf: PendingFile = { file, uploading: true };
      if (file.type.startsWith("image/")) {
        pf.preview = URL.createObjectURL(file);
      }
      setPendingFiles((prev) => [...prev, pf]);

      const fd = new FormData();
      fd.append("file", file);
      try {
        const res = await messagingApi.uploadFile(fd);
        setPendingFiles((prev) =>
          prev.map((p) => p.file === file ? { ...p, uploading: false, uploaded: res.data } : p)
        );
      } catch {
        setPendingFiles((prev) => prev.filter((p) => p.file !== file));
      }
    }
  };

  const removeFile = (file: File) => {
    setPendingFiles((prev) => {
      const pf = prev.find((p) => p.file === file);
      if (pf?.preview) URL.revokeObjectURL(pf.preview);
      return prev.filter((p) => p.file !== file);
    });
  };

  const canSend = (content.trim() || pendingFiles.some((f) => f.uploaded)) && !sending && !disabled;

  return (
    <div className="shrink-0 border-t border-line bg-surface px-4 py-3">
      {/* Pending file previews */}
      {pendingFiles.length > 0 && (
        <div className="mb-3 flex flex-wrap gap-2">
          {pendingFiles.map((pf, i) => (
            <div key={i} className="group relative">
              {pf.preview ? (
                // eslint-disable-next-line @next/next/no-img-element
                <img src={pf.preview} alt={pf.file.name} className="size-16 rounded-lg border border-line object-cover" />
              ) : pf.file.type.startsWith("video/") ? (
                <div className="flex size-16 items-center justify-center rounded-lg border border-line bg-surface-2">
                  <Film className="size-6 text-fg-subtle" aria-label={pf.file.name} />
                </div>
              ) : (
                <div className="flex size-16 items-center justify-center rounded-lg border border-line bg-surface-2">
                  <FileText className="size-6 text-fg-subtle" aria-label={pf.file.name} />
                </div>
              )}
              {pf.uploading && (
                <div className="absolute inset-0 flex items-center justify-center rounded-lg bg-black/40" aria-label="Uploading">
                  <div className="size-4 animate-spin rounded-full border-2 border-white border-t-transparent" />
                </div>
              )}
              <button
                type="button"
                onClick={() => removeFile(pf.file)}
                aria-label={`Remove ${pf.file.name}`}
                className="absolute -right-1.5 -top-1.5 flex size-5 items-center justify-center rounded-full bg-fg text-surface transition-opacity focus:opacity-100 group-hover:opacity-100 [@media(hover:hover)]:opacity-0"
              >
                <X className="size-3" aria-hidden />
              </button>
            </div>
          ))}
        </div>
      )}

      <div className="flex items-end gap-2">
        <IconButton icon={Paperclip} label="Attach files" onClick={() => fileInputRef.current?.click()} className="shrink-0" />
        <input
          ref={fileInputRef}
          type="file"
          multiple
          accept="image/*,video/*,.pdf,.doc,.docx,.xls,.xlsx,.ppt,.pptx,.txt,.zip"
          className="hidden"
          onChange={handleFileChange}
        />

        {/* Text input */}
        <div className="relative flex-1">
          <MentionDropdown
            trigger={mentionTrigger}
            query={mentionQuery}
            onSelect={insertMention}
            onClose={() => { setMentionTrigger(null); setMentionQuery(""); }}
          />
          <textarea
            ref={textareaRef}
            value={content}
            onChange={handleInput}
            onKeyDown={handleKeyDown}
            placeholder={placeholder}
            aria-label={placeholder}
            rows={1}
            disabled={disabled}
            className={cn(controlClass, "block min-h-11 resize-none py-2.5 leading-relaxed md:min-h-9 md:py-1.5")}
          />
        </div>

        {/* Emoji picker */}
        <div className="relative shrink-0">
          <IconButton icon={Smile} label="Insert emoji" aria-expanded={showEmoji} onClick={() => setShowEmoji((v) => !v)} />
          {showEmoji && (
            <div className="absolute bottom-full right-0 z-50 mb-2 w-52 rounded-lg border border-line bg-surface-raised p-2 shadow-pop">
              <div className="grid grid-cols-5 gap-1">
                {COMMON_EMOJIS.map((e) => (
                  <button
                    key={e}
                    type="button"
                    aria-label={`Insert ${e}`}
                    onClick={() => { setContent((c) => c + e); setShowEmoji(false); }}
                    className="flex size-8 items-center justify-center rounded-lg text-lg transition-colors hover:bg-surface-2"
                  >
                    {e}
                  </button>
                ))}
              </div>
            </div>
          )}
        </div>

        <IconButton icon={Send} label="Send" variant="primary" disabled={!canSend} onClick={handleSend} className="shrink-0" />
      </div>
    </div>
  );
}
