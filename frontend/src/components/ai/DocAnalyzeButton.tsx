"use client";

import { useState } from "react";
import { Sparkles, Loader2 } from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { aiApi } from "@/lib/api";
import { Button, Dialog, Textarea } from "@/components/ui";

type AnalyzeBody = {
  source: "attachment" | "email" | "crm_email";
  attachment_id?: string;
  message_id?: string;
  att_id?: string;
};

/** PDF/image-only "Ask AI about this document" action. Renders nothing for unsupported types. */
export default function DocAnalyzeButton({
  body, filename, mime, className,
}: { body: AnalyzeBody; filename?: string; mime?: string; className?: string }) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const [loading, setLoading] = useState(false);
  const [answer, setAnswer] = useState("");
  const [error, setError] = useState("");

  const supported = !mime || mime === "application/pdf" || mime.startsWith("image/");
  if (!supported) return null;

  const run = async () => {
    setLoading(true); setError(""); setAnswer("");
    try {
      const res = await aiApi.analyzeDocument({ ...body, question: q.trim() || undefined });
      setAnswer(res.data?.answer || "No answer produced.");
    } catch (e) {
      const detail = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      setError(detail || "Analysis failed. Check Settings, AI.");
    } finally { setLoading(false); }
  };

  return (
    <>
      <button type="button" onClick={(e) => { e.stopPropagation(); setOpen(true); }} title="Ask AI about this document"
        aria-label="Ask AI about this document"
        className={className || "rounded-lg p-1.5 text-fg-subtle transition-colors hover:bg-tone-violet-soft hover:text-tone-violet"}>
        <Sparkles className="w-4 h-4" aria-hidden />
      </button>

      <Dialog open={open} onOpenChange={setOpen} size="md"
        title={<span className="inline-flex min-w-0 items-center gap-2"><Sparkles className="size-4 shrink-0 text-tone-violet" aria-hidden /><span className="truncate">Ask AI · {filename || "document"}</span></span>}>
        <div onClick={(e) => e.stopPropagation()}>
          <div className="flex items-end gap-2">
            <Textarea value={q} onChange={(e) => setQ(e.target.value)} rows={2} aria-label="Question about the document"
              onKeyDown={(e) => { if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) run(); }}
              placeholder="Ask anything, or leave blank to summarize and extract key fields"
              className="flex-1 resize-none" />
            <Button variant="primary" onClick={run} loading={loading}>
              {loading ? null : "Analyze"}
            </Button>
          </div>
          {error && <p className="mt-2 text-xs text-tone-red">{error}</p>}
          {answer && (
            <div className="prose prose-sm mt-3 max-w-none border-t border-line pt-3 text-fg prose-p:my-1 prose-ul:my-1">
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{answer}</ReactMarkdown>
            </div>
          )}
          {loading && !answer && <p role="status" className="mt-3 flex items-center gap-1.5 text-xs text-fg-muted"><Loader2 className="size-3 animate-spin" aria-hidden /> Reading the document...</p>}
        </div>
      </Dialog>
    </>
  );
}
