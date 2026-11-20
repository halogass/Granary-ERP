"use client";

// The segment of a lead or deal, editable on the record page. A new deal starts with its lead's segment.

import { useState } from "react";
import { Layers } from "lucide-react";
import { extractApiError } from "@/lib/apiError";
import { segmentPatch, useSegments } from "@/lib/segments";
import SegmentInput from "./SegmentInput";

export default function SegmentFact({ segment, onChange }: {
  segment?: string | null;
  onChange: (patch: Record<string, unknown>) => Promise<void>;
}) {
  const segments = useSegments();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  async function save(patch: Record<string, unknown>) {
    setSaving(true); setError("");
    try { await onChange(patch); }
    catch (err) { setError(extractApiError(err, "Could not save.")); }
    finally { setSaving(false); }
  }
  return (
    <>
      <div className="flex items-start gap-2.5 py-1.5">
        <Layers className="mt-0.5 size-3.5 shrink-0 text-fg-subtle" aria-hidden />
        <div className="min-w-0 flex-1">
          <p className="text-xs text-fg-muted">Segment</p>
          <SegmentInput segments={segments} value={segment ?? ""} disabled={saving} className="h-8 text-[13px] md:h-8"
            onCommit={(v) => save(segmentPatch(v))} />
        </div>
      </div>
      {error && <p role="alert" className="text-xs text-tone-red">{error}</p>}
    </>
  );
}
