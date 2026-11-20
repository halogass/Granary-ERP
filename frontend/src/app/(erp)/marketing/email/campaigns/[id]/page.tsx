"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useState } from "react";
import { useParams, useRouter } from "next/navigation";
import Link from "next/link";
import { Eye, EyeOff, Send, Clock, BarChart2, Loader2 } from "lucide-react";
import { emailMarketingApi } from "@/lib/api";
import BlockEditor, { type Block } from "../../_components/BlockEditor";
import CampaignPreview from "../../_components/CampaignPreview";
import SenderSettings from "../../_components/SenderSettings";
import TokenInput from "../../_components/TokenInput";
import {
  Button, Card, CardBody, CardHeader, Checkbox, EmptyState, Field, FieldGrid, Input, PageHeader, Stat, StatGroup,
  StatusPill, controlClass,
} from "@/components/ui";
import { cn } from "@/lib/utils";

interface Campaign {
  id: string; name: string; subject: string; from_name: string; from_email: string;
  blocks: Block[]; list_ids: string[]; status: string;
  sender_user_id: string | null; include_signature: boolean;
  total_recipients: number; sent_count: number; open_count: number; failed_count: number;
  scheduled_at?: string; sent_at?: string;
}
interface MailingList { id: string; name: string; subscriber_count: number }

