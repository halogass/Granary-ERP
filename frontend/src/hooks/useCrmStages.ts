"use client";

import { useEffect, useState } from "react";
import { crmApi } from "@/lib/api";
import { EMPTY_REGISTRY, type FunnelRegistry } from "@/lib/crmStages";

// One GET /crm/stages per page load, shared by every component that asks.
let cached: Promise<FunnelRegistry> | null = null;

function loadRegistry(): Promise<FunnelRegistry> {
  if (!cached) {
    cached = crmApi.getStages().then((r) => r.data as FunnelRegistry).catch(() => {
      cached = null;          // a failed load may be retried by the next caller
      return EMPTY_REGISTRY;
    });
  }
  return cached;
}

/** The funnel vocabulary the backend owns (stages, statuses, transitions, picklists). */
export function useCrmStages() {
  const [reg, setReg] = useState<FunnelRegistry>(EMPTY_REGISTRY);
  const [loaded, setLoaded] = useState(false);
  useEffect(() => {
    let alive = true;
    loadRegistry().then((r) => { if (alive) { setReg(r); setLoaded(true); } });
    return () => { alive = false; };
  }, []);
  return { reg, loaded };
}

/** For tests: forget the cached registry. */
export function resetCrmStagesCache() {
  cached = null;
}
