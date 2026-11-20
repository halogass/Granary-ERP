import { sumByCurrency } from "./currency";

export interface PaidRunLike {
  status: string;
  year: number;
  total_net: number;
  currency?: string | null;
}

/** Net salaries paid in `year`, in the runs' own currency. A run without a currency is
 *  in the functional currency, and with no paid runs the zero is shown in the functional
 *  currency too (never a hard-coded one). */
export function paidThisYear(runs: PaidRunLike[], year: number, functional: string) {
  return sumByCurrency(
    runs.filter((r) => r.status === "paid" && r.year === year),
    (r) => Number(r.total_net) || 0,
    (r) => r.currency,
    functional,
  );
}