export default function CampaignDetailPage() {
  const { id } = useParams<{ id: string }>();
  const router  = useRouter();
  const [campaign, setCampaign] = useState<Campaign | null>(null);
  const [lists, setLists]       = useState<MailingList[]>([]);
  const [loading, setLoading]   = useState(true);
  const [saving, setSaving]     = useState(false);
  const [sending, setSending]   = useState(false);
  const [preview, setPreview]   = useState(false);
  const [blocks, setBlocks]     = useState<Block[]>([]);
  const [form, setForm]         = useState({ name: "", subject: "", from_name: "", from_email: "" });
  const [sender, setSender] = useState<{ sender_user_id: string | null; include_signature: boolean }>({
    sender_user_id: null, include_signature: true,
  });
  const [selectedLists, setSelectedLists] = useState<string[]>([]);
  const [scheduledAt, setScheduledAt]     = useState("");
  const [scheduleMode, setScheduleMode]   = useState(false);

  async function load() {
    setLoading(true);
    const [cr, lr] = await Promise.all([
      emailMarketingApi.getCampaign(id),
      fetchAllPages<MailingList>(emailMarketingApi.listLists),
    ]);
    const c = cr.data as Campaign;
    setCampaign(c);
    setLists(lr.rows);
    setBlocks(c.blocks || []);
    setForm({ name: c.name, subject: c.subject, from_name: c.from_name, from_email: c.from_email });
    setSender({ sender_user_id: c.sender_user_id, include_signature: c.include_signature ?? true });
    setSelectedLists(c.list_ids || []);
    setLoading(false);
  }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { load(); }, [id]);

  async function save() {
    setSaving(true);
    await emailMarketingApi.updateCampaign(id, { ...form, ...sender, blocks, list_ids: selectedLists });
    setSaving(false);
    load();
  }

  async function dispatch(sendNow: boolean) {
    if (selectedLists.length === 0 || blocks.length === 0) {
      alert("Add at least one list and one content block before dispatching.");
      return;
    }
    setSending(true);
    await emailMarketingApi.updateCampaign(id, { ...form, ...sender, blocks, list_ids: selectedLists });
    await emailMarketingApi.dispatch(id, {
      send_now: sendNow,
      scheduled_at: !sendNow && scheduledAt ? new Date(scheduledAt).toISOString() : undefined,
    });
    setSending(false);
    router.push("/marketing/email/campaigns");
  }

  function toggleList(lid: string) {
    setSelectedLists((prev) => prev.includes(lid) ? prev.filter((x) => x !== lid) : [...prev, lid]);
  }

  if (loading) return <div className="flex justify-center py-20"><Loader2 className="size-6 animate-spin text-fg-muted" aria-label="Loading" /></div>;
  if (!campaign) return <EmptyState title="Campaign not found." />;

  const isDraft    = campaign.status === "draft";
  const isSent     = campaign.status === "sent";
  const openRate   = campaign.sent_count > 0 ? Math.round((campaign.open_count / campaign.sent_count) * 100) : 0;
  const tokenCls   = `${controlClass} h-11 md:h-9`;

  return (
    <div className="space-y-4">
      <PageHeader title={campaign.name} summary={campaign.subject}
        back={{ href: "/marketing/email/campaigns", label: "Email campaigns" }}
        status={<StatusPill family="email_campaign" status={campaign.status} />}
        actions={<>
          {isSent && (
            <Button asChild><Link href={`/marketing/email/campaigns/${id}/report`}><BarChart2 aria-hidden /> View report</Link></Button>
          )}
          {isDraft && (
            <>
              <Button onClick={() => setPreview(!preview)}>
                {preview ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
                {preview ? "Hide preview" : "Preview"}
              </Button>
              <Button onClick={save} loading={saving}>Save</Button>
              <Button onClick={() => setScheduleMode(!scheduleMode)} aria-expanded={scheduleMode}><Clock aria-hidden /> Schedule</Button>
              <Button variant="primary" onClick={() => dispatch(true)} loading={sending}>{!sending && <Send aria-hidden />} Send now</Button>
            </>
          )}
        </>} />

      {!isDraft && (
        <StatGroup>
          <Stat label="Recipients" value={campaign.total_recipients.toLocaleString()} />
          <Stat label="Sent" value={campaign.sent_count.toLocaleString()} />
          <Stat label="Failed" value={campaign.failed_count.toLocaleString()} alert={campaign.failed_count > 0} />
          <Stat label="Open rate" value={isSent ? `${openRate}%` : "-"} />
        </StatGroup>
      )}

      {scheduleMode && isDraft && (
        <Card className="flex flex-wrap items-end gap-4 p-4">
          <Field label="Schedule send time" className="min-w-[220px] flex-1">
            <Input type="datetime-local" value={scheduledAt} onChange={(e) => setScheduledAt(e.target.value)} className="max-w-xs" />
          </Field>
          <Button variant="primary" onClick={() => dispatch(false)} loading={sending} disabled={!scheduledAt}>
            <Clock aria-hidden /> Schedule
          </Button>
        </Card>
      )}

      <div className={cn("grid grid-cols-1 gap-4", preview && isDraft && "xl:grid-cols-2")}>
        <div className="space-y-4">
          <Card>
            <CardHeader title="Campaign settings" />
            <CardBody className="space-y-5">
              <FieldGrid cols={2}>
                <Field label="Campaign name">
                  <Input value={form.name} disabled={!isDraft} onChange={(e) => setForm({ ...form, name: e.target.value })} />
                </Field>
                <Field label="Subject">
                  <TokenInput className={tokenCls} value={form.subject} disabled={!isDraft} onChange={(subject) => setForm({ ...form, subject })} />
                </Field>
                <Field label="From name">
                  <Input value={form.from_name} disabled={!isDraft} onChange={(e) => setForm({ ...form, from_name: e.target.value })} />
                </Field>
                <Field label="From email">
                  <Input value={form.from_email} disabled={!isDraft} onChange={(e) => setForm({ ...form, from_email: e.target.value })} />
                </Field>
              </FieldGrid>

              <div>
                <h3 className="mb-2 text-[13px] font-medium text-fg">Sender and signature</h3>
                <SenderSettings senderUserId={sender.sender_user_id} includeSignature={sender.include_signature}
                  disabled={!isDraft} onChange={(patch) => setSender((prev) => ({ ...prev, ...patch }))} />
              </div>

              <div>
                <h3 className="mb-2 text-[13px] font-medium text-fg">Target lists</h3>
                <div className="space-y-2">
                  {lists.map((l) => (
                    <Checkbox key={l.id} disabled={!isDraft} checked={selectedLists.includes(l.id)} onChange={() => isDraft && toggleList(l.id)}
                      className={cn("flex w-full rounded-lg border border-line p-3", isDraft ? "hover:bg-surface-2" : "opacity-70")}
                      label={<span className="flex w-full gap-3"><span className="font-medium">{l.name}</span><span className="ml-auto text-xs text-fg-muted tabular">{l.subscriber_count.toLocaleString()} subs</span></span>} />
                  ))}
                </div>
              </div>
            </CardBody>
          </Card>

          <Card>
            <CardHeader title="Email content" />
            <CardBody>
              {isDraft
                ? <BlockEditor blocks={blocks} onChange={setBlocks} />
                : <p className="text-sm text-fg-muted">Content is read-only after sending.</p>}
            </CardBody>
          </Card>
        </div>

        {preview && isDraft && (
          <CampaignPreview subject={form.subject} blocks={blocks} listIds={selectedLists}
            senderUserId={sender.sender_user_id} includeSignature={sender.include_signature} />
        )}
      </div>
    </div>
  );
}
