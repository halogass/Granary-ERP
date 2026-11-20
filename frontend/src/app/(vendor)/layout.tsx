import { getServerSession } from "next-auth";
import { redirect } from "next/navigation";
import { authOptions } from "@/lib/auth";
import { PortalShell } from "@/components/portal/PortalShell";

const NAV = [
  { label: "Dashboard",   href: "/vendor/dashboard" },
  { label: "Work orders", href: "/vendor/work-orders" },
  { label: "RFQs",        href: "/vendor/rfqs" },
  { label: "Invoices",    href: "/vendor/invoices" },
  { label: "Timesheets",  href: "/vendor/timesheets" },
  { label: "Documents",   href: "/vendor/documents" },
];

export default async function VendorLayout({ children }: { children: React.ReactNode }) {
  const session = await getServerSession(authOptions);
  if (!session || session.error === "RefreshTokenExpired") redirect("/vendor/login");
  if (session.user.portal_type !== "vendor" && !session.user.is_superadmin) {
    redirect("/dashboard");
  }

  return (
    <PortalShell subtitle="Vendor and contractor portal" nav={NAV} loginPath="/vendor/login"
      user={{ name: session.user?.name, email: session.user?.email }}>
      {children}
    </PortalShell>
  );
}
