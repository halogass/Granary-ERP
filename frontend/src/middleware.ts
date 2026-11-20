import { getToken } from "next-auth/jwt";
import { NextResponse } from "next/server";
import type { NextRequest } from "next/server";

const PUBLIC_PATHS = [
  "/login", "/vendor/login", "/partner/login",
  "/vendor/register", "/partner/register",
  "/support", "/support/track",
  "/sign/", "/verify/", "/v/",
  "/meet/",
  "/drop/",       // 5 Nov 2026: one-time upload links (the token is the key)
  "/unsubscribe",
  "/clock",        // kiosks authenticate with their own device token; personal mode redirects to login itself
  "/api/",
  "/.well-known/oauth-",   // MCP sign-in discovery (agents, no session): rewritten to the backend in next.config
  "/_next/", "/favicon",
];

function isPublic(pathname: string): boolean {
  return PUBLIC_PATHS.some((p) => pathname.startsWith(p));
}

// Build a redirect to a login page that also clears any stale NextAuth session
// cookie. Used when there is no token OR the token's backend refresh has expired,
// so a protected page never renders for an unauthenticated/expired session.
function redirectToLogin(request: NextRequest, loginPath: string): NextResponse {
  const res = NextResponse.redirect(new URL(loginPath, request.url));
  res.cookies.delete("next-auth.session-token");
  res.cookies.delete("__Secure-next-auth.session-token");
  return res;
}

export async function middleware(request: NextRequest) {
  const { pathname } = request.nextUrl;

  if (isPublic(pathname)) return NextResponse.next();

  const token = await getToken({ req: request, secret: process.env.NEXTAUTH_SECRET });

  const isVendorRoute = pathname === "/vendor" || pathname.startsWith("/vendor/");
  const isPartnerRoute = pathname === "/partner" || pathname.startsWith("/partner/");

  // Treat a token whose backend refresh has expired the same as no token at all,
  // so an expired session can never flash a protected page before the client
  // signs out. (The NextAuth cookie lives 30 days even after the backend refresh
  // token is revoked, with `error: "RefreshTokenExpired"`.)
  const isExpired = !!(token as { error?: string } | null)?.error;

  // Not authenticated (or session expired), redirect to appropriate login page
  if (!token || isExpired) {
    if (isVendorRoute) return redirectToLogin(request, "/vendor/login");
    if (isPartnerRoute) return redirectToLogin(request, "/partner/login");
    // An agent's sign-in (MCP) comes back to its consent page, with all its parameters, after the login.
    if (pathname.startsWith("/oauth/")) {
      return redirectToLogin(request, `/login?callbackUrl=${encodeURIComponent(pathname + request.nextUrl.search)}`);
    }
    return redirectToLogin(request, "/login");
  }

  const portalType = (token as { portal_type?: string }).portal_type ?? "internal";
  const isSuperadmin = (token as { is_superadmin?: boolean }).is_superadmin ?? false;

  // Vendor portal routes: only vendor users (or superadmin) allowed
  if (isVendorRoute) {
    if (portalType !== "vendor" && !isSuperadmin) {
      return NextResponse.redirect(new URL("/vendor/login", request.url));
    }
    return NextResponse.next();
  }

  // Partner portal routes: only partner users (or superadmin) allowed
  if (isPartnerRoute) {
    if (portalType !== "partner" && !isSuperadmin) {
      return NextResponse.redirect(new URL("/partner/login", request.url));
    }
    return NextResponse.next();
  }

  // ERP routes: redirect portal users away from the internal ERP
  if (pathname.startsWith("/dashboard") || pathname.startsWith("/finance") ||
      pathname.startsWith("/hr") || pathname.startsWith("/crm") ||
      pathname.startsWith("/projects") || pathname.startsWith("/sales") ||
      pathname.startsWith("/manufacturing") || pathname.startsWith("/fleet") ||
      pathname.startsWith("/maintenance") || pathname.startsWith("/helpdesk") ||
      pathname.startsWith("/sign/") || pathname.startsWith("/inventory") ||
      pathname.startsWith("/partner-mgmt") || pathname.startsWith("/vendor-mgmt") ||
      pathname.startsWith("/admin")) {
    if (portalType === "vendor") {
      return NextResponse.redirect(new URL("/vendor/dashboard", request.url));
    }
    if (portalType === "partner") {
      return NextResponse.redirect(new URL("/partner/dashboard", request.url));
    }
  }

  return NextResponse.next();
}

export const config = {
  matcher: [
    "/((?!_next/static|_next/image|favicon.ico|public/).*)",
  ],
};
