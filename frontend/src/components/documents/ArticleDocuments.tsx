"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { FileText, Link2, X } from "lucide-react";
import { Button, Card, CardBody, CardHeader, Combobox, IconButton, toast } from "@/components/ui";
import type { Option, OptionSource } from "@/components/ui/SearchSelect";
import { extractApiError } from "@/lib/apiError";
import { docLibApi, itemsOf, languageLabel, type LibraryDocument } from "@/lib/documentLibrary";
import { DocumentCard, DownloadButton } from "./DocumentCards";

function useArticleDocuments(articleId: string) {
  const [docs, setDocs] = useState<LibraryDocument[]>([]);
  const [loaded, setLoaded] = useState(false);
  const load = useCallback(async () => {
    try {
      const r = await docLibApi.articleDocuments(articleId);
      setDocs(itemsOf(r.data));
    } catch {
      setDocs([]);
    } finally {
      setLoaded(true);
    }
  }, [articleId]);
  useEffect(() => { void load(); }, [load]);
  return { docs, setDocs, loaded, reload: load };
}

/** The article page: the documents it embeds, as cards with the current version. */
export function ArticleDocumentCards({ articleId }: { articleId: string }) {
  const { docs } = useArticleDocuments(articleId);
  if (!docs.length) return null;
  return (
    <section aria-label="Documents" className="border-t border-line px-5 py-5 md:px-8">
      <h2 className="mb-3 text-sm font-semibold text-fg">Documents</h2>
      <ul className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        {docs.map((d) => (
          <li key={d.id}>
            <DocumentCard doc={d} href={`/knowledge/documents/${d.id}`} showAudiences={false}>
              <DownloadButton doc={d} />
            </DocumentCard>
          </li>
        ))}
      </ul>
    </section>
  );
}

const documentSource: OptionSource = {
  fetch: (q) => docLibApi.list({ view: "all", q: q || undefined, limit: 30 })
    .then((r) => itemsOf(r.data).map((d): Option => ({
      id: d.id, label: d.title,
      sublabel: [d.current_version?.label, languageLabel(d.language), d.category?.name].filter(Boolean).join(" · "),
    })))
    .catch(() => []),
};

/** The article editor: "Link a document" (search and pick); saved at once. */
export function ArticleDocumentLinks({ articleId }: { articleId: string }) {
  const { docs, setDocs, loaded } = useArticleDocuments(articleId);
  const [saving, setSaving] = useState(false);
  const linkedIds = useMemo(() => docs.map((d) => d.id), [docs]);

  async function save(next: LibraryDocument[], done: string) {
    setSaving(true);
    try {
      await docLibApi.setArticleDocuments(articleId, next.map((d) => d.id));
      setDocs(next);
      toast.success(done);
    } catch (e) {
      toast.error(extractApiError(e, "The documents could not be saved."));
    } finally {
      setSaving(false);
    }
  }

  async function add(id: string) {
    if (!id || linkedIds.includes(id)) return;
    try {
      const r = await docLibApi.get(id);
      await save([...docs, r.data], "Document linked");
    } catch (e) {
      toast.error(extractApiError(e, "The document could not be linked."));
    }
  }

  return (
    <Card>
      <CardHeader title={<span className="inline-flex items-center gap-2"><Link2 className="size-4 text-fg-muted" aria-hidden /> Linked documents</span>}
        description="Shown as cards under the article, always with the current version." />
      <CardBody className="space-y-3">
        {loaded && docs.length === 0 && <p className="text-xs text-fg-muted">No documents linked yet.</p>}
        {docs.length > 0 && (
          <ul className="space-y-1.5">
            {docs.map((d) => (
              <li key={d.id} className="flex items-center gap-2 rounded-lg bg-surface-2 px-3 py-1.5">
                <FileText className="size-4 shrink-0 text-fg-muted" aria-hidden />
                <span className="min-w-0 flex-1 truncate text-sm text-fg">{d.title}</span>
                <span className="shrink-0 font-mono text-xs text-fg-muted">{d.current_version?.label ?? ""}</span>
                <IconButton icon={X} size="sm" label={`Unlink ${d.title}`} disabled={saving}
                  onClick={() => void save(docs.filter((x) => x.id !== d.id), "Document unlinked")} />
              </li>
            ))}
          </ul>
        )}
        <div className="max-w-md">
          <Combobox value="" onChange={(id) => void add(id)} source={documentSource} excludeIds={linkedIds}
            placeholder="Link a document: search the library" disabled={saving} />
        </div>
        <Button asChild size="sm" variant="ghost"><a href="/knowledge/documents" target="_blank" rel="noopener noreferrer">Open the library</a></Button>
      </CardBody>
    </Card>
  );
}
