"use client";

// The segment of a lead, deal or catalog item: a picklist when segments are configured (Settings, CRM),
// else free text. See lib/segments.ts.

import { useEffect, useState } from "react";
import { Input, Select } from "@/components/ui";
import { segmentOptions, type Segment } from "@/lib/segments";

export default function SegmentInput({ segments, value, onChange, onCommit, disabled, className, placeholder }: {
  segments: Segment[];
  value: string;
  /** Every change (form fields). */
  onChange?: (value: string) => void;
  /** A finished change: a pick, or free text on blur (inline editing). */
  onCommit?: (value: string) => void;
  disabled?: boolean;
  className?: string;
  placeholder?: string;
}) {
  const [text, setText] = useState(value);
  useEffect(() => { setText(value); }, [value]);

  if (segments.length > 0) {
    return (
      <Select aria-label="Segment" value={value} disabled={disabled} className={className} placeholder={placeholder ?? "None"}
        onChange={(e) => { onChange?.(e.target.value); onCommit?.(e.target.value); }}
        options={segmentOptions(segments, value)} />
    );
  }
  return (
    <Input aria-label="Segment" value={text} disabled={disabled} className={className} placeholder={placeholder ?? "None"}
      onChange={(e) => { setText(e.target.value); onChange?.(e.target.value); }}
      onBlur={() => { if (text.trim() !== value) onCommit?.(text.trim()); }} />
  );
}
