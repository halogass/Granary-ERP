"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { Check } from "lucide-react";
import { Button, Card, CardBody, CardFooter, Field, FieldGrid, Input, Pill, Select, Textarea, toast } from "@/components/ui";
import { PublicPage, PublicNotice } from "@/components/portal/PublicPage";

/** The partner types the API accepts (GET /api/v1/portal/register/partner-types, public). */
type TypeOption = { value: string; label: string };

export default function PartnerRegisterPage() {
  const [loading, setLoading] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const [types, setTypes] = useState<TypeOption[]>([]);
  useEffect(() => {
    let alive = true;
    fetch("/api/v1/portal/register/partner-types")
      .then((r) => (r.ok ? r.json() : []))
      .then((list) => { if (alive && Array.isArray(list)) setTypes(list); })
      .catch(() => { /* the field keeps the default type */ });
    return () => { alive = false; };
  }, []);
  const [form, setForm] = useState({
    company_name: "", contact_name: "", email: "", phone: "",
    partner_type: "reseller", country: "", website: "", notes: "",
  });

  function set(field: string, value: string) {
    setForm(f => ({ ...f, [field]: value }));
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setLoading(true);
    try {
      const res = await fetch("/api/v1/portal/register/partner", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(form),
      });
      if (!res.ok) {
        const err = await res.json();
        throw new Error(err.detail || "Submission failed");
      }
      setSubmitted(true);
    } catch (err: unknown) {
      toast.error(err instanceof Error ? err.message : "Submission failed. Please try again.");
    } finally {
      setLoading(false);
    }
  }

  if (submitted) {
    return (
      <PublicPage subtitle="Partner portal" width="sm" center>
        <PublicNotice icon={Check} tone="blue" title="Application submitted"
          action={<Button variant="primary" asChild><Link href="/partner/login">Back to partner sign in</Link></Button>}>
          <p>
            Your partner application has been submitted. Our partnerships team will review it and
              reach out to <strong className="text-fg">{form.email}</strong> within 2 to 3 business days.
          </p>
        </PublicNotice>
      </PublicPage>
    );
  }

  return (
    <PublicPage subtitle="Partner portal" width="sm">
      <div className="mb-6 text-center">
        <Pill tone="blue">Partner program</Pill>
        <h1 className="mt-3 text-xl font-semibold text-fg">Apply to the partner program</h1>
        <p className="mt-1 text-sm text-fg-muted">Tell us about your company to join our partner network.</p>
      </div>

      <Card>
        <form onSubmit={handleSubmit}>
          <CardBody>
            <FieldGrid cols={2}>
              <Field label="Company name" required className="md:col-span-2">
                <Input type="text" value={form.company_name} onChange={(e) => set("company_name", e.target.value)} required autoComplete="organization" />
              </Field>
              <Field label="Contact name" required>
                <Input type="text" value={form.contact_name} onChange={(e) => set("contact_name", e.target.value)} required autoComplete="name" />
              </Field>
              <Field label="Phone">
                <Input type="tel" value={form.phone} onChange={(e) => set("phone", e.target.value)} autoComplete="tel" />
              </Field>
              <Field label="Business email" required className="md:col-span-2">
                <Input type="email" value={form.email} onChange={(e) => set("email", e.target.value)} required autoComplete="email" />
              </Field>
              <Field label="Partner type" required className="md:col-span-2">
                <Select value={form.partner_type} onChange={(e) => set("partner_type", e.target.value)} options={types.length ? types : [{ value: "reseller", label: "Reseller" }]} />
              </Field>
              <Field label="Country">
                <Input type="text" value={form.country} onChange={(e) => set("country", e.target.value)} placeholder="Japan" autoComplete="country-name" />
              </Field>
              <Field label="Website">
                <Input type="url" value={form.website} onChange={(e) => set("website", e.target.value)} placeholder="https://..." />
              </Field>
              <Field label="Tell us about your business" className="md:col-span-2">
                <Textarea value={form.notes} onChange={(e) => set("notes", e.target.value)} rows={3}
                  placeholder="Your target markets, existing client base, why you want to partner with us..." />
              </Field>
            </FieldGrid>
          </CardBody>
          <CardFooter className="flex-col items-stretch gap-3 py-4">
            <Button type="submit" variant="primary" size="lg" className="w-full" loading={loading}>
              {loading ? "Submitting..." : "Submit partner application"}
            </Button>
            <p className="text-center text-[13px] text-fg-muted">
              Already a partner?{" "}
              <Link href="/partner/login" className="font-medium text-brand-ink hover:underline">Sign in</Link>
            </p>
          </CardFooter>
        </form>
      </Card>
    </PublicPage>
  );
}
