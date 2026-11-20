"use client";

import { useEffect, useState } from "react";
import { useParams } from "next/navigation";
import { Printer } from "lucide-react";
import { manufacturingApi } from "@/lib/api";
import { apiErrorMessage } from "@/components/manufacturing/units";
import { Button, PageHeader, Skeleton } from "@/components/ui";

// Shape of GET /manufacturing/units/{id}/label.
export interface UnitLabel {
  id: string;
  serial_number: string;
  is_test: boolean;
  hardware_revision: string | null;
  product_sku: string | null;
  product_name: string | null;
  qr_url: string;
  qr: string;        // PNG data URI of qr_url
  width_mm: number;  // manufacturing.label.width_mm
  height_mm: number; // manufacturing.label.height_mm
}

/**
 * The printable unit label: the serial, the product and a QR that opens the unit in the registry, drawn at the
 * label printer's size (Settings, Operations, Manufacturing). Printing sets the page to the label size and prints
 * only the label.
 */
export default function UnitLabelPage() {
  const { id } = useParams<{ id: string }>();
  const [label, setLabel] = useState<UnitLabel | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    let live = true;
    manufacturingApi.getUnitLabel(id)
      .then((r) => { if (live) setLabel(r.data as UnitLabel); })
      .catch((e) => { if (live) setError(apiErrorMessage(e, "The label could not be loaded.")); });
    return () => { live = false; };
  }, [id]);

  const w = label?.width_mm ?? 0;
  const h = label?.height_mm ?? 0;
  // The QR is a square as tall as the label allows (2 mm margin all round); the text takes the rest.
  const qr = Math.max(0, Math.min(h, w) - 4);
  const serialPt = Math.max(6, Math.min(14, h * 0.32));

  return (
    <div className="space-y-4">
      <PageHeader title="Unit label" back={{ href: "/manufacturing/units", label: "Units" }}
        summary={label ? `${label.width_mm} x ${label.height_mm} mm, the size set in Settings, Operations, Manufacturing` : undefined}
        actions={label ? <Button variant="primary" onClick={() => window.print()}><Printer aria-hidden /> Print</Button> : undefined} />

      {error && <p role="alert" className="text-sm text-tone-red">{error}</p>}
      {!label && !error && <Skeleton className="h-28 w-64" />}

      {label && (
        <>
          {/* Print only the label, on a page the size of the label. */}
          <style>{`
            @page { size: ${w}mm ${h}mm; margin: 0; }
            @media print {
              body * { visibility: hidden !important; }
              .unit-label, .unit-label * { visibility: visible !important; }
              .unit-label { position: fixed !important; left: 0; top: 0; border: 0 !important; box-shadow: none !important; }
            }
          `}</style>
          <div
            className="unit-label flex items-center overflow-hidden border border-dashed border-line-strong bg-white text-black"
            data-testid="unit-label"
            style={{ width: `${w}mm`, height: `${h}mm`, padding: "2mm", gap: "2mm", boxSizing: "border-box" }}>
            {/* eslint-disable-next-line @next/next/no-img-element */}
            <img src={label.qr} alt={`QR code for ${label.serial_number}`} style={{ width: `${qr}mm`, height: `${qr}mm` }} />
            <div className="min-w-0 flex-1 leading-tight">
              <p className="font-mono font-bold" style={{ fontSize: `${serialPt}pt`, wordBreak: "break-all" }}>{label.serial_number}</p>
              {label.product_name && <p className="truncate" style={{ fontSize: `${Math.max(5, serialPt * 0.6)}pt` }}>{label.product_name}</p>}
              {(label.product_sku || label.hardware_revision) && (
                <p className="truncate" style={{ fontSize: `${Math.max(5, serialPt * 0.5)}pt` }}>
                  {[label.product_sku, label.hardware_revision && `Rev ${label.hardware_revision}`].filter(Boolean).join(", ")}
                </p>
              )}
              {label.is_test && <p className="font-bold" style={{ fontSize: `${Math.max(5, serialPt * 0.6)}pt` }}>TEST UNIT</p>}
            </div>
          </div>
          <p className="text-xs text-fg-muted">
            The QR opens this unit in the registry. If the label prints at the wrong size, set the label size in
            Settings, Operations, Manufacturing, and turn off scaling (&quot;Fit to page&quot;) in the print dialog.
          </p>
        </>
      )}
    </div>
  );
}
