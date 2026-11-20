"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import Link from "next/link";
import { Check } from "lucide-react";
import { Button, Card, CardBody, CardFooter, Field, FieldGrid, Input, Pill, Select, Textarea, toast } from "@/components/ui";
import { PublicPage, PublicNotice } from "@/components/portal/PublicPage";

const VENDOR_TYPES = [
  { value: "professional_services", label: "Professional services" },
  { value: "it_infrastructure", label: "IT infrastructure" },
  { value: "component_supplier", label: "Component supplier" },
  { value: "assembly_subcontractor", label: "Assembly subcontractor" },
  { value: "facility_services", label: "Facility services" },
  { value: "logistics", label: "Logistics and freight" },
  { value: "temp_staff", label: "Temporary staffing" },
  { value: "professional_nontech", label: "Professional non-technical (legal, accounting)" },
];

export default function VendorRegisterPage() {
  const router = useRouter();
  const [loading, setLoading] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const [form, setForm] = useState({
    company_name: "", contact_name: "", email: "", phone: "",
    vendor_type: "professional_services", country: "", website: "", notes: "",
  });

  function set(field: string, value: string) {
    setForm(f => ({ ...f, [field]: value }));
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setLoading(true);
    try {
      const res = await fetch("/api/v1/portal/register/vendor", {
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
      <PublicPage subtitle="Vendor portal" width="sm" center>
        <PublicNotice icon={Check} tone="blue" title="Request submitted"
          action={<Button variant="primary" asChild><Link href="/vendor/login">Back to vendor sign in</Link></Button>}>
          <p>
            Your vendor registration request has been submitted. Our team will review it and
              send your login credentials to <strong className="text-fg">{form.email}</strong> once approved.
          </p>
        </PublicNotice>
      </PublicPage>
    );
  }

  return (
    <PublicPage subtitle="Vendor portal" width="sm">
      <div className="mb-6 text-center">
        <Pill tone="orange">Vendor or contractor</Pill>
        <h1 className="mt-3 text-xl font-semibold text-fg">Request portal access</h1>
        <p className="mt-1 text-sm text-fg-muted">Fill in your details to request a vendor portal account.</p>
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
              <Field label="Email address" required className="md:col-span-2">
                <Input type="email" value={form.email} onChange={(e) => set("email", e.target.value)} required autoComplete="email" />
              </Field>
              <Field label="Vendor or contractor type" required className="md:col-span-2">
                <Select value={form.vendor_type} onChange={(e) => set("vendor_type", e.target.value)} options={VENDOR_TYPES} />
              </Field>
              <Field label="Country">
                <Input type="text" value={form.country} onChange={(e) => set("country", e.target.value)} placeholder="Japan" autoComplete="country-name" />
              </Field>
              <Field label="Website">
                <Input type="url" value={form.website} onChange={(e) => set("website", e.target.value)} placeholder="https://..." />
              </Field>
              <Field label="Services offered" className="md:col-span-2">
                <Textarea value={form.notes} onChange={(e) => set("notes", e.target.value)} rows={3}
                  placeholder="Briefly describe your services..." />
              </Field>
            </FieldGrid>
          </CardBody>
          <CardFooter className="flex-col items-stretch gap-3 py-4">
            <Button type="submit" variant="primary" size="lg" className="w-full" loading={loading}>
              {loading ? "Submitting..." : "Submit registration request"}
            </Button>
            <p className="text-center text-[13px] text-fg-muted">
              Already have an account?{" "}
              <Link href="/vendor/login" className="font-medium text-brand-ink hover:underline">Sign in</Link>
            </p>
          </CardFooter>
        </form>
      </Card>
    </PublicPage>
  );
}
