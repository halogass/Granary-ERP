import { NextAuthOptions } from "next-auth";
import CredentialsProvider from "next-auth/providers/credentials";
import axios from "axios";
import { TOTP_EXPIRED, TOTP_REQUIRED } from "./totpLogin";

const API_URL = process.env.INTERNAL_API_URL || process.env.API_URL || "http://backend:8000";

declare module "next-auth" {
  // No refresh_token here on purpose: the session is what /api/auth/session hands to page
  // JavaScript. The refresh token lives only in the encrypted, httpOnly NextAuth JWT cookie.
  interface Session {
    access_token: string;
    error?: string;
    user: {
      id: string;
      email: string;
      name: string;
      is_superadmin: boolean;
      is_system_admin: boolean;
      portal_type: string;
      must_change_password: boolean;
    };
  }
  interface User {
    id: string;
    email: string;
    name: string;
    access_token: string;
    refresh_token: string;
    is_superadmin: boolean;
    is_system_admin: boolean;
    portal_type: string;
    must_change_password: boolean;
  }
}

declare module "next-auth/jwt" {
  interface JWT {
    access_token: string;
    refresh_token: string;
    user_id: string;
    is_superadmin: boolean;
    is_system_admin: boolean;
    portal_type: string;
    must_change_password: boolean;
    access_token_expires_at: number; // ms epoch
    error?: string;
  }
}

type RefreshResult =
  | { ok: true; access_token: string; refresh_token: string }
  | { ok: false; fatal: boolean };

// One jwt callback runs per getSession() call, and the API client calls getSession()
// on every request: so a page loading a dozen widgets used to fire a dozen parallel
// POSTs to /auth/refresh with the SAME refresh token. The backend rotates the token on
// first use, so every loser of that race came back 401 and the user was thrown out to
// the login page mid-work. Share a single in-flight refresh per token instead.
const inFlightRefresh = new Map<string, Promise<RefreshResult>>();

type HeaderBag = Record<string, string | string[] | undefined> | Headers | undefined | null;

function headerValue(headers: HeaderBag, name: string): string {
  if (!headers) return "";
  const raw = typeof (headers as Headers).get === "function"
    ? (headers as Headers).get(name)
    : (headers as Record<string, string | string[] | undefined>)[name];
  const v = Array.isArray(raw) ? raw[0] : raw;
  return (v ?? "").trim();
}

/**
 * Headers that tell the backend who the real client is. The login reaches the backend from this
 * container, so without them every user shares one IP for the login rate limit. nginx sets
 * x-real-ip; the backend only trusts X-Client-IP alongside the shared INTERNAL_AUTH_TOKEN
 * (server-side env, never NEXT_PUBLIC). Without the token nothing is sent.
 */
export function clientIpHeaders(headers: HeaderBag): Record<string, string> {
  const secret = process.env.INTERNAL_AUTH_TOKEN;
  if (!secret) return {};
  // Only nginx's x-real-ip: the first X-Forwarded-For entry is whatever the client sent.
  const ip = headerValue(headers, "x-real-ip");
  if (!ip) return {};
  return { "X-Client-IP": ip, "X-Internal-Auth": secret };
}

/** The client IP headers for the request being handled, when there is one (route handler). */
async function requestIpHeaders(): Promise<Record<string, string>> {
  if (!process.env.INTERNAL_AUTH_TOKEN) return {};
  try {
    const { headers } = await import("next/headers");
    return clientIpHeaders(headers());
  } catch {
    return {}; // not inside a request (tests, build)
  }
}

