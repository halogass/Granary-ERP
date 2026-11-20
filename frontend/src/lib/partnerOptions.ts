"use client";

import { useEffect, useState } from "react";
import { partnerApi, type PartnerOption, type PartnerOptions } from "@/lib/api";

const EMPTY: PartnerOptions = { tiers: [], partner_types: [], statuses: [] };

/** The label of `value` in `list`, else the value with underscores as spaces. */
export function optionLabel(list: PartnerOption[], value: string | null | undefined): string {
  if (!value) return "-";
  return list.find((o) => o.value === value)?.label ?? value.replace(/_/g, " ");
}

/** The partner tiers, types and statuses from the API (one list for every partner screen). */
export function usePartnerOptions(): PartnerOptions {
  const [opts, setOpts] = useState<PartnerOptions>(EMPTY);
  useEffect(() => {
    let alive = true;
    partnerApi.options()
      .then((r) => { if (alive && r.data) setOpts({ ...EMPTY, ...r.data }); })
      .catch(() => { /* the lists stay empty; values show as stored */ });
    return () => { alive = false; };
  }, []);
  return opts;
}
