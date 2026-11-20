"use client";

import { useCallback, useEffect, useState } from "react";
import Link from "next/link";
import { UserCircle2, Mail, Phone, MessageCircle, Briefcase, Building2, Linkedin } from "lucide-react";
import { crmApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import RecordLayout, { type RecordShell } from "@/components/crm/RecordLayout";
import Fact from "@/components/crm/Fact";
import { ResearchButton, ResearchPanel, useResearch } from "@/components/crm/Research";
import AttachmentPanel from "@/components/ui/AttachmentPanel";
import { Card, CardBody, CardHeader } from "@/components/ui";

interface Contact extends RecordShell {
  first_name: string;
  last_name?: string | null;
  email?: string | null;
  phone?: string | null;
  whatsapp?: string | null;
  position?: string | null;
  department?: string | null;
  linkedin_url?: string | null;
  organization_id?: string | null;
}

export default function ContactRecordPage({ params }: { params: { id: string } }) {
  const id = params.id;
  const [contact, setContact] = useState<Contact | null>(null);
  const [orgName, setOrgName] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  const load = useCallback(async () => {
    try {
      const r = await crmApi.getContact(id);
      const d = r.data;
      const name = `${d.first_name} ${d.last_name || ""}`.trim();
      setContact({ ...d, id: d.id, title: name, subtitle: d.position || null, custom: d.custom ?? {} });
      if (d.organization_id) {
        crmApi.getOrg(d.organization_id).then((o) => setOrgName(o.data?.name ?? null)).catch(() => {});
      } else {
        setOrgName(null);
      }
    } catch (e) {
      setError(extractApiError(e, "Contact not found."));
    } finally { setLoading(false); }
  }, [id]);

  useEffect(() => { load(); }, [load]);

  const onPatch = useCallback(async (patch: Record<string, unknown>) => {
    if (Object.keys(patch).length > 0) await crmApi.updateContact(id, patch);
    await load();
  }, [id, load]);

  const research = useResearch("contact", id, load);

  return (
    <RecordLayout
      objectType="contact" recordId={id} record={contact}
      actions={contact && <ResearchButton state={research} label="Research this contact" />}
      aside={<ResearchPanel state={research} objectType="contact" />}
      backHref="/crm/contacts" backLabel="Contacts"
      loading={loading} error={error} onPatch={onPatch}
      facts={contact && (
        <>
          {contact.organization_id && orgName && (
            <div className="flex items-start gap-2.5 py-1.5">
              <Building2 className="mt-0.5 size-3.5 shrink-0 text-fg-subtle" aria-hidden />
              <div className="min-w-0 flex-1">
                <p className="text-xs text-fg-muted">Organization</p>
                <Link href={`/crm/organizations/${contact.organization_id}`}
                  className="block truncate text-sm text-brand-ink hover:underline">{orgName}</Link>
              </div>
            </div>
          )}
          <Fact icon={Briefcase} label="Position" value={contact.position} />
          <Fact icon={UserCircle2} label="Department" value={contact.department} />
          <Fact icon={Mail} label="Email" value={contact.email} href={contact.email ? `mailto:${contact.email}` : undefined} />
          <Fact icon={Phone} label="Phone" value={contact.phone} />
          <Fact icon={MessageCircle} label="WhatsApp" value={contact.whatsapp} />
          <Fact icon={Linkedin} label="LinkedIn" value={contact.linkedin_url} href={contact.linkedin_url ?? undefined} />
        </>
      )}
    >
      {/* 29 Oct 2026: a contact's own files (also what your AI client or Nary stored with upload_file) */}
      <Card>
        <CardHeader title="Attachments" />
        <CardBody><AttachmentPanel entityType="crm_contact" entityId={id} /></CardBody>
      </Card>
    </RecordLayout>
  );
}
