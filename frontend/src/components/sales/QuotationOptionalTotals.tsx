/** 4 Nov 2026: optional quotation lines are offered, not sold. The server keeps them out of the subtotal, tax
 *  and total and gives their own figures; this block shows them under the total. An older quotation whose
 *  stored total still counts them (optional_in_total) says so instead. */
export interface OptionalTotalsQuote {
  total: number;
  items: { is_optional?: boolean | null }[];
  optional_subtotal?: number;
  optional_tax?: number;
  total_with_optional?: number;
  optional_in_total?: boolean;
}

export function OptionalTotals({ quote, fmt }: { quote: OptionalTotalsQuote; fmt: (n: number) => string }) {
  if (!quote.items.some((i) => i.is_optional)) return null;
  if (quote.optional_in_total) {
    return (
      <p className="text-xs text-fg-muted" data-testid="optional-totals">
        Priced before optional items left the total: this total includes them. A revision totals them apart.
      </p>
    );
  }
  const tax = quote.optional_tax ?? 0;
  return (
    <div className="space-y-1 border-t border-dashed border-line pt-2 text-fg-muted" data-testid="optional-totals">
      <div className="flex justify-between">
        <span>Optional items (not in the total)</span>
        <span className="font-medium text-fg tabular">{fmt(quote.optional_subtotal ?? 0)}</span>
      </div>
      {tax > 0 && (
        <div className="flex justify-between text-xs">
          <span>Their tax</span><span className="tabular">{fmt(tax)}</span>
        </div>
      )}
      <div className="flex justify-between text-xs">
        <span>Total with optional items</span>
        <span className="tabular">{fmt(quote.total_with_optional ?? quote.total)}</span>
      </div>
    </div>
  );
}
