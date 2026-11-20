import { statusLabel } from "@/components/ui/status";

/** Display names for invoice types; the API sends snake_case codes ("credit_note"). */
export const INVOICE_TYPE_LABELS: Record<string, string> = {
  commercial: "Commercial",
  proforma: "Proforma",
  credit_note: "Credit note",
};

/** A readable invoice type: known codes get their name, any other code is humanised
 *  ("down_payment" -> "Down payment") instead of being shown raw. */
export function invoiceTypeLabel(type: string | null | undefined): string {
  return (type && INVOICE_TYPE_LABELS[type]) || statusLabel(type);
}
