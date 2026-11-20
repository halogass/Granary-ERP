import Link from "next/link";
import { Building2, User } from "lucide-react";
import { Card, CardBody, CardHeader } from "@/components/ui";

/** The customer fields GET /finance/invoices/{id} serves. */
export interface InvoiceClientFields {
  customer_org_id?: string | null;
  customer_contact_id?: string | null;
  customer_org?: { id: string; name: string; email?: string | null } | null;
  customer_contact?: { id: string; full_name: string; email?: string | null } | null;
  /** The bill-to snapshot on the invoice (kept even when the CRM record changes). */
  customer_name?: string | null;
  customer_email?: string | null;
  customer_contact_person?: string | null;
}

function Row({ icon: Icon, name, email, href }: { icon: React.ElementType; name: string; email?: string | null; href?: string }) {
  return (
    <div className="flex items-start gap-3">
      <Icon className="mt-0.5 size-4 shrink-0 text-fg-muted" aria-hidden />
      <div className="min-w-0">
        {href
          ? <Link href={href} className="text-sm font-medium text-fg hover:underline">{name}</Link>
          : <p className="text-sm font-medium text-fg">{name}</p>}
        {email && <p className="mt-0.5 text-xs text-fg-muted">{email}</p>}
      </div>
    </div>
  );
}

/** The invoice's client: its organization and contact, else the bill-to name on the invoice itself. */
export default function InvoiceClientCard({ invoice }: { invoice: InvoiceClientFields }) {
  const orgId = invoice.customer_org?.id ?? invoice.customer_org_id ?? null;
  const contactId = invoice.customer_contact?.id ?? invoice.customer_contact_id ?? null;
  const contactName = invoice.customer_contact?.full_name || invoice.customer_contact_person || "";
  // the bill-to name stands for the organization, unless the invoice is to a contact only
  const orgName = invoice.customer_org?.name || (orgId || !contactName ? invoice.customer_name : "") || "";
  // the invoice's own email goes with the organization, or with the contact when there is no organization
  const orgEmail = invoice.customer_org?.email || invoice.customer_email;
  const contactEmail = invoice.customer_contact?.email || (orgName ? null : invoice.customer_email);
  return (
    <Card>
      <CardHeader title="Client" />
      <CardBody className="space-y-3">
        {orgName && (
          <Row icon={Building2} name={orgName} email={orgEmail} href={orgId ? `/crm/organizations/${orgId}` : undefined} />
        )}
        {contactName && contactName !== orgName && (
          <Row icon={User} name={contactName} email={contactEmail} href={contactId ? `/crm/contacts/${contactId}` : undefined} />
        )}
        {!orgName && !contactName && <p className="text-sm text-fg-muted">No client set.</p>}
      </CardBody>
    </Card>
  );
}
