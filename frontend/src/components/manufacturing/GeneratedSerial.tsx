"use client";

import { useState } from "react";
import Link from "next/link";
import { Check, Copy, Printer } from "lucide-react";
import { Button, Pill } from "@/components/ui";
import { copyText } from "./units";

/**
 * A unit serial the ERP generated: shown large with a copy button, a Test pill for a test unit and, with
 * `unitId`, a link to the printable label (serial + QR, at the label size set in Settings).
 */
export default function GeneratedSerial({ serial, isTest, hint, unitId }: {
  serial: string; isTest?: boolean; hint?: string; unitId?: string;
}) {
  const [copied, setCopied] = useState(false);
  async function copy() {
    if (await copyText(serial)) {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  }
  return (
    <div role="status" aria-label="Generated serial" className="rounded-lg border border-line bg-surface-raised px-3 py-2">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-xs text-fg-muted">Serial</span>
        <span className="font-mono text-base font-semibold tracking-wide text-fg" data-testid="generated-serial">{serial}</span>
        {isTest && <Pill tone="orange">Test</Pill>}
        <span className="ml-auto flex gap-1">
          <Button size="sm" variant="ghost" onClick={copy} aria-label={`Copy serial ${serial}`}>
            {copied ? <Check aria-hidden /> : <Copy aria-hidden />} {copied ? "Copied" : "Copy"}
          </Button>
          {unitId && (
            <Button size="sm" variant="ghost" asChild>
              <Link href={`/manufacturing/units/${unitId}/label`} target="_blank" rel="noopener">
                <Printer aria-hidden /> Print label
              </Link>
            </Button>
          )}
        </span>
      </div>
      {hint && <p className="mt-1 text-xs text-fg-muted">{hint}</p>}
    </div>
  );
}
