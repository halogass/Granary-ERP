"use client";

import { useState } from "react";
import { documentsApi, type SendDefaults } from "@/lib/api";
import { apiErrorCode, extractApiError } from "@/lib/apiError";
import SendDocumentModal from "./SendDocumentModal";

interface Options {
  entityType: string;
  label: string;
  /** Sent, or submitted for approval: the page reloads its document and approval panel. */
  onChanged?: () => void;
  onMessage: (kind: "success" | "info" | "error", message: string) => void;
}

/**
 * "Send": nothing leaves before approval. An unapproved document is submitted (auto-approved within
 * standing authority, or when the Owner sends their own); the user is told it now waits for approval.
 * An approved one opens the send dialog with the recipient pre-filled.
 */
export function useSendDocument({ entityType, label, onChanged, onMessage }: Options) {
  const [target, setTarget] = useState<{ id: string; defaults: SendDefaults } | null>(null);
  const [preparing, setPreparing] = useState<string | null>(null);

  async function start(entityId: string) {
    setPreparing(entityId);
    try {
      const res = await documentsApi.prepareSend(entityType, entityId);
      setTarget({ id: entityId, defaults: res.data });
    } catch (e) {
      if (apiErrorCode(e) === "approval_pending") {
        onMessage("info", extractApiError(e,
          `Submitted for approval: the ${label.toLowerCase()} can be sent once it is approved.`));
      } else {
        onMessage("error", extractApiError(e, `The ${label.toLowerCase()} cannot be sent yet.`));
      }
    } finally {
      setPreparing(null);
      onChanged?.();
      window.dispatchEvent(new Event("approvals:changed"));
    }
  }

  const modal = target ? (
    <SendDocumentModal entityType={entityType} entityId={target.id} label={label} defaults={target.defaults}
      onClose={() => setTarget(null)}
      onSent={(message) => { setTarget(null); onMessage("success", message); onChanged?.(); }} />
  ) : null;

  return { start, preparing, modal };
}
