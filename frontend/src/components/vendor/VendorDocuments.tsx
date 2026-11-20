"use client";

import { useCallback, useEffect, useState } from "react";
import { Download, ShieldCheck } from "lucide-react";
import { vendorApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { downloadStamped } from "@/lib/documentLibrary";
import { Button, Pill, Skeleton, toast } from "@/components/ui";

// Shape of GET /vendor/vendors/{id}/documents.
interface VendorDocument {
  id: string;
  document_type: string;
  name: string;
  signed_at: string | null;
  expires_at: string | null;
  is_verified: boolean;
  notes: string | null;
  created_at: string;
}

const fmt = (d: string) => new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });

/** The documents a vendor uploaded in the vendor portal, for staff who manage vendors to check and verify. */
export default function VendorDocuments({ vendorId }: { vendorId: string }) {
  const [docs, setDocs] = useState<VendorDocument[] | null>(null);
  const [error, setError] = useState("");
  const [busyId, setBusyId] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const res = await vendorApi.listVendorDocuments(vendorId);
      setDocs(res.data as VendorDocument[]);
      setError("");
    } catch (e) { setError(extractApiError(e, "The documents could not be loaded")); }
  }, [vendorId]);

  useEffect(() => { load(); }, [load]);

  async function verify(doc: VendorDocument) {
    setBusyId(doc.id);
    try {
      await vendorApi.verifyVendorDocument(doc.id);
      toast.success(`${doc.name} verified`);
      setDocs((cur) => cur?.map((d) => (d.id === doc.id ? { ...d, is_verified: true } : d)) ?? cur);
    } catch (e) {
      toast.error(extractApiError(e, "The document could not be verified"));
    } finally { setBusyId(null); }
  }

  async function download(doc: VendorDocument) {
    try {
      await downloadStamped(() => vendorApi.downloadVendorDocument(doc.id), doc.name);
    } catch { toast.error("The file could not be downloaded."); }
  }

  return (
    <section aria-label="Vendor documents" className="space-y-2">
      <h4 className="text-[13px] font-semibold text-fg">Documents</h4>
      {error && <p className="text-[13px] text-tone-red">{error}</p>}
      {!docs && !error && <Skeleton className="h-10" />}
      {docs && docs.length === 0 && <p className="text-[13px] text-fg-muted">No documents uploaded in the vendor portal yet.</p>}
      {docs && docs.length > 0 && (
        <ul className="divide-y divide-line rounded-lg border border-line">
          {docs.map((d) => {
            const expired = !!d.expires_at && new Date(d.expires_at).getTime() < Date.now();
            return (
              <li key={d.id} className="flex flex-wrap items-center gap-2 px-3 py-2">
                <div className="min-w-0 flex-1">
                  <p className="truncate text-sm font-medium text-fg">{d.name}</p>
                  <p className="text-xs capitalize text-fg-muted">
                    {d.document_type.replace(/_/g, " ")}
                    {d.expires_at ? `, expires ${fmt(d.expires_at)}` : ""}
                  </p>
                </div>
                {expired && <Pill tone="red">Expired</Pill>}
                {d.is_verified ? <Pill tone="blue" icon={ShieldCheck}>Verified</Pill> : <Pill tone="orange">Not verified</Pill>}
                <Button size="sm" variant="ghost" onClick={() => download(d)} aria-label={`Download ${d.name}`}>
                  <Download aria-hidden /> Download
                </Button>
                {!d.is_verified && (
                  <Button size="sm" loading={busyId === d.id} onClick={() => verify(d)} aria-label={`Verify ${d.name}`}>
                    <ShieldCheck aria-hidden /> Verify
                  </Button>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
