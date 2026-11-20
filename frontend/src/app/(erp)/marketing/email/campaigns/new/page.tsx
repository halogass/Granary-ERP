"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { Eye, EyeOff, Send, Clock } from "lucide-react";
import { emailMarketingApi } from "@/lib/api";
import BlockEditor, { type Block } from "../../_components/BlockEditor";
import CampaignPreview from "../../_components/CampaignPreview";
import SenderSettings from "../../_components/SenderSettings";
import TokenInput from "../../_components/TokenInput";
import {
  Button, Card, CardBody, CardHeader, Checkbox, Field, FieldGrid, Input, PageHeader, Tabs, TabsContent, TabsList,
  TabsTrigger, controlClass,
} from "@/components/ui";
import { cn } from "@/lib/utils";

interface MailingList { id: string; name: string; subscriber_count: number }

export default function NewCampaignPage() {
  const router = useRouter();
  const [lists, setLists]       = useState<MailingList[]>([]);
  const [saving, setSaving]     = useState(false);
  const [sending, setSending]   = useState(false);
  const [preview, setPreview]   = useState(false);
  const [tab, setTab]           = useState<"content" | "settings">("content");
  const [blocks, setBlocks]     = useState<Block[]>([]);
  const [form, setForm] = useState({
    name: "", subject: "", from_name: "Your Company", from_email: "hello@example.com",
  });
  const [sender, setSender] = useState<{ sender_user_id: string | null; include_signature: boolean }>({
    sender_user_id: null, include_signature: true,
  });
  const [selectedLists, setSelectedLists] = useState<string[]>([]);
  const [scheduleMode, setScheduleMode]   = useState(false);
  const [scheduledAt, setScheduledAt]     = useState("");

  useEffect(() => {
    fetchAllPages<MailingList>(emailMarketingApi.listLists).then(({ rows }) => setLists(rows));
  }, []);

  function toggleList(id: string) {
    setSelectedLists((prev) => prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]);
  }

  async function saveDraft() {
    if (!form.name || !form.subject) return;
    setSaving(true);
    await emailMarketingApi.createCampaign({
      ...form, ...sender, blocks, list_ids: selectedLists,
    });
    setSaving(false);
    router.push("/marketing/email/campaigns");
  }

  async function dispatch(sendNow: boolean) {
    if (!form.name || !form.subject || selectedLists.length === 0 || blocks.length === 0) {
      alert("Please fill in all fields, select at least one list, and add content blocks.");
      return;
    }
    setSending(true);
    const r = await emailMarketingApi.createCampaign({ ...form, ...sender, blocks, list_ids: selectedLists });
    const campaignId = r.data.id;
    await emailMarketingApi.dispatch(campaignId, {
      send_now: sendNow,
      scheduled_at: !sendNow && scheduledAt ? new Date(scheduledAt).toISOString() : undefined,
    });
    setSending(false);
    router.push("/marketing/email/campaigns");
  }

  return (
    <div className="space-y-4">
      <PageHeader title="New email campaign" summary="Build your campaign using the block editor"
        back={{ href: "/marketing/email/campaigns", label: "Email campaigns" }}
        actions={<>
          <Button onClick={() => setPreview(!preview)}>
            {preview ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
            {preview ? "Hide preview" : "Preview"}
          </Button>
          <Button onClick={saveDraft} loading={saving} disabled={!form.name || !form.subject}>Save draft</Button>
          <Button onClick={() => setScheduleMode(!scheduleMode)} aria-expanded={scheduleMode}><Clock aria-hidden /> Schedule</Button>
          <Button variant="primary" onClick={() => dispatch(true)} loading={sending}>{!sending && <Send aria-hidden />} Send now</Button>
        </>} />

      {scheduleMode && (
        <Card className="flex flex-wrap items-end gap-4 p-4">
          <Field label="Schedule send time" className="min-w-[220px] flex-1">
            <Input type="datetime-local" value={scheduledAt} onChange={(e) => setScheduledAt(e.target.value)} className="max-w-xs" />
          </Field>
          <Button variant="primary" onClick={() => dispatch(false)} loading={sending} disabled={!scheduledAt}>
            <Clock aria-hidden /> Schedule campaign
          </Button>
        </Card>
      )}

      <div className={cn("grid grid-cols-1 gap-4", preview && "xl:grid-cols-2")}>
        <Tabs value={tab} onValueChange={(v) => setTab(v as "content" | "settings")} className="space-y-4">
          <TabsList>
            <TabsTrigger value="content">Content</TabsTrigger>
            <TabsTrigger value="settings">Settings</TabsTrigger>
          </TabsList>

          <TabsContent value="content">
            <Card><CardBody><BlockEditor blocks={blocks} onChange={setBlocks} /></CardBody></Card>
          </TabsContent>

          <TabsContent value="settings" className="space-y-4">
            <Card>
              <CardHeader title="Campaign details" />
              <CardBody>
                <FieldGrid cols={2}>
                  <Field label="Campaign name" required className="md:col-span-2">
                    <Input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="e.g. May 2026 Newsletter" />
                  </Field>
                  <Field label="Email subject" required className="md:col-span-2">
                    <TokenInput className={`${controlClass} h-11 md:h-9`} value={form.subject} onChange={(subject) => setForm({ ...form, subject })}
                      placeholder="e.g. {{first_name}}, news from our team" />
                  </Field>
                  <Field label="From name">
                    <Input value={form.from_name} onChange={(e) => setForm({ ...form, from_name: e.target.value })} />
                  </Field>
                  <Field label="From email">
                    <Input value={form.from_email} onChange={(e) => setForm({ ...form, from_email: e.target.value })} />
                  </Field>
                </FieldGrid>
              </CardBody>
            </Card>

            <Card>
              <CardHeader title="Sender and signature" />
              <CardBody>
                <SenderSettings senderUserId={sender.sender_user_id} includeSignature={sender.include_signature}
                  onChange={(patch) => setSender((prev) => ({ ...prev, ...patch }))} />
              </CardBody>
            </Card>

            <Card>
              <CardHeader title="Target lists"
                description={selectedLists.length > 0 ? `${selectedLists.length} list(s) selected` : undefined} />
              <CardBody>
                {lists.length === 0 ? (
                  <p className="text-sm text-fg-muted">No mailing lists found. Create one first.</p>
                ) : (
                  <div className="space-y-2">
                    {lists.map((l) => (
                      <Checkbox key={l.id} checked={selectedLists.includes(l.id)} onChange={() => toggleList(l.id)}
                        className="flex w-full rounded-lg border border-line p-3 hover:bg-surface-2"
                        label={<span className="font-medium">{l.name}</span>}
                        hint={`${l.subscriber_count.toLocaleString()} subscribers`} />
                    ))}
                  </div>
                )}
              </CardBody>
            </Card>
          </TabsContent>
        </Tabs>

        {preview && (
          <CampaignPreview subject={form.subject} blocks={blocks} listIds={selectedLists}
            senderUserId={sender.sender_user_id} includeSignature={sender.include_signature} />
        )}
      </div>
    </div>
  );
}
