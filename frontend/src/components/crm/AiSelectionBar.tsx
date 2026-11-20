"use client";

import { useEffect, useState } from "react";
import { Loader2, Sparkles, X } from "lucide-react";
import { crmRecordsApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Button } from "@/components/ui";

interface Attribute { key: string; label: string; is_ai: boolean; ai_kind?: string | null }

/**
 * The bar that appears once rows are ticked: run one AI attribute across the selection, or,
 * for companies, research them on the web.
 *
 * The run happens on a worker, so this only reports that it started; the result arrives as a
 * notification. Nothing here writes to a record directly.
 */
export default function AiSelectionBar({
  objectType, selected, onClear, onMessage,
}: {
  objectType: "organization" | "contact" | "lead" | "deal";
  selected: string[];
  onClear: () => void;
  onMessage: (kind: "success" | "info" | "error", message: string) => void;
}) {
  const [attributes, setAttributes] = useState<Attribute[]>([]);
  const [open, setOpen] = useState(false);
  const [running, setRunning] = useState(false);

  useEffect(() => {
    let alive = true;
    crmRecordsApi.listAttributes({ object_type: objectType })
      .then((r) => { if (alive) setAttributes((r.data.items ?? []).filter((a: Attribute) => a.is_ai)); })
      .catch(() => { if (alive) setAttributes([]); });
    return () => { alive = false; };
  }, [objectType]);

  if (selected.length === 0) return null;

  async function run(action: "attribute" | "enrich" | "research", attributeKey?: string) {
    setOpen(false);
    setRunning(true);
    try {
      const res = await crmRecordsApi.bulkRunAiField({
        object_type: objectType, record_ids: selected, action, attribute_key: attributeKey ?? null,
      });
      const data = res.data as { queued?: boolean; succeeded?: number; message?: string };
      if (data.queued) {
        onMessage("info", data.message ?? `Running over ${selected.length} record(s).`);
      } else {
        onMessage("success", `Done: ${data.succeeded ?? 0} of ${selected.length} updated.`);
      }
      onClear();
    } catch (e) {
      onMessage("error", extractApiError(e, "That run could not be started."));
    } finally {
      setRunning(false);
    }
  }

  const item = "block w-full px-3 py-2 text-left text-sm text-fg hover:bg-surface-2 focus-visible:bg-surface-2 focus-visible:outline-none";
  const hint = "block text-xs text-fg-muted";

  return (
    <div className="flex items-center gap-3 border-b border-line bg-brand/[0.07] px-4 py-2">
      <span className="text-sm font-medium text-fg tabular">{selected.length} selected</span>
      <div className="relative">
        <Button size="sm" variant="primary" disabled={running} onClick={() => setOpen((v) => !v)} aria-expanded={open}>
          {running ? <Loader2 className="animate-spin" aria-hidden /> : <Sparkles aria-hidden />}
          Fill with AI
        </Button>
        {open && (
          <div className="absolute left-0 z-20 mt-1 w-64 overflow-hidden rounded-lg border border-line bg-surface-raised py-1 shadow-pop">
            {(objectType === "organization" || objectType === "contact") && (
              <button type="button" onClick={() => run("research")} className={item}>
                Deep research
                <span className={hint}>
                  {objectType === "organization" ? "Facts, people and news from the web and their site" : "Role and LinkedIn from public profiles"}
                </span>
              </button>
            )}
            {objectType === "organization" && (
              <button type="button" onClick={() => run("enrich")} className={item}>
                Enrich from the web
                <span className={hint}>Fills empty fields, with sources</span>
              </button>
            )}
            {attributes.map((a) => (
              <button key={a.key} type="button" onClick={() => run("attribute", a.key)} className={item}>
                {a.label}
                <span className={hint}>{a.ai_kind}</span>
              </button>
            ))}
            {attributes.length === 0 && (objectType === "lead" || objectType === "deal") && (
              <p className="px-3 py-2 text-xs text-fg-muted">
                No AI attributes on {objectType}s yet. Add one in Settings, CRM attributes.
              </p>
            )}
          </div>
        )}
      </div>
      <Button size="sm" variant="ghost" onClick={onClear} className="ml-auto">
        <X aria-hidden /> Clear
      </Button>
    </div>
  );
}
