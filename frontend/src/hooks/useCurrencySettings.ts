"use client";

import { useEffect, useState } from "react";
import { systemApi } from "@/lib/api";

/** The org-wide currency configuration, administered under Settings → Money & tax → Currencies.
 *
 *  `functional` is the base currency for reporting and the default for new records;
 *  `active` is the subset of currencies offered in transaction dropdowns. Both are
 *  stored as SystemSetting rows (`currency.functional` / `currency.active`) and served
 *  by GET /system/currencies. */
export interface CurrencySettings {
  functional: string;
  active: string[];
}

// Mirrors the backend fallbacks in app/services/fx.py, so an unreachable API degrades
// to the same values the server would have returned rather than a different guess.
const DEFAULT: CurrencySettings = { functional: "USD", active: ["USD", "EUR"] };

// Module-level cache so multiple components share one fetch
let _cache: CurrencySettings | null = null;
let _promise: Promise<CurrencySettings> | null = null;

export async function fetchCurrencySettings(): Promise<CurrencySettings> {
  if (_cache) return _cache;
  if (!_promise) {
    _promise = systemApi
      .getCurrencySettings()
      .then((res) => {
        const active: string[] = Array.isArray(res.data?.active) && res.data.active.length
          ? res.data.active
          : DEFAULT.active;
        const functional: string = res.data?.functional || DEFAULT.functional;
        // The functional currency must always be selectable, even if an admin has
        // toggled it out of the active list.
        _cache = {
          functional,
          active: active.includes(functional) ? active : [functional, ...active],
        };
        return _cache;
      })
      .catch(() => {
        _promise = null;   // don't cache a failure: retry on the next mount
        return DEFAULT;
      });
  }
  return _promise;
}

export function invalidateCurrencySettingsCache() {
  _cache = null;
  _promise = null;
}

export function useCurrencySettings(): CurrencySettings {
  const [settings, setSettings] = useState<CurrencySettings>(_cache ?? DEFAULT);

  useEffect(() => {
    fetchCurrencySettings().then(setSettings);
  }, []);

  return settings;
}

/** Select options for a currency field: the active currencies, plus the field's current value when
 *  it is no longer active (an older record keeps showing its own currency instead of a blank). */
export function currencyOptions(active: string[], current?: string): { value: string; label: string }[] {
  const list = current && !active.includes(current) ? [current, ...active] : active;
  return list.map((c) => ({ value: c, label: c }));
}
