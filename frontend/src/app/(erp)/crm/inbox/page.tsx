import { redirect } from "next/navigation";

// The CRM email inbox lives at /crm/email. Older Nary notifications linked to /crm/inbox?thread=...;
// keep those links working by forwarding with the same query.
export default function CrmInboxRedirect({
  searchParams,
}: {
  searchParams: Record<string, string | string[] | undefined>;
}) {
  const query = new URLSearchParams();
  for (const [key, value] of Object.entries(searchParams)) {
    for (const v of Array.isArray(value) ? value : value === undefined ? [] : [value]) query.append(key, v);
  }
  const qs = query.toString();
  redirect(qs ? `/crm/email?${qs}` : "/crm/email");
}
