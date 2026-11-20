"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { FileText, Search } from "lucide-react";
import { Card, EmptyState, FilterChips, Input, ListFrame, PageHeader, Skeleton } from "@/components/ui";
import { DocumentCard, DownloadButton } from "@/components/documents/DocumentCards";
import { WATERMARK_LIMIT, itemsOf, languageLabel, partnerDocsApi, type LibraryDocument } from "@/lib/documentLibrary";

export default function PartnerDocumentsPage() {
  const [docs, setDocs] = useState<LibraryDocument[]>([]);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);
  const [category, setCategory] = useState("");
  const [language, setLanguage] = useState("");
  const [q, setQ] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await partnerDocsApi.list();
      setDocs(itemsOf(res.data));
      setFailed(false);
    } catch {
      setDocs([]);
      setFailed(true);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { void load(); }, [load]);

  const categories = useMemo(() => {
    const m = new Map<string, string>();
    for (const d of docs) if (d.category) m.set(d.category.key, d.category.name);
    return Array.from(m.entries());
  }, [docs]);
  const languages = useMemo(() => Array.from(new Set(docs.map((d) => d.language).filter(Boolean))), [docs]);

  const shown = docs.filter((d) =>
    (!category || d.category?.key === category) && (!language || d.language === language)
    && (!q.trim() || d.title.toLowerCase().includes(q.trim().toLowerCase())));

  const chips = [
    ...categories.map(([key, name]) => ({ id: `c:${key}`, label: name, active: category === key })),
    ...(languages.length > 1 ? languages.map((l) => ({ id: `l:${l}`, label: languageLabel(l), active: language === l })) : []),
  ];

  return (
    <div className="space-y-4">
      <PageHeader title="Documents" summary="Brochures, datasheets and company documents from us, always the current version." />

      {docs.length > 0 && (
        <ListFrame>
          <FilterChips className="border-b-0" chips={chips}
            onToggle={(id) => {
              const v = id.slice(2);
              if (id.startsWith("c:")) setCategory((cur) => (cur === v ? "" : v));
              else setLanguage((cur) => (cur === v ? "" : v));
            }}
            onClear={() => { setCategory(""); setLanguage(""); }}>
            <Input type="search" leading={Search} value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search documents"
              aria-label="Search documents" autoComplete="off" className="md:w-56" />
          </FilterChips>
        </ListFrame>
      )}

      {loading ? (
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2" aria-busy="true">
          {[0, 1, 2, 3].map((i) => <Skeleton key={i} className="h-24" />)}
        </div>
      ) : shown.length === 0 ? (
        <Card>
          <EmptyState icon={FileText} title={failed ? "Documents could not be loaded" : docs.length ? "No documents match" : "No documents yet"}
            description={failed ? "Try again in a moment." : docs.length ? "Clear the filters or the search." : "We add brochures and datasheets here as they are published."} />
        </Card>
      ) : (
        <ul className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          {shown.map((d) => (
            <li key={d.id}>
              <DocumentCard doc={d} showAudiences={false}>
                <DownloadButton doc={d} fetch={() => partnerDocsApi.download(d.id)} />
              </DocumentCard>
            </li>
          ))}
        </ul>
      )}

      <p className="text-xs text-fg-muted">
        Each download is prepared for your company and carries a footer with your company name and a tracking ID. {WATERMARK_LIMIT}
      </p>
    </div>
  );
}
