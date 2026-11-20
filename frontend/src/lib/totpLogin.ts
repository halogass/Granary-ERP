/**
 * Two-factor sign-in (TOTP, 2 Nov 2026) through NextAuth's credentials provider.
 *
 * Step 1 posts the email and password. For an account with TOTP on, the backend answers only after a right
 * password, with a short-lived signed challenge instead of tokens; `authorize` throws it as the error
 * `TOTP_REQUIRED:<challenge>`, which next-auth hands back to the page as `result.error`. Step 2 calls
 * signIn again with `{ challenge, code }` or `{ challenge, recovery_code }`.
 */

export const TOTP_REQUIRED = "TOTP_REQUIRED:";
/** The challenge ran out (or the password changed meanwhile): start again from the password. */
export const TOTP_EXPIRED = "TOTP_EXPIRED";

/** The challenge carried by a step-1 error, or null for any other error. */
export function challengeFromError(error: string | null | undefined): string | null {
  if (!error || !error.startsWith(TOTP_REQUIRED)) return null;
  const c = error.slice(TOTP_REQUIRED.length).trim();
  return c || null;
}

/** What a step-2 error means to the person. */
export function secondStepMessage(error: string, recovery: boolean): string {
  if (error === TOTP_EXPIRED) return "The sign-in took too long. Enter your email and password again.";
  if (error === "CredentialsSignin") {
    return recovery
      ? "That recovery code is not valid or was already used."
      : "That code is not valid or was already used. Wait for the next code and try again.";
  }
  return readableError(error);
}

/** Shown when the sign-in fails without a message a person can use (13 Nov 2026: "undefined" was shown). */
export const SIGN_IN_INCOMPLETE =
  "Sign-in did not complete. Please try again. If it keeps happening, enter your email and password again.";

/**
 * A sign-in error as the person should read it. Messages written for people (the rate limit's "Too many login
 * attempts. Try again in N minutes.") are sentences and pass through; next-auth's bare codes ("Configuration",
 * "Callback", "AccessDenied") and empty or "undefined"/"null" errors become SIGN_IN_INCOMPLETE.
 */
export function readableError(error: string | null | undefined): string {
  const e = (error ?? "").trim();
  if (!e || e === "undefined" || e === "null" || !/\s/.test(e)) return SIGN_IN_INCOMPLETE;
  return e;
}

/** What a step-1 (email and password) error means to the person. */
export function firstStepMessage(error: string): string {
  return error === "CredentialsSignin" ? "Invalid email or password." : readableError(error);
}

/** The page to open after signing in: the portal's home, or a same-site `callbackUrl` (an agent's consent page). */
export function afterLoginPath(portalType: string | undefined, search: string, fallback = "/dashboard"): string {
  if (portalType === "vendor") return "/vendor/dashboard";
  if (portalType === "partner") return "/partner/dashboard";
  const next = new URLSearchParams(search).get("callbackUrl") ?? "";
  return next.startsWith("/") && !next.startsWith("//") && !next.startsWith("/\\") ? next : fallback;
}