function refreshTokens(refreshToken: string, headers: Record<string, string> = {}): Promise<RefreshResult> {
  const existing = inFlightRefresh.get(refreshToken);
  if (existing) return existing;

  const pending: Promise<RefreshResult> = axios
    .post(`${API_URL}/api/v1/auth/refresh`, { refresh_token: refreshToken }, { headers })
    .then((res) => ({
      ok: true as const,
      access_token: res.data.access_token,
      refresh_token: res.data.refresh_token,
    }))
    .catch((err) => {
      // Only a definitive rejection ends the session. A backend restart, a timeout or
      // a 5xx must NOT log the user out, the refresh token is still perfectly good.
      const status = (err as { response?: { status?: number } })?.response?.status;
      return { ok: false as const, fatal: status === 401 || status === 403 };
    })
    .finally(() => {
      inFlightRefresh.delete(refreshToken);
    });

  inFlightRefresh.set(refreshToken, pending);
  return pending;
}

/** An error whose message is written for the person (the login rate limit): shown as it is. */
class ShownError extends Error {}

type SignInFields = Partial<Record<"email" | "password" | "challenge" | "code" | "recovery_code", string>>;

/** The credentials sign-in (both steps). Wrong details give null; messages for the person are thrown. */
async function signInStep(credentials: SignInFields | undefined, req: { headers?: HeaderBag } | undefined) {
  const toUser = (data: Record<string, unknown>) => ({
    id: data.user_id as string,
    email: data.email as string,
    name: data.full_name as string,
    access_token: data.access_token as string,
    refresh_token: data.refresh_token as string,
    is_superadmin: data.is_superadmin as boolean,
    is_system_admin: data.is_system_admin as boolean,
    portal_type: data.portal_type as string,
    must_change_password: (data.must_change_password as boolean | undefined) ?? false,
  });
  // A 429 carries a message the person should see ("Try again in N minutes"); next-auth passes a thrown
  // message through as result.error. Wrong credentials or a wrong code stay a plain null.
  const tooMany = (err: unknown) => {
    if (axios.isAxiosError(err) && err.response?.status === 429) {
      const detail = err.response.data?.detail;
      throw new ShownError(typeof detail === "string" && detail ? detail : "Too many login attempts. Try again later.");
    }
  };

  if (credentials?.challenge) {
    const code = credentials.code?.trim();
    const recovery = credentials.recovery_code?.trim();
    if (!code && !recovery) return null;
    try {
      const res = await axios.post(`${API_URL}/api/v1/auth/login/totp`, {
        challenge: credentials.challenge,
        ...(code ? { code } : { recovery_code: recovery }),
      }, { headers: clientIpHeaders(req?.headers) });
      return toUser(res.data);
    } catch (err) {
      tooMany(err);
      if (axios.isAxiosError(err) && err.response?.data?.code === "totp_challenge_expired") {
        throw new Error(TOTP_EXPIRED);
      }
      return null;
    }
  }

  if (!credentials?.email || !credentials?.password) return null;
  let data: Record<string, unknown>;
  try {
    const res = await axios.post(`${API_URL}/api/v1/auth/login`, {
      email: credentials.email,
      password: credentials.password,
    }, { headers: clientIpHeaders(req?.headers) });
    data = res.data;
  } catch (err) {
    tooMany(err);
    return null;
  }
  // The password was right and the account has TOTP on: the page asks for the code next.
  if (data.totp_required && typeof data.challenge === "string") {
    throw new Error(`${TOTP_REQUIRED}${data.challenge}`);
  }
  return toUser(data);
}

/** Errors `signInStep` throws on purpose: their message is what the page shows or reads. */
const meant = (err: unknown, m: string) =>
  err instanceof ShownError || m === TOTP_EXPIRED || m.startsWith(TOTP_REQUIRED);

/**
 * Anything else that goes wrong in a sign-in (13 Nov 2026: an error without a message reached the page as
 * "undefined"): one log line for the server log, never the password, code, challenge or tokens, and an error
 * with a real message so next-auth never hands the page an empty one.
 */
export function signInFailure(err: unknown, step: "password" | "code"): Error {
  const e = err as { name?: string; message?: unknown; response?: { status?: number } } | null;
  const message = typeof e?.message === "string" ? e.message : "";
  if (err instanceof Error && message && meant(err, message)) return err;
  console.error(
    `[auth] sign-in ${step} step failed: ${e?.name ?? typeof err} ${message || "(no message)"}` +
      (e?.response?.status ? ` http ${e.response.status}` : ""),
  );
  return new Error("SignInIncomplete");
}

