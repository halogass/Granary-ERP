"use client";

import { useEffect, useState } from "react";
import { hrApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Button, Dialog, Field, FieldGrid, Input, Select, Textarea } from "@/components/ui";
import { GRANARY_DEPARTMENTS as DEPTS } from "@/lib/departments";

export interface OfferLetter {
  id: string;
  number: string;
  applicant_id: string;
  applicant_name: string | null;
  applicant_email: string | null;
  job_opening_id: string | null;
  position_title: string;
  department: string | null;
  employment_type: string | null;
  salary_gross_monthly: number | null;
  salary_currency: string;
  start_date: string | null;
  probation_months: number | null;
  benefits: string | null;
  other_terms: string | null;
  acceptance_deadline: string | null;
  status: string;
  sent_at: string | null;
  responded_at: string | null;
  candidate_response_notes: string | null;
  created_at: string | null;
  approval: { request_id: string; status: string; current_step: string | null } | null;
}

interface Applicant { id: string; name: string; email?: string; stage: string; job_opening_id: string }
interface JobOpening { id: string; title: string; department?: string; employment_type?: string }

/** Offer status labels; colors come from the "offer" family in components/ui/status.ts. */
export const OFFER_STATUS_LABELS: Record<string, string> = {
  draft: "Draft", sent: "Sent", accepted: "Accepted", declined: "Declined",
  countered: "Countered", expired: "Expired", cancelled: "Cancelled",
};

const EMP_TYPES = ["full_time", "part_time", "contract", "intern"];

interface Props {
  offer?: OfferLetter;
  presetApplicantId?: string;
  onClose: () => void;
  onSaved: (offer: OfferLetter) => void;
}

/** Create or edit (draft only) an offer letter. */
export default function OfferLetterFormModal({ offer, presetApplicantId, onClose, onSaved }: Props) {
  const [applicants, setApplicants] = useState<Applicant[]>([]);
  const [openings, setOpenings] = useState<JobOpening[]>([]);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [form, setForm] = useState({
    applicant_id: offer?.applicant_id ?? presetApplicantId ?? "",
    position_title: offer?.position_title ?? "",
    department: offer?.department ?? "",
    employment_type: offer?.employment_type ?? "full_time",
    salary_gross_monthly: offer?.salary_gross_monthly != null ? String(offer.salary_gross_monthly) : "",
    salary_currency: offer?.salary_currency ?? "USD",
    start_date: offer?.start_date ?? "",
    probation_months: String(offer?.probation_months ?? 3),
    acceptance_deadline: offer?.acceptance_deadline ?? "",
    benefits: offer?.benefits ?? "",
    other_terms: offer?.other_terms ?? "",
  });

  useEffect(() => {
    hrApi.listApplicants().then((r) => setApplicants(r.data ?? [])).catch(() => setApplicants([]));
    hrApi.listJobOpenings({ status: "all" }).then((r) => setOpenings(r.data ?? [])).catch(() => setOpenings([]));
  }, []);

  function pickApplicant(id: string) {
    const applicant = applicants.find((a) => a.id === id);
    const opening = openings.find((o) => o.id === applicant?.job_opening_id);
    setForm((f) => ({
      ...f,
      applicant_id: id,
      position_title: f.position_title || opening?.title || "",
      department: f.department || opening?.department || "",
      employment_type: opening?.employment_type || f.employment_type,
    }));
  }

  async function save() {
    if (!form.applicant_id || !form.position_title.trim() || !(parseFloat(form.salary_gross_monthly) > 0)) {
      setError("Candidate, position and a monthly gross salary are required.");
      return;
    }
    setSaving(true);
    setError("");
    const payload = {
      applicant_id: form.applicant_id,
      position_title: form.position_title.trim(),
      department: form.department || undefined,
      employment_type: form.employment_type,
      salary_gross_monthly: parseFloat(form.salary_gross_monthly),
      salary_currency: form.salary_currency,
      start_date: form.start_date || undefined,
      probation_months: parseInt(form.probation_months) || 0,
      acceptance_deadline: form.acceptance_deadline || undefined,
      benefits: form.benefits.trim() || undefined,
      other_terms: form.other_terms.trim() || undefined,
    };
    try {
      const res = offer ? await hrApi.updateOfferLetter(offer.id, payload) : await hrApi.createOfferLetter(payload);
      onSaved(res.data);
    } catch (e) {
      setError(extractApiError(e, "Could not save the offer letter."));
    } finally {
      setSaving(false);
    }
  }

  const set = (key: keyof typeof form) =>
    (e: React.ChangeEvent<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>) =>
      setForm((f) => ({ ...f, [key]: e.target.value }));

  return (
    <Dialog open size="lg" onOpenChange={(o) => { if (!o && !saving) onClose(); }}
      title={offer ? `Edit ${offer.number}` : "New offer letter"}
      description="The Owner approves every offer before it is issued."
      footer={<>
        <Button onClick={onClose} disabled={saving}>Cancel</Button>
        <Button variant="primary" onClick={save} loading={saving}>{offer ? "Save changes" : "Create draft"}</Button>
      </>}>
      <FieldGrid cols={2}>
        <Field label="Candidate" required className="md:col-span-2">
          <Select value={form.applicant_id} onChange={(e) => pickApplicant(e.target.value)} placeholder="Select a candidate...">
            {applicants.map((a) => (
              <option key={a.id} value={a.id}>{a.name}{a.email ? `, ${a.email}` : ""} ({a.stage.replace(/_/g, " ")})</option>
            ))}
          </Select>
        </Field>
        <Field label="Position" required><Input value={form.position_title} onChange={set("position_title")} /></Field>
        <Field label="Department">
          <Select value={form.department} onChange={set("department")} placeholder="-" options={DEPTS.map((d) => ({ value: d, label: d }))} />
        </Field>
        <Field label="Employment type">
          <Select value={form.employment_type} onChange={set("employment_type")}
            options={EMP_TYPES.map((t) => ({ value: t, label: t.replace("_", " ") }))} />
        </Field>
        <div className="grid grid-cols-3 gap-2">
          <Field label="Gross per month" required className="col-span-2">
            <Input numeric type="number" min="0" step="any" value={form.salary_gross_monthly} onChange={set("salary_gross_monthly")} />
          </Field>
          <Field label="Currency">
            <Select value={form.salary_currency} onChange={set("salary_currency")}
              options={["USD", "EUR", "GBP", "JPY"].map((c) => ({ value: c, label: c }))} />
          </Field>
        </div>
        <Field label="Start date"><Input type="date" value={form.start_date} onChange={set("start_date")} /></Field>
        <Field label="Probation (months, max 3)">
          <Input numeric type="number" min="0" max="3" value={form.probation_months} onChange={set("probation_months")} />
        </Field>
        <Field label="Respond by" hint="Defaults to 5 business days from today.">
          <Input type="date" value={form.acceptance_deadline} onChange={set("acceptance_deadline")} />
        </Field>
        <Field label="Benefits" className="md:col-span-2"><Textarea rows={2} value={form.benefits} onChange={set("benefits")} /></Field>
        <Field label="Other terms" className="md:col-span-2"><Textarea rows={2} value={form.other_terms} onChange={set("other_terms")} /></Field>
        {error && <p role="alert" className="text-xs text-tone-red md:col-span-2">{error}</p>}
      </FieldGrid>
    </Dialog>
  );
}
