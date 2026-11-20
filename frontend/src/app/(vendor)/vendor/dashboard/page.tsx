import Link from "next/link";
import { getServerSession } from "next-auth";
import { ArrowRight, ClipboardList, FileText, Timer } from "lucide-react";
import { authOptions } from "@/lib/auth";
import ForcePasswordChange from "@/components/portal/ForcePasswordChange";
import { PageHeader } from "@/components/ui/PageHeader";

const AREAS = [
  { title: "Work orders", href: "/vendor/work-orders", icon: ClipboardList, note: "Assignments from us." },
  { title: "Timesheets", href: "/vendor/timesheets", icon: Timer, note: "Log your hours for review and payment." },
  { title: "Invoices", href: "/vendor/invoices", icon: FileText, note: "Submit invoices and track payment." },
];

export default async function VendorDashboard() {
  const session = await getServerSession(authOptions);
  return (
    <div>
      {session?.user?.must_change_password && <ForcePasswordChange />}

      <PageHeader title="Vendor dashboard" summary={session?.user?.name ? `Welcome, ${session.user.name}.` : undefined} />

      <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
        {AREAS.map(({ title, href, icon: Icon, note }) => (
          <Link key={href} href={href}
            className="group flex items-start gap-3 rounded-lg border border-line bg-surface p-4 shadow-card transition-colors hover:border-line-strong focus-visible:outline focus-visible:outline-2 focus-visible:outline-[rgb(var(--focus)/0.7)]">
            <span className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-surface-2 text-fg-muted">
              <Icon className="size-4" aria-hidden />
            </span>
            <span className="min-w-0 flex-1">
              <span className="block text-sm font-semibold text-fg">{title}</span>
              <span className="mt-0.5 block text-[13px] text-fg-muted">{note}</span>
            </span>
            <ArrowRight className="mt-0.5 size-4 shrink-0 text-fg-subtle transition-colors group-hover:text-fg" aria-hidden />
          </Link>
        ))}
      </div>
    </div>
  );
}
