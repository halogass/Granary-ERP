import { getServerSession } from "next-auth";
import { redirect } from "next/navigation";
import { authOptions } from "@/lib/auth";
import Sidebar from "@/components/layout/Sidebar";
import Topbar from "@/components/layout/Topbar";
import BottomNav from "@/components/layout/BottomNav";
import CommandPalette from "@/components/layout/CommandPalette";
import { NavBadgesProvider } from "@/components/layout/useNavBadges";
import SessionErrorGuard from "@/components/SessionErrorGuard";
import DocumentTitle from "@/components/DocumentTitle";
import AssistantLauncher from "@/components/ai/AssistantLauncher";
import ForcePasswordChange from "@/components/portal/ForcePasswordChange";

export default async function ERPLayout({ children }: { children: React.ReactNode }) {
  const session = await getServerSession(authOptions);
  // No session, or the backend refresh token expired (cookie still present) →
  // never render the ERP shell; send to login. Mirrors middleware.ts.
  if (!session || session.error === "RefreshTokenExpired") redirect("/login");
  if (session.user.portal_type === "partner") redirect("/partner/dashboard");
  if (session.user.portal_type === "vendor") redirect("/vendor/dashboard");

  return (
    <NavBadgesProvider>
      <div className="flex h-dvh overflow-hidden bg-canvas">
        <a href="#main" className="sr-only focus:not-sr-only focus:fixed focus:left-3 focus:top-3 focus:z-[80] focus:rounded-lg focus:bg-surface focus:px-3 focus:py-2 focus:text-sm focus:shadow-pop">
          Skip to content
        </a>
        <SessionErrorGuard />
        <DocumentTitle />
        <Sidebar user={session.user} />
        <div className="flex min-w-0 flex-1 flex-col overflow-hidden">
          <Topbar user={session.user} />
          {/* bottom padding on phones leaves room for the bottom nav */}
          <main id="main" className="flex-1 overflow-y-auto p-4 pb-24 md:p-6 md:pb-6">{children}</main>
        </div>
        <BottomNav />
        <CommandPalette />
        <AssistantLauncher />
        {/* Stage 8: the API refuses everything else until a temporary password is replaced */}
        {session.user.must_change_password && <ForcePasswordChange />}
      </div>
    </NavBadgesProvider>
  );
}
