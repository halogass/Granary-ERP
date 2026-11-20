"use client";

import { useState, useEffect, useRef } from "react";
import { useSession } from "next-auth/react";
import { Search } from "lucide-react";
import { Dialog, EmptyState, Input, Pill, Skeleton, type Tone } from "@/components/ui";
import { cn } from "@/lib/utils";

type SignerType = "internal" | "partner" | "vendor";

interface SearchResult {
  id: string;
  name: string;
  email: string;
  subtitle: string;
  signer_type: SignerType;
  partner_account_id?: string;
}

interface Props {
  isOpen: boolean;
  onClose: () => void;
  onSelect: (result: SearchResult) => void;
}

const TYPE_LABELS: Record<SignerType, { label: string; tone: Tone }> = {
  internal: { label: "Employee", tone: "blue" },
  partner:  { label: "Partner",  tone: "violet" },
  vendor:   { label: "Vendor",   tone: "orange" },
};

const AVATAR: Record<Tone, string> = {
  blue: "bg-tone-blue-soft text-tone-blue", violet: "bg-tone-violet-soft text-tone-violet",
  orange: "bg-tone-orange-soft text-tone-orange", neutral: "bg-tone-neutral-soft text-tone-neutral",
  red: "bg-tone-red-soft text-tone-red",
};

export default function SignerPickerModal({ isOpen, onClose, onSelect }: Props) {
  const { data: session } = useSession();
  const [activeTab, setActiveTab] = useState<SignerType>("internal");
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [loading, setLoading] = useState(false);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (!isOpen) { setQuery(""); setResults([]); return; }
  }, [isOpen]);

  useEffect(() => {
    if (!isOpen) return;
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      fetchResults(activeTab, query);
    }, 300);
  }, [query, activeTab, isOpen]);

  async function fetchResults(type: SignerType, q: string) {
    if (!session?.access_token) return;
    setLoading(true);
    try {
      const params = new URLSearchParams({ type, q, limit: "20" });
      const res = await fetch(`/api/v1/sign/signers/search?${params}`, {
        headers: { Authorization: `Bearer ${session.access_token}` },
      });
      if (!res.ok) throw new Error("Search failed");
      setResults(await res.json());
    } catch {
      setResults([]);
    } finally {
      setLoading(false);
    }
  }

  function handleSelect(r: SearchResult) {
    onSelect(r);
    onClose();
  }

  const typeLabel = TYPE_LABELS[activeTab].label.toLowerCase();

  return (
    <Dialog open={isOpen} onOpenChange={(o) => { if (!o) onClose(); }} title="Add signer"
      description="Pick a person from the ERP. To add an external signer, close this and type the name and email."
      className="h-[75vh]">
      <div className="flex h-full flex-col gap-3">
        <div role="tablist" aria-label="Signer type" className="flex gap-1 border-b border-line">
          {(["internal", "partner", "vendor"] as SignerType[]).map(tab => {
            const on = activeTab === tab;
            return (
              <button key={tab} type="button" role="tab" aria-selected={on}
                onClick={() => { setActiveTab(tab); setResults([]); }}
                className={cn("-mb-px inline-flex min-h-11 items-center border-b-2 px-3 text-sm transition-colors md:min-h-9",
                  on ? "border-brand font-semibold text-fg" : "border-transparent text-fg-muted hover:text-fg")}>
                {TYPE_LABELS[tab].label}
              </button>
            );
          })}
        </div>

        <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
          value={query} onChange={(e) => setQuery(e.target.value)} autoFocus
          placeholder={`Search ${typeLabel}s by name or email...`} aria-label={`Search ${typeLabel}s`} />

        <div className="-mx-1 min-h-0 flex-1 space-y-1 overflow-y-auto px-1">
          {loading && [0, 1, 2].map(i => <Skeleton key={i} className="h-12" />)}

          {!loading && results.length === 0 && (
            <EmptyState compact title={query ? `No ${typeLabel}s found for "${query}"` : `Start typing to search ${typeLabel}s`} />
          )}

          {!loading && results.map(r => {
            const meta = TYPE_LABELS[r.signer_type];
            return (
              <button key={r.id} type="button" onClick={() => handleSelect(r)}
                className="flex min-h-11 w-full items-center gap-3 rounded-lg px-3 py-2 text-left transition-colors hover:bg-surface-2">
                <span className={cn("flex size-9 shrink-0 items-center justify-center rounded-full text-sm font-semibold", AVATAR[meta.tone])} aria-hidden>
                  {r.name.charAt(0).toUpperCase()}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm font-medium text-fg">{r.name}</span>
                  <span className="block truncate text-xs text-fg-muted">{r.email}{r.subtitle && r.subtitle !== r.email ? `, ${r.subtitle}` : ""}</span>
                </span>
                <Pill tone={meta.tone} className="shrink-0">{meta.label}</Pill>
              </button>
            );
          })}
        </div>
      </div>
    </Dialog>
  );
}
