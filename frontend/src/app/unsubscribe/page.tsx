"use client";

import { Suspense, useEffect, useState } from "react";
import { useSearchParams } from "next/navigation";
import { CheckCircle, XCircle, Loader2, MailCheck } from "lucide-react";
import { Button } from "@/components/ui";
import { PublicPage, PublicNotice } from "@/components/portal/PublicPage";

function UnsubscribeContent() {
  const params = useSearchParams();
  const token  = params.get("token") ?? "";

  const [state, setState] = useState<"loading" | "success" | "error" | "resubscribed">("loading");
  const [email, setEmail] = useState("");
  const [company, setCompany] = useState("");
  const [msg,   setMsg]   = useState("");

  useEffect(() => {
    if (!token) { setState("error"); setMsg("Invalid unsubscribe link."); return; }

    fetch(`/api/v1/marketing/email/unsubscribe/${token}`)
      .then((r) => r.json())
      .then((data) => {
        if (data.success) {
          setState("success");
          setEmail(data.email ?? "");
          setCompany(data.company ?? "");
        } else {
          setState("error");
          setMsg(data.message ?? "Something went wrong.");
        }
      })
      .catch(() => { setState("error"); setMsg("Network error. Please try again."); });
  }, [token]);

  function resubscribe() {
    fetch(`/api/v1/marketing/email/resubscribe/${token}`, { method: "POST" })
      .then((r) => r.json())
      .then(() => setState("resubscribed"))
      .catch(() => {});
  }

  if (state === "loading") return <Waiting text="Processing your request..." />;

  if (state === "success") return (
    <PublicNotice icon={CheckCircle} tone="blue" title="You have been unsubscribed"
      action={<div className="flex flex-col items-center gap-2"><p className="text-[13px] text-fg-muted">Changed your mind?</p><Button onClick={resubscribe}>Re-subscribe</Button></div>}>
      {email && (
        <p>
          <span className="font-medium text-fg">{email}</span> will no longer receive any marketing email
          {company ? ` from ${company}` : ""}.
        </p>
      )}
    </PublicNotice>
  );

  if (state === "resubscribed") return (
    <PublicNotice icon={MailCheck} tone="blue" title="You are back">
      <p>You have been re-subscribed to our marketing email.</p>
    </PublicNotice>
  );

  return (
    <PublicNotice icon={XCircle} tone="red" title="Something went wrong">
      <p>{msg}</p>
    </PublicNotice>
  );
}

function Waiting({ text }: { text: string }) {
  return (
    <div role="status" className="flex flex-col items-center gap-3 rounded-lg border border-line bg-surface px-6 py-12 text-sm text-fg-muted shadow-card">
      <Loader2 className="size-6 animate-spin text-brand-ink" aria-hidden />
      <p>{text}</p>
    </div>
  );
}

export default function UnsubscribePage() {
  return (
    <PublicPage subtitle="Email preferences" width="sm" center footer="hello@example.com">
      <Suspense fallback={<Waiting text="Loading..." />}>
        <UnsubscribeContent />
      </Suspense>
    </PublicPage>
  );
}
