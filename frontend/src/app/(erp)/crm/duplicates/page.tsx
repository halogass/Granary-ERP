"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useCallback, useEffect, useState } from "react";
import { Copy, Search, X, ArrowRight, AlertTriangle } from "lucide-react";
import { crmRecordsApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Button, Card, CardBody, Dialog, EmptyState, PageHeader, Pill, Skeleton, Tabs, TabsList, TabsTrigger } from "@/components/ui";

interface Side { id: string; label: string; email?: string | null; phone?: string | null }
interface Candidate {
  id: string; object_type: string; score: number; reasons: string[]; a: Side; b: Side;
}
interface PreviewField {
  field: string; keep: unknown; merge: unknown; result: unknown;
  taken_from: "keep" | "merge" | "empty";
}
interface Preview {
  keep: { id: string; label: string };
  merge: { id: string; label: string };
  fields: PreviewField[];
  moves: { what: string; count: number }[];
}

const TYPES = [
  { id: "organization", label: "Organizations" },
  { id: "contact", label: "Contacts" },
] as const;

function fmt(v: unknown): string {
  if (v === null || v === undefined || v === "") return "-";
  if (Array.isArray(v)) return v.join(", ");
  if (typeof v === "object") return JSON.stringify(v);
  return String(v);
}

export default function DuplicatesPage() {
  const [objectType, setObjectType] = useState<string>("organization");
  const [items, setItems] = useState<Candidate[]>([]);
  const [loading, setLoading] = useState(true);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState("");

  const [preview, setPreview] = useState<Preview | null>(null);
  const [previewFor, setPreviewFor] = useState<Candidate | null>(null);
  const [swapped, setSwapped] = useState(false);
  const [merging, setMerging] = useState(false);

  const load = useCallback(async () => {
    setLoading(true); setError("");
    try {
      // The duplicates list has no offset: one page as large as it gets.
      const { rows } = await fetchAllPages<Candidate>(crmRecordsApi.listDuplicates, { object_type: objectType, status: "open" }, 1000);
      setItems(rows);
    } catch (e) {
      setError(extractApiError(e, "Could not load duplicates."));
    } finally { setLoading(false); }
  }, [objectType]);

  useEffect(() => { load(); }, [load]);

  async function scan() {
    setScanning(true); setError("");
    try {
      await crmRecordsApi.scanDuplicates(objectType);
      await load();
    } catch (e) {
      setError(extractApiError(e, "The scan failed."));
    } finally { setScanning(false); }
  }

  async function openPreview(c: Candidate, flip = false) {
    setPreviewFor(c); setSwapped(flip); setPreview(null); setError("");
    const keep = flip ? c.b.id : c.a.id;
    const merge = flip ? c.a.id : c.b.id;
    try {
      const r = await crmRecordsApi.mergePreview({
        object_type: c.object_type, keep_id: keep, merge_id: merge });
      setPreview(r.data);
    } catch (e) {
      setError(extractApiError(e, "Could not build the merge preview."));
      setPreviewFor(null);
    }
  }

  async function confirmMerge() {
    if (!preview || !previewFor) return;
    setMerging(true); setError("");
    try {
      await crmRecordsApi.mergeRecords({
        object_type: previewFor.object_type,
        keep_id: preview.keep.id, merge_id: preview.merge.id,
        candidate_id: previewFor.id,
      });
      setPreviewFor(null); setPreview(null);
      await load();
    } catch (e) {
      setError(extractApiError(e, "The merge failed."));
    } finally { setMerging(false); }
  }

  async function dismiss(c: Candidate) {
    try {
      await crmRecordsApi.dismissDuplicate(c.id);
      setItems((prev) => prev.filter((x) => x.id !== c.id));
    } catch (e) {
      setError(extractApiError(e, "Could not dismiss."));
    }
  }

  const closePreview = () => { setPreviewFor(null); setPreview(null); };

  return (
    <div className="max-w-5xl">
      <PageHeader title="Duplicates"
        summary="Suggested duplicate records. Nothing is merged until you confirm it."
        actions={<Button variant="primary" onClick={scan} loading={scanning}>{!scanning && <Search aria-hidden />} Scan for duplicates</Button>} />

      <Tabs value={objectType} onValueChange={setObjectType} className="mb-4">
        <TabsList aria-label="Record type">
          {TYPES.map((t) => <TabsTrigger key={t.id} value={t.id}>{t.label}</TabsTrigger>)}
        </TabsList>
      </Tabs>

      {error && (
        <div role="alert" className="mb-4 rounded-lg border border-tone-red/30 bg-tone-red-soft px-4 py-3 text-sm text-tone-red">{error}</div>
      )}

      {loading ? (
        <div className="space-y-3" aria-busy>
          {[0, 1].map((i) => <Skeleton key={i} className="h-36 w-full rounded-lg" />)}
        </div>
      ) : items.length === 0 ? (
        <Card>
          <EmptyState icon={Copy} title="No open duplicates" description="Run a scan to look for new ones." />
        </Card>
      ) : (
        <div className="space-y-3">
          {items.map((c) => (
            <Card key={c.id}>
              <CardBody>
                <div className="mb-3 flex flex-wrap items-center gap-2">
                  <Pill tone="orange" className="tabular">{Math.round(c.score)}% match</Pill>
                  <span className="text-xs text-fg-muted">{c.reasons.join(", ")}</span>
                </div>

                <div className="grid gap-3 sm:grid-cols-2">
                  {[c.a, c.b].map((side, i) => (
                    <div key={side.id} className="rounded-lg border border-line bg-surface-2 p-3">
                      <p className="truncate text-sm font-medium text-fg">{side.label}</p>
                      <p className="truncate text-xs text-fg-muted">{side.email || "-"}</p>
                      <p className="truncate text-xs text-fg-muted tabular">{side.phone || "-"}</p>
                      <Button size="sm" variant="ghost" className="mt-2 -ml-2.5 text-brand-ink" onClick={() => openPreview(c, i === 1)}>
                        Keep this one <ArrowRight aria-hidden />
                      </Button>
                    </div>
                  ))}
                </div>

                <div className="mt-3 flex justify-end">
                  <Button size="sm" variant="ghost" onClick={() => dismiss(c)}>
                    <X aria-hidden /> Not a duplicate
                  </Button>
                </div>
              </CardBody>
            </Card>
          ))}
        </div>
      )}

      <Dialog open={!!previewFor} onOpenChange={(o) => { if (!o) closePreview(); }} size="lg"
        title="Confirm merge" description="This cannot be undone."
        footer={preview && previewFor ? <>
          <Button variant="ghost" className="mr-auto" onClick={() => openPreview(previewFor, !swapped)}>
            <ArrowRight aria-hidden /> Swap which one survives
          </Button>
          <Button onClick={closePreview}>Cancel</Button>
          <Button variant="primary" onClick={confirmMerge} loading={merging}>Merge permanently</Button>
        </> : undefined}>
        {!preview ? (
          <p className="py-8 text-sm text-fg-muted">Building preview</p>
        ) : (
          <>
            <div className="mb-4 flex items-center gap-2 rounded-lg border border-tone-orange/30 bg-tone-orange-soft px-4 py-3">
              <AlertTriangle className="size-4 shrink-0 text-tone-orange" aria-hidden />
              <p className="text-[13px] text-fg">
                <strong className="font-semibold">{preview.merge.label}</strong> will be archived and everything attached to it
                moves to <strong className="font-semibold">{preview.keep.label}</strong>.
              </p>
            </div>

            {preview.moves.length > 0 && (
              <div className="mb-4">
                <p className="mb-1.5 text-sm font-semibold text-fg">What moves</p>
                <div className="flex flex-wrap gap-1.5">
                  {preview.moves.map((m) => (
                    <Pill key={m.what} className="tabular">{m.count} {m.what}</Pill>
                  ))}
                </div>
              </div>
            )}

            <p className="mb-1.5 text-sm font-semibold text-fg">Resulting fields</p>
            <table className="w-full text-[13px]">
              <tbody>
                {preview.fields.filter((f) => f.taken_from !== "empty").map((f) => (
                  <tr key={f.field} className="border-b border-line-soft last:border-0">
                    <th scope="row" className="py-1.5 pr-3 text-left align-top font-medium capitalize text-fg-muted">
                      {f.field.replace(/_/g, " ")}
                    </th>
                    <td className="py-1.5">
                      <span className="text-fg">{fmt(f.result)}</span>
                      {f.taken_from === "merge" && (
                        <Pill tone="blue" className="ml-1.5">filled from the archived record</Pill>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </>
        )}
      </Dialog>
    </div>
  );
}
