import { approvalsApi, verificationApi } from "./api";

interface VerificationRow {
  status: string;
  live_state: string | null;
  has_pdf: boolean;
  id: string;
  document_number: string | null;
}

export type DownloadOutcome =
  | { outcome: "verified" }
  | { outcome: "print" }
  | { outcome: "refused"; message: string };

interface Options {
  /** Refuse every download until the document is approved, and never hand out an unverified print of it. */
  requireApproval?: boolean;
  /** Used in refusal messages, e.g. "quotation". */
  label?: string;
}

function saveBlob(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 30_000);
}

function notApprovedMessage(gate: string, label: string): string {
  switch (gate) {
    case "pending": return `This ${label} is awaiting approval. It can be downloaded once approved.`;
    case "rejected": return `This ${label} was rejected. Revise and resubmit it before downloading.`;
    case "stale": return `This ${label} changed after it was approved. Resubmit it for approval before downloading.`;
    case "cancelled": return `The approval request was withdrawn. Resubmit this ${label} before downloading.`;
    default: return `This ${label} must be approved before it can be downloaded. Submit it for approval first.`;
  }
}

async function fetchVerified(entityType: string, entityId: string, mayIssue: boolean): Promise<boolean> {
  const listed = await verificationApi.forEntity(entityType, entityId);
  let active: VerificationRow | undefined = (listed.data.verifications as VerificationRow[] | undefined)
    ?.find((v) => v.status === "active" && v.live_state === "valid" && v.has_pdf);
  if (!active && mayIssue && listed.data.supported && listed.data.qr_enabled) {
    active = (await verificationApi.issue(entityType, entityId)).data;
  }
  if (!active) return false;
  const pdf = await verificationApi.downloadPdf(active.id);
  const name = (active.document_number ?? "document").replace(/[^A-Za-z0-9._-]+/g, "_");
  saveBlob(pdf.data as Blob, `${name}.pdf`);
  return true;
}

/**
 * The "Download" button: hand out the verified PDF (QR on every page) whenever the document has one,
 * issuing it first when the document is approved but no copy exists yet.
 *
 * Default: anything else falls back to the plain browser print.
 * requireApproval: an unapproved document is refused outright, and an approved one is only ever
 * handed out as its verified copy.
 */
export async function downloadVerifiedCopy(
  entityType: string,
  entityId: string,
  fallbackPrint: () => Promise<void> | void,
  { requireApproval = false, label = "document" }: Options = {},
): Promise<DownloadOutcome> {
  if (requireApproval) {
    let gate: string;
    try {
      gate = (await approvalsApi.forEntity(entityType, entityId)).data.gate;
    } catch {
      return { outcome: "refused", message: `Could not confirm this ${label} is approved. Try again.` };
    }
    if (gate === "not_required") {
      await fallbackPrint();          // no approval policy governs this document type
      return { outcome: "print" };
    }
    if (gate !== "approved") return { outcome: "refused", message: notApprovedMessage(gate, label) };
    try {
      if (await fetchVerified(entityType, entityId, true)) return { outcome: "verified" };
    } catch { /* reported below */ }
    return { outcome: "refused", message: `The verified PDF for this ${label} could not be produced. Try again shortly.` };
  }

  try {
    const listed = await verificationApi.forEntity(entityType, entityId);
    const hasActive = (listed.data.verifications as VerificationRow[] | undefined)
      ?.some((v) => v.status === "active" && v.live_state === "valid" && v.has_pdf);
    let mayIssue = false;
    if (!hasActive && listed.data.supported && listed.data.qr_enabled) {
      mayIssue = (await approvalsApi.forEntity(entityType, entityId)).data.gate === "approved";
    }
    if ((hasActive || mayIssue) && await fetchVerified(entityType, entityId, mayIssue)) return { outcome: "verified" };
  } catch {
    /* fall through to the plain print */
  }
  await fallbackPrint();
  return { outcome: "print" };
}
