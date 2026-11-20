"use client";

import { useEffect, useMemo, useState } from "react";
import { AlertCircle } from "lucide-react";
import { financeApi } from "@/lib/api";
import { Select } from "@/components/ui";

export interface BankAccountOption {
  id: string;
  name: string;
  bank_name: string | null;
  account_number: string | null;
  currency: string;
  gl_account_id: string | null;
  is_active?: boolean;
}

// One request per page load, shared by every picker on the page. A failed request (e.g. a 403
// for users without finance:manage_bank) is not cached, and leaves only the default option.
let cache: Promise<BankAccountOption[]> | null = null;
function loadBankAccounts(): Promise<BankAccountOption[]> {
  if (!cache) {
    cache = financeApi.listBankAccounts()
      .then(res => ((res.data ?? []) as BankAccountOption[]).filter(a => a.is_active !== false))
      .catch(() => { cache = null; return []; });
  }
  return cache;
}

interface Props {
  value: string;
  onChange: (id: string) => void;
  /** When given, accounts in this currency are listed first. */
  currency?: string;
  className?: string;
  disabled?: boolean;
}

/** Bank account a payment moved through. "" = the bank_default GL role (Finance > Accounts > Account mapping). */
export default function BankAccountSelect({ value, onChange, currency, className, disabled }: Props) {
  const [accounts, setAccounts] = useState<BankAccountOption[]>([]);

  useEffect(() => {
    let alive = true;
    loadBankAccounts().then(a => { if (alive) setAccounts(a); });
    return () => { alive = false; };
  }, []);

  const ordered = useMemo(() => {
    if (!currency) return accounts;
    return [...accounts.filter(a => a.currency === currency), ...accounts.filter(a => a.currency !== currency)];
  }, [accounts, currency]);

  const chosen = accounts.find(a => a.id === value);

  return (
    <div>
      <Select value={value} onChange={e => onChange(e.target.value)} disabled={disabled} className={className}>
        <option value="">Default bank account</option>
        {ordered.map(a => (
          <option key={a.id} value={a.id}>{a.name} ({a.currency})</option>
        ))}
      </Select>
      {chosen && !chosen.gl_account_id && (
        <p className="mt-1 flex items-center gap-1 text-xs text-tone-orange">
          <AlertCircle className="size-3 shrink-0" aria-hidden />
          Not linked to a GL account; posting will fail
        </p>
      )}
    </div>
  );
}
