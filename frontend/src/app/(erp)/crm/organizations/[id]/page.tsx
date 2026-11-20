"use client";

import { useCallback, useEffect, useState } from "react";
import { Building2, Globe, Loader2, MapPin, Mail, Phone, Sparkles } from "lucide-react";
import { crmApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Button, Switch } from "@/components/ui";
import { usePermissions } from "@/hooks/usePermissions";
import RecordLayout, { type RecordShell } from "@/components/crm/RecordLayout";
import Fact from "@/components/crm/Fact";
import { ResearchButton, ResearchPanel, useResearch } from "@/components/crm/Research";

interface Org extends RecordShell {
  name: string;
  industry?: string | null;
  website?: string | null;
  country?: string | null;
  city?: string | null;
  email?: string | null;
  phone?: string | null;
  employee_count?: number | null;
  type?: string[];
  /** 2 Nov 2026: quoted without tax: quotations, invoices and proformas start with no tax. */
  quote_without_tax?: boolean;
}

export default function OrganizationRecordPage({ params }: { params: { id: string } }) {
  const id = params.id;
  const [org, setOrg] = useState<Org | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [enriching, setEnriching] = useState(false);
  const [enrichNote, setEnrichNote] = useState("");

  const load = useCallback(async () => {
    try {
      const r = await crmApi.getOrg(id);
      const d = r.data;
      setOrg({ ...d, id: d.id, title: d.name, subtitle: d.industry || null, custom: d.custom ?? {} });
    } catch (e) {
      setError(extractApiError(e, "Organization not found."));
    } finally { setLoading(false); }
  }, [id]);

  useEffect(() => { load(); }, [load]);

  // An empty patch is a plain refetch: RecordLayout uses it after an AI run.
  const onPatch = useCallback(async (patch: Record<string, unknown>) => {
    if (Object.keys(patch).length > 0) await crmApi.updateOrg(id, patch);
    await load();
  }, [id, load]);

  const research = useResearch("organization", id, load);
  const { can } = usePermissions();
  const [taxSaving, setTaxSaving] = useState(false);
  const [taxError, setTaxError] = useState("");

  async function setWithoutTax(on: boolean) {
    setTaxSaving(true); setTaxError("");
    try {
      await crmApi.updateOrg(id, { quote_without_tax: on });
      setOrg((o) => (o ? { ...o, quote_without_tax: on } : o));
    } catch (e) {
      setTaxError(extractApiError(e, "Could not change it."));
    } finally { setTaxSaving(false); }
  }

  /** Quick pass: one search plus the homepage, fills whatever is still empty. */
  async function enrich() {
    setEnriching(true);
    setEnrichNote("");
    try {
      const r = await crmApi.enrichOrg(id);
      const written = Object.keys(r.data.written ?? {});
      const suggested = Object.keys(r.data.suggested ?? {});
      setEnrichNote(written.length
        ? `Filled ${written.join(", ")}.${suggested.length ? ` Low confidence, not written: ${suggested.join(", ")}.` : ""}`
        : "Nothing new was found for the empty fields.");
      await load();
    } catch (e) {
      setEnrichNote(extractApiError(e, "Could not research this company."));
    } finally {
      setEnriching(false);
    }
  }

  return (
    <RecordLayout
      objectType="organization" recordId={id} record={org}
      backHref="/crm/organizations" backLabel="Organizations"
      loading={loading} error={error} onPatch={onPatch}
      actions={org && (
        <>
          <Button onClick={enrich} disabled={enriching}
            title="One search plus the homepage: fills empty fields in a few seconds">
            {enriching ? <Loader2 className="animate-spin" aria-hidden /> : <Sparkles aria-hidden />}
            {enriching ? "Enriching…" : "Quick enrich"}
          </Button>
          <ResearchButton state={research} label="Deep research" />
        </>
      )}
      aside={<ResearchPanel state={research} objectType="organization" />}
      facts={org && (
        <>
          {org.type && org.type.length > 0 && (
            <p className="mb-1 text-xs capitalize text-fg-muted">{org.type.join(" · ")}</p>
          )}
          <Fact icon={Globe} label="Website" value={org.website} href={org.website ?? undefined} />
          <Fact icon={MapPin} label="Location" value={[org.city, org.country].filter(Boolean).join(", ")} />
          <Fact icon={Mail} label="Email" value={org.email} href={org.email ? `mailto:${org.email}` : undefined} />
          <Fact icon={Phone} label="Phone" value={org.phone} />
          <Fact icon={Building2} label="Employees" value={org.employee_count?.toString()} />
          <div className="mt-2">
            <Switch checked={!!org.quote_without_tax} onCheckedChange={setWithoutTax}
              disabled={taxSaving || !can("crm:edit_contact")} label="Quote without tax" />
            <p className="mt-0.5 text-xs text-fg-muted">Quotations, invoices and proformas for this customer start with no tax; a line can still get one.</p>
            {taxError && <p role="alert" className="mt-1 text-xs font-medium text-tone-red">{taxError}</p>}
          </div>
          {enrichNote && <p role="status" className="mt-3 text-xs leading-relaxed text-fg-muted">{enrichNote}</p>}
        </>
      )}
    />
  );
}
