"use client";

import { useState } from "react";
import { Button } from "./Button";
import { Dialog } from "./Dialog";
import { Field } from "./Field";
import { Textarea } from "./Input";

interface Props {
  title: string;
  description?: string;
  /** A trailing " *" is read as "required" and shown as the Field's own mark. */
  label?: string;
  placeholder?: string;
  confirmText?: string;
  /** danger for destructive confirms (revoke, cancel); primary otherwise. */
  confirmVariant?: "primary" | "danger";
  onConfirm: (reason: string) => Promise<void>;
  onClose: () => void;
}

/** Small modal that collects a required reason and runs an async confirm action. */
export default function ReasonModal({
  title, description, label = "Reason *", placeholder,
  confirmText = "Confirm", confirmVariant = "primary",
  onConfirm, onClose,
}: Props) {
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const submit = async () => {
    if (!reason.trim()) { setError("Please enter a reason."); return; }
    setBusy(true); setError("");
    try {
      await onConfirm(reason.trim());
    } catch (e) {
      setError((e as { response?: { data?: { detail?: string } } })?.response?.data?.detail || (e as Error).message || "Failed.");
      setBusy(false);
    }
  };

  const fieldLabel = label.replace(/\s*\*\s*$/, "");
  return (
    <Dialog open onOpenChange={(o) => { if (!o && !busy) onClose(); }} dismissable={!busy} size="md"
      title={title} description={description}
      footer={<>
        <Button onClick={onClose} disabled={busy}>Cancel</Button>
        <Button variant={confirmVariant} onClick={submit} loading={busy} disabled={busy || !reason.trim()}>{confirmText}</Button>
      </>}>
      <Field label={fieldLabel} required={fieldLabel !== label} error={error || undefined}>
        <Textarea value={reason} onChange={(e) => setReason(e.target.value)} rows={3} autoFocus placeholder={placeholder} />
      </Field>
    </Dialog>
  );
}
