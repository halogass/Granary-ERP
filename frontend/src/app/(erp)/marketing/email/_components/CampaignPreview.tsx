"use client";

import { useCallback, useEffect, useState } from "react";
import { Monitor, RefreshCw, Smartphone } from "lucide-react";
import { emailMarketingApi, type CampaignPreview as Preview } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import type { Block } from "./BlockEditor";
import { Button, Card, IconButton, Select } from "@/components/ui";
import { cn } from "@/lib/utils";

interface Props {
  subject: string;
  blocks: Block[];
  listIds: string[];
  senderUserId: string | null;
  includeSignature: boolean;
}

interface ListContact { id: string; email: string; first_name?: string | null; last_name?: string | null }

/** The whole email exactly as it will be sent, rendered by the server, never sent. */
export default function CampaignPreview({ subject, blocks, listIds, senderUserId, includeSignature }: Props) {
  const [preview, setPreview] = useState<Preview | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [contacts, setContacts] = useState<ListContact[]>([]);
  const [contactId, setContactId] = useState<string>("");
  const [mobile, setMobile] = useState(false);

  const listKey = listIds.join(",");
  useEffect(() => {
    if (!listIds.length) { setContacts([]); setContactId(""); return; }
    Promise.all(listIds.map((id) =>
      emailMarketingApi.listContacts(id, { subscribed_only: true, limit: 25 }).then((r) => r.data as ListContact[])
        .catch(() => [] as ListContact[])))
      .then((groups) => {
        const seen = new Set<string>();
        setContacts(groups.flat().filter((c) => !seen.has(c.email) && seen.add(c.email)));
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [listKey]);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const r = await emailMarketingApi.preview({
        subject, blocks, list_ids: listIds, sender_user_id: senderUserId, include_signature: includeSignature,
        contact_id: contactId || null,
      });
      setPreview(r.data);
    } catch (e) {
      setError(extractApiError(e, "The preview could not be rendered."));
    } finally {
      setLoading(false);
    }
  }, [subject, blocks, listIds, senderUserId, includeSignature, contactId]);

  // Render on open and whenever the chosen recipient or sender changes; content edits refresh on demand.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { refresh(); }, [contactId, senderUserId, includeSignature, listKey]);

  const recipient = preview?.recipient;

  return (
    <Card className="space-y-3 p-4">
      <div className="flex flex-wrap items-center gap-1">
        <p className="flex-1 text-xs font-medium text-fg-muted">Preview, exactly as sent</p>
        <IconButton size="sm" icon={Monitor} label="Desktop width" aria-pressed={!mobile} onClick={() => setMobile(false)}
          className={cn(!mobile && "bg-surface-2 text-fg")} />
        <IconButton size="sm" icon={Smartphone} label="Phone width" aria-pressed={mobile} onClick={() => setMobile(true)}
          className={cn(mobile && "bg-surface-2 text-fg")} />
        <Button size="sm" onClick={refresh} loading={loading}>
          {!loading && <RefreshCw aria-hidden />} Refresh
        </Button>
      </div>

      <div className="flex flex-wrap items-center gap-2 text-xs">
        <span className="text-fg-muted">Preview as</span>
        <div className="min-w-0 flex-1">
          <Select aria-label="Preview as" value={contactId} onChange={(e) => setContactId(e.target.value)}
            placeholder={listIds.length ? "First subscriber of the selected lists" : "Sample recipient (Jane Doe, Acme Corp)"}
            options={contacts.map((c) => ({ value: c.id, label: `${[c.first_name, c.last_name].filter(Boolean).join(" ") || c.email}, ${c.email}` }))} />
        </div>
      </div>

      {error && <p className="text-xs text-tone-red">{error}</p>}

      {preview && (
        <>
          <div className="space-y-0.5 rounded-lg border border-line bg-surface-2 px-3 py-2 text-xs">
            <p><span className="text-fg-muted">To:</span> <span className="text-fg">{recipient?.email}</span>
              {recipient?.sample && <span className="text-fg-muted"> (sample)</span>}</p>
            <p><span className="text-fg-muted">Subject:</span> <span className="font-semibold text-fg">{preview.subject || "(no subject)"}</span></p>
          </div>
          <div className="flex justify-center overflow-x-auto rounded-lg bg-surface-2 p-2">
            {/* an email is always rendered on white, whatever the app theme */}
            <iframe title="Email preview" srcDoc={preview.html} sandbox=""
              className="rounded border-0 transition-all"
              style={{ width: mobile ? 375 : "100%", minWidth: mobile ? 375 : 0, height: 720, background: "white", colorScheme: "light" }} />
          </div>
          <p className="text-xs text-fg-muted">
            Links are inactive in the preview. Content edits appear after Refresh.
          </p>
        </>
      )}
    </Card>
  );
}
