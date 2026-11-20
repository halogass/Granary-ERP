"use client";

import { useState, useEffect } from "react";
import Link from "next/link";
import { CheckCircle2 } from "lucide-react";
import { Button, Card, CardBody, CardFooter, CardHeader, Field, FieldGrid, Input, Select, Textarea } from "@/components/ui";
import { PublicPage, PublicNotice } from "@/components/portal/PublicPage";
import { FormError } from "@/components/portal/AuthLayout";

interface Team { id: string; name: string; }
interface Category { id: string; team_id: string; name: string; }

export default function SupportPage() {
  const [teams, setTeams] = useState<Team[]>([]);
  const [categories, setCategories] = useState<Category[]>([]);
  const [form, setForm] = useState({
    customer_name: "", customer_email: "", customer_company: "",
    subject: "", description: "", priority: "normal",
    team_id: "", category_id: "",
  });
  const [submitting, setSubmitting] = useState(false);
  const [submitted, setSubmitted] = useState<{ ticket_number: string; public_token: string } | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    fetch("/api/v1/helpdesk/public/teams")
      .then(r => r.ok ? r.json() : [])
      .then(setTeams)
      .catch(() => {});
  }, []);

  useEffect(() => {
    if (!form.team_id) { setCategories([]); return; }
    fetch(`/api/v1/helpdesk/public/categories?team_id=${encodeURIComponent(form.team_id)}`)
      .then(r => r.ok ? r.json() : [])
      .then(setCategories)
      .catch(() => {});
  }, [form.team_id]);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!form.customer_name.trim() || !form.customer_email.trim() || !form.subject.trim()) {
      setError("Name, email, and subject are required.");
      return;
    }
    setSubmitting(true);
    setError("");
    try {
      const res = await fetch("/api/v1/helpdesk/public/tickets", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          ...form,
          team_id: form.team_id || null,
          category_id: form.category_id || null,
          source: "portal",
        }),
      });
      if (!res.ok) {
        const d = await res.json();
        setError(d.detail ?? "Submission failed. Please try again.");
        return;
      }
      const data = await res.json();
      setSubmitted({ ticket_number: data.ticket_number, public_token: data.public_token });
    } catch {
      setError("Network error. Please try again.");
    } finally {
      setSubmitting(false);
    }
  }

  if (submitted) {
    return (
      <PublicPage subtitle="Support" width="sm" center>
        <PublicNotice icon={CheckCircle2} tone="blue" title="Ticket submitted"
          action={
            <div className="flex w-full flex-col gap-2">
              <Button variant="primary" size="lg" className="w-full" asChild>
                <a href={`/support/track/${submitted.public_token}`}>Track my ticket</a>
              </Button>
              <Button variant="ghost" className="w-full"
                onClick={() => { setSubmitted(null); setForm({ customer_name: "", customer_email: "", customer_company: "", subject: "", description: "", priority: "normal", team_id: "", category_id: "" }); }}>
                Submit another ticket
              </Button>
            </div>
          }>
          <p>Your support request has been received. Our team will get back to you shortly.</p>
          <div className="mt-5 rounded-lg bg-surface-2 p-4">
            <p className="text-xs text-fg-muted">Ticket number</p>
            <p className="mt-1 font-mono text-lg font-semibold text-fg">{submitted.ticket_number}</p>
          </div>
        </PublicNotice>
      </PublicPage>
    );
  }

  return (
    <PublicPage subtitle="Support">
      <div className="mb-6">
        <p className="mb-1 text-xs font-semibold uppercase tracking-[0.16em] text-brand-ink">Support portal</p>
        <h1 className="text-2xl font-semibold text-fg">How can we help you?</h1>
        <p className="mt-1 text-sm text-fg-muted">
          Submit your request below. You will receive a tracking link to follow its progress.
        </p>
      </div>

      <form onSubmit={submit}>
        <Card>
          <CardHeader title="Your information" />
          <CardBody>
            <FieldGrid cols={2}>
              <Field label="Full name" required>
                <Input value={form.customer_name} onChange={e => setForm({ ...form, customer_name: e.target.value })} required autoComplete="name" placeholder="John Doe" />
              </Field>
              <Field label="Email address" required>
                <Input type="email" value={form.customer_email} onChange={e => setForm({ ...form, customer_email: e.target.value })} required autoComplete="email" placeholder="you@company.com" />
              </Field>
              <Field label="Company or organization" className="md:col-span-2">
                <Input value={form.customer_company} onChange={e => setForm({ ...form, customer_company: e.target.value })} autoComplete="organization" placeholder="Acme Corp (optional)" />
              </Field>
            </FieldGrid>
          </CardBody>
          <CardHeader title="Your request" className="border-t" />
          <CardBody>
            <FieldGrid cols={3}>
              <Field label="Subject" required className="md:col-span-3">
                <Input value={form.subject} onChange={e => setForm({ ...form, subject: e.target.value })} required placeholder="Brief description of your issue" />
              </Field>
              <Field label="Priority">
                <Select value={form.priority} onChange={e => setForm({ ...form, priority: e.target.value })}
                  options={[{ value: "low", label: "Low" }, { value: "normal", label: "Normal" }, { value: "high", label: "High" }]} />
              </Field>
              {teams.length > 0 && (
                <Field label="Team">
                  <Select value={form.team_id} onChange={e => setForm({ ...form, team_id: e.target.value, category_id: "" })}
                    placeholder="Any team" options={teams.map(t => ({ value: t.id, label: t.name }))} />
                </Field>
              )}
              {categories.length > 0 && (
                <Field label="Category">
                  <Select value={form.category_id} onChange={e => setForm({ ...form, category_id: e.target.value })}
                    placeholder="Select a category" options={categories.map(c => ({ value: c.id, label: c.name }))} />
                </Field>
              )}
              <Field label="Description" className="md:col-span-3">
                <Textarea value={form.description} onChange={e => setForm({ ...form, description: e.target.value })} rows={5}
                  placeholder="Please give as much detail as you can about your issue or request..." />
              </Field>
            </FieldGrid>
            {error && <div className="mt-4"><FormError>{error}</FormError></div>}
          </CardBody>
          <CardFooter className="flex-col items-stretch gap-2 py-4">
            <Button type="submit" variant="primary" size="lg" className="w-full" loading={submitting}>
              {submitting ? "Submitting..." : "Submit support request"}
            </Button>
            <p className="text-center text-xs text-fg-muted">
              After submitting, you will receive a tracking link to follow your ticket status.
            </p>
          </CardFooter>
        </Card>
      </form>
      <p className="mt-6 text-center text-[13px] text-fg-muted">
        Staff member? <Link href="/login" className="text-fg hover:underline">Sign in to the ERP</Link>
      </p>
    </PublicPage>
  );
}