export const authOptions: NextAuthOptions = {
  providers: [
    CredentialsProvider({
      name: "credentials",
      credentials: {
        email: { label: "Email", type: "email" },
        password: { label: "Password", type: "password" },
        // Two-factor sign-in, step 2 (lib/totpLogin.ts): the challenge from step 1 and a code or a recovery code
        challenge: { label: "Challenge", type: "text" },
        code: { label: "Code", type: "text" },
        recovery_code: { label: "Recovery code", type: "text" },
      },
      async authorize(credentials, req) {
        try {
          return await signInStep(credentials, req);
        } catch (err) {
          throw signInFailure(err, credentials?.challenge ? "code" : "password");
        }
      },
    }),
  ],
  callbacks: {
    async jwt({ token, user }) {
      // Initial sign-in: store tokens + expiry
      if (user) {
        return {
          ...token,
          access_token: user.access_token,
          refresh_token: user.refresh_token,
          user_id: user.id,
          is_superadmin: user.is_superadmin,
          is_system_admin: user.is_system_admin,
          portal_type: user.portal_type,
          must_change_password: user.must_change_password,
          // 5-min buffer so we refresh before the backend rejects the token
          access_token_expires_at: Date.now() + 55 * 60 * 1000,
          error: undefined,
        };
      }

      // Token still valid: return as-is
      if (token.access_token_expires_at && Date.now() < token.access_token_expires_at - 60_000) {
        return token;
      }

      // Access token expired: attempt backend refresh (server-side, with the client's IP)
      const result = await refreshTokens(token.refresh_token, await requestIpHeaders());

      if (result.ok) {
        return {
          ...token,
          access_token: result.access_token,
          refresh_token: result.refresh_token,
          access_token_expires_at: Date.now() + 55 * 60 * 1000,
          error: undefined,
        };
      }

      // Refresh token is revoked or expired, force re-login
      if (result.fatal) {
        return { ...token, error: "RefreshTokenExpired" };
      }

      // Couldn't reach the backend. Keep the session and try again in ~30s rather than
      // logging out over a blip. (The guard above returns early until 60s before this.)
      return { ...token, access_token_expires_at: Date.now() + 90_000, error: undefined };
    },
    async session({ session, token }) {
      // Only what page JavaScript needs. Never the refresh token (see the Session type above).
      session.access_token = token.access_token;
      session.error = token.error;
      session.user = {
        id: token.user_id,
        email: token.email!,
        name: token.name!,
        is_superadmin: token.is_superadmin,
        is_system_admin: token.is_system_admin,
        portal_type: token.portal_type,
        must_change_password: token.must_change_password ?? false,
      };
      return session;
    },
  },
  events: {
    // Revoke the refresh token on the backend when the user signs out (best effort: the
    // cookie is cleared regardless).
    async signOut({ token }) {
      const refresh = (token as { refresh_token?: string } | null)?.refresh_token;
      if (!refresh) return;
      try {
        await axios.post(`${API_URL}/api/v1/auth/logout`, { refresh_token: refresh }, { timeout: 5000 });
      } catch { /* the session cookie is gone either way */ }
    },
  },
  // next-auth's own errors (JWT, session cookie, callbacks) in the server log with a prefix to grep for
  logger: {
    error(code, metadata) {
      const m = metadata as { message?: string; name?: string } | undefined;
      console.error(`[next-auth] ${code} ${m?.name ?? ""} ${m?.message ?? ""}`.trim());
    },
    warn(code) { console.warn(`[next-auth] warn ${code}`); },
    debug() {},
  },
  pages: {
    signIn: "/login",
    error: "/login",
  },
  session: {
    strategy: "jwt",
    maxAge: 30 * 24 * 60 * 60, // 30 days
  },
  secret: process.env.NEXTAUTH_SECRET,
};
