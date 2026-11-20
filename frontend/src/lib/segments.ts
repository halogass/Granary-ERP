/**
 * Segment: an optional, configurable label on leads, deals and catalog items (for example "Retail" or
 * "Public sector"). The choices come from the `crm.segments` setting, served with the CRM registry
 * (GET /crm/stages, `segments`). An empty list means segment is free text.
 *
 * The API field is `segment`.
 * This file is the only place that knows the name: pages read and write segments through the helpers below.
 */

import { useMemo } from "react";
import { useCrmStages } from "@/hooks/useCrmStages";

/** The API field that holds a record's segment (leads, deals, catalog items). */
export const SEGMENT_FIELD = "segment" as const;
/** The list filter's query parameter. */
export const SEGMENT_FILTER_PARAM = "segment" as const;
/** Filter value for records without a segment. */
export const SEGMENT_NONE = "none";

export interface Segment { key: string; label: string }

/** A record as the API sends it: the segment under its API name. */
export interface WithSegment {
  segment?: string | null;
}

/** The segment of a record ("" when it has none). */
export function segmentOf(r: WithSegment | null | undefined): string {
  return r?.[SEGMENT_FIELD] ?? "";
}

/** The request body part that sets a segment (an empty value clears it). */
export function segmentPatch(value: string): Record<string, string | null> {
  const v = value.trim();
  return { [SEGMENT_FIELD]: v || null };
}

/** The list query parameters for a segment filter ("" = no filter). */
export function segmentFilterParams(value: string): Record<string, string> {
  return value ? { [SEGMENT_FILTER_PARAM]: value } : {};
}

/**
 * The configured segments from the registry value: a list of keys or of {key, label}. Anything else
 * (missing, malformed) is no list, so segment stays free text.
 */
export function parseSegments(raw: unknown): Segment[] {
  if (!Array.isArray(raw)) return [];
  const out: Segment[] = [];
  for (const item of raw) {
    if (typeof item === "string" && item.trim()) out.push({ key: item.trim(), label: item.trim() });
    else if (item && typeof item === "object") {
      const { key, label } = item as { key?: unknown; label?: unknown };
      if (typeof key === "string" && key.trim()) {
        out.push({ key: key.trim(), label: typeof label === "string" && label.trim() ? label.trim() : key.trim() });
      }
    }
  }
  return out;
}

/** A segment's label; an unknown value shows as itself, none as a dash. */
export function segmentLabel(segments: Segment[], value?: string | null): string {
  if (!value) return "-";
  return segments.find((s) => s.key === value)?.label ?? value;
}

/** Select options for a segment picker. A current value that is no longer configured stays selectable. */
export function segmentOptions(segments: Segment[], current?: string | null): { value: string; label: string }[] {
  const opts = segments.map((s) => ({ value: s.key, label: s.label }));
  if (current && !segments.some((s) => s.key === current)) opts.push({ value: current, label: current });
  return opts;
}

/** Options for a list filter: every segment plus records without one. */
export function segmentFilterOptions(segments: Segment[]): { value: string; label: string }[] {
  return [...segmentOptions(segments), { value: SEGMENT_NONE, label: "No segment" }];
}

/** The configured segments (empty until the registry loads, or when none are set). */
export function useSegments(): Segment[] {
  const { reg } = useCrmStages();
  const raw = (reg as { segments?: unknown }).segments;
  return useMemo(() => parseSegments(raw), [raw]);
}
