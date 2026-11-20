"use client";

import { useEffect, useState } from "react";
import { Send } from "lucide-react";
import { Button, toast } from "@/components/ui";
import { approvalsApi } from "@/lib/api";
import SignatureSection from "./SignatureSection";
import { useSendDocument } from "./useSendDocument";
import VerificationSection from "./VerificationSection";
import type { EntityApproval } from "./types";

/** Verified PDF + e-signature actions for any approved document (used where the document has no detail page). */
const SENDABLE: Record<string, string> = {
  quotation: "Quotation", invoice: "Invoice", purchase_order: "Purchase order", work_order: "Work order",
  sales_document: "Proposal",
};

export default function DocumentActions({ entityType, entityId }: { entityType: string; entityId: string }) {
  const [data, setData] = useState<EntityApproval | null>(null);
  const sender = useSendDocument({
    entityType, label: SENDABLE[entityType] ?? "Document",
    onMessage: (kind, message) =>
      kind === "success" ? toast.success(message) : kind === "info" ? toast.info(message) : toast.error(message),
  });

  useEffect(() => {
    let alive = true;
    approvalsApi.forEntity(entityType, entityId)
      .then((res) => { if (alive) setData(res.data); })
      .catch(() => { if (alive) setData(null); });
    return () => { alive = false; };
  }, [entityType, entityId]);

  if (!data?.policy) return null;
  const approved = data.gate === "approved";
  const signing = data.policy.signature_flow && data.policy.signature_flow !== "none";
  const sendable = approved && entityType in SENDABLE;
  if (!data.policy.qr_enabled && !signing && !sendable) return null;

  return (
    <div className="space-y-4">
      {sendable && (
        <div>
          <Button variant="primary" onClick={() => sender.start(entityId)} loading={!!sender.preparing}>
            {!sender.preparing && <Send aria-hidden />} Send to {entityType === "purchase_order" || entityType === "work_order" ? "vendor" : "client"}
          </Button>
          {sender.modal}
        </div>
      )}
      {data.policy.qr_enabled && <VerificationSection entityType={entityType} entityId={entityId} canIssue={approved} />}
      {signing && (
        <SignatureSection entityType={entityType} entityId={entityId} flow={data.policy.signature_flow} canStart={approved} />
      )}
    </div>
  );
}
