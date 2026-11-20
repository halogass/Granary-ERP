"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { Plus } from "lucide-react";
import { Button, Skeleton } from "@/components/ui";
import { usePermissions } from "@/hooks/usePermissions";
import { docLibApi, itemsOf, type LibraryDocument } from "@/lib/documentLibrary";
import { DocumentCard, DownloadButton } from "./DocumentCards";
import { NewDocumentDialog, useDocCategories } from "./DocumentDialogs";

/**
 * "Documents for this product" on a catalog item: library documents linked to the product or its
 * family (datasheets, brochures). Replaces the item's own file list; Nary sends from the library.
 */
export default function ProductDocumentsPanel({ product }: { product: { id: string; name: string; sku: string; family?: string | null } }) {
  const { can } = usePermissions();
  const canCreate = can("documents:create");
  const [docs, setDocs] = useState<LibraryDocument[] | null>(null);
  const [creating, setCreating] = useState(false);
  const categories = useDocCategories();

  useEffect(() => {
    let alive = true;
    docLibApi.catalogDocuments(product.id)
      .then((r) => { if (alive) setDocs(itemsOf(r.data)); })
      .catch(() => { if (alive) setDocs([]); });
    return () => { alive = false; };
  }, [product.id]);

  return (
    <section aria-label="Documents for this product" className="max-w-3xl">
      <div className="mb-2 flex flex-wrap items-center gap-2">
        <h3 className="text-xs font-semibold uppercase tracking-wide text-fg-muted">Documents for this product</h3>
        <div className="ml-auto flex gap-2">
          <Button size="sm" variant="ghost" asChild><Link href={`/knowledge/documents?product=${product.id}`}>Open the library</Link></Button>
          {canCreate && <Button size="sm" onClick={() => setCreating(true)}><Plus aria-hidden /> Add a document</Button>}
        </div>
      </div>
      {docs === null ? (
        <Skeleton className="h-16 w-full" />
      ) : docs.length === 0 ? (
        <p className="text-[13px] text-fg-muted">No datasheet or brochure is linked to this product or its family yet.</p>
      ) : (
        <ul className="grid grid-cols-1 gap-2 sm:grid-cols-2">
          {docs.map((d) => (
            <li key={d.id}>
              <DocumentCard doc={d} href={`/knowledge/documents/${d.id}`}><DownloadButton doc={d} /></DocumentCard>
            </li>
          ))}
        </ul>
      )}
      {creating && (
        <NewDocumentDialog open onOpenChange={setCreating} categories={categories}
          defaults={{ category: "products", audiences: ["customer"], products: [{ id: product.id, name: product.name, sku: product.sku }] }} />
      )}
    </section>
  );
}
