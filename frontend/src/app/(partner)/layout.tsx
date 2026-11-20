import { getServerSession } from "next-auth";
import { redirect } from "next/navigation";
import { authOptions } from "@/lib/auth";
import { PortalShell } from "@/components/portal/PortalShell";

const NAV = [
  { label: "Dashboard",    href: "/partner/dashboard" },
  { label: "Deals",        href: "/partner/deals" },
  { label: "Leads",        href: "/partner/leads" },
  { label: "MDF",          href: "/partner/mdf" },
  { label: "Commissions",  href: "/partner/commissions" },
  { label: "Resources",    href: "/partner/resources" },
  { label: "Documents",    href: "/partner/documents" },
  { label: "Training",     href: "/partner/training" },
];

export default async function PartnerLayout({ children }: { children: React.ReactNode }) {
  const session = await getServerSession(authOptions);
  if (!session || session.error === "RefreshTokenExpired") redirect("/partner/login");
  if (session.user.portal_type !== "partner" && !session.user.is_superadmin) {
    redirect("/dashboard");
  }

  return (
    <PortalShell subtitle="Partner portal" nav={NAV} loginPath="/partner/login"
      user={{ name: session.user?.name, email: session.user?.email }}>
      {children}
    </PortalShell>
  );
}
