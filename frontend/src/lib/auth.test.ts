import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

const post = vi.fn();
let axiosErrors = false;
vi.mock("axios", () => ({
  default: { post: (...a: unknown[]) => post(...a), isAxiosError: () => axiosErrors },
}));

import { authOptions, clientIpHeaders } from "./auth";

type Authorize = (c: Record<string, string>, req: { headers: Record<string, string> }) => Promise<unknown>;
const authorize = (authOptions.providers[0] as unknown as { options: { authorize: Authorize } }).options.authorize;

const LOGIN = {
  user_id: "u1", email: "a@example.com", full_name: "A", access_token: "acc", refresh_token: "ref",
  is_superadmin: false, is_system_admin: false, portal_type: "staff", must_change_password: false,
};

beforeEach(() => {
  axiosErrors = false;
  post.mockReset().mockResolvedValue({ data: LOGIN });
  delete process.env.INTERNAL_AUTH_TOKEN;
});
afterEach(() => { delete process.env.INTERNAL_AUTH_TOKEN; });

describe("session callback", () => {
  it("hands page JavaScript the access token but never the refresh token", async () => {
    const session = await authOptions.callbacks!.session!({
      session: { expires: "x" } as never,
      token: {
        access_token: "acc", refresh_token: "ref", user_id: "u1", email: "a@example.com", name: "A",
        is_superadmin: false, is_system_admin: false, portal_type: "staff", must_change_password: false,
        access_token_expires_at: 0,
      } as never,
      user: undefined as never,
      newSession: undefined,
      trigger: "update",
    } as never) as unknown as Record<string, unknown>;
    expect(session.access_token).toBe("acc");
    expect("refresh_token" in session).toBe(false);
    expect(JSON.stringify(session)).not.toContain("ref\"");
  });

  it("the JWT (server side) still keeps the refresh token for the refresh", async () => {
    const token = await authOptions.callbacks!.jwt!({
      token: {} as never, user: { id: "u1", ...LOGIN } as never, account: null,
    } as never) as unknown as Record<string, unknown>;
    expect(token.refresh_token).toBe("ref");
  });
});

describe("authorize: the client's IP reaches the backend", () => {
  it("sends X-Client-IP and X-Internal-Auth from x-real-ip when the token is set", async () => {
    process.env.INTERNAL_AUTH_TOKEN = "s3cret";
    await authorize({ email: "a@example.com", password: "pw" }, { headers: { "x-real-ip": "203.0.113.7" } });
    expect(post).toHaveBeenCalledTimes(1);
    const [url, body, config] = post.mock.calls[0];
    expect(url).toMatch(/\/api\/v1\/auth\/login$/);
    expect(body).toEqual({ email: "a@example.com", password: "pw" });
    expect(config.headers).toEqual({ "X-Client-IP": "203.0.113.7", "X-Internal-Auth": "s3cret" });
  });

  it("sends nothing extra without the token", async () => {
    await authorize({ email: "a@example.com", password: "pw" }, { headers: { "x-real-ip": "203.0.113.7" } });
    expect(post.mock.calls[0][2].headers).toEqual({});
  });

  it("ignores a client-supplied X-Forwarded-For", () => {
    process.env.INTERNAL_AUTH_TOKEN = "s3cret";
    expect(clientIpHeaders({ "x-forwarded-for": "1.2.3.4" })).toEqual({});
    expect(clientIpHeaders(new Headers({ "x-real-ip": "198.51.100.2" })))
      .toEqual({ "X-Client-IP": "198.51.100.2", "X-Internal-Auth": "s3cret" });
  });
});

describe("authorize: two-factor sign-in (TOTP)", () => {
  it("a right password on an account with TOTP gives the challenge as the error, no session", async () => {
    post.mockResolvedValue({ data: { totp_required: true, challenge: "ch.al.lenge", expires_in: 300 } });
    await expect(authorize({ email: "a@example.com", password: "pw" }, { headers: {} }))
      .rejects.toThrow("TOTP_REQUIRED:ch.al.lenge");
  });

  it("the second step posts the challenge and the code, and signs in", async () => {
    const user = await authorize({ challenge: "ch", code: " 123456 " }, { headers: {} }) as Record<string, unknown>;
    const [url, body] = post.mock.calls[0];
    expect(url).toMatch(/\/api\/v1\/auth\/login\/totp$/);
    expect(body).toEqual({ challenge: "ch", code: "123456" });
    expect(user.access_token).toBe("acc");
  });

  it("a recovery code goes as recovery_code; nothing typed sends nothing", async () => {
    await authorize({ challenge: "ch", recovery_code: "abcde-23456" }, { headers: {} });
    expect(post.mock.calls[0][1]).toEqual({ challenge: "ch", recovery_code: "abcde-23456" });
    post.mockClear();
    expect(await authorize({ challenge: "ch", code: "" }, { headers: {} })).toBeNull();
    expect(post).not.toHaveBeenCalled();
  });

  it("a wrong code is a plain failure, an expired challenge and the rate limit say so", async () => {
    axiosErrors = true;
    post.mockRejectedValue({ response: { status: 401, data: { code: "totp_invalid" } } });
    expect(await authorize({ challenge: "ch", code: "000000" }, { headers: {} })).toBeNull();
    post.mockRejectedValue({ response: { status: 401, data: { code: "totp_challenge_expired" } } });
    await expect(authorize({ challenge: "ch", code: "000000" }, { headers: {} })).rejects.toThrow("TOTP_EXPIRED");
    post.mockRejectedValue({ response: { status: 429, data: { detail: "Too many login attempts. Try again in 1 minutes." } } });
    await expect(authorize({ challenge: "ch", code: "000000" }, { headers: {} })).rejects.toThrow(/Too many login attempts/);
  });
});

describe("authorize: an unexpected failure (13 Nov 2026)", () => {
  it("is logged without secrets and thrown with a message, never an empty one", async () => {
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    post.mockResolvedValue({ data: null });
    await expect(authorize({ email: "a@example.com", password: "s3cret!" }, { headers: {} })).rejects.toThrow("SignInIncomplete");
    expect(log).toHaveBeenCalledTimes(1);
    const line = String(log.mock.calls[0][0]);
    expect(line).toMatch(/^\[auth\] sign-in password step failed: TypeError/);
    expect(line).not.toContain("s3cret!");
    log.mockRestore();
  });

  it("the rate limit and the TOTP challenge still pass through untouched", async () => {
    post.mockResolvedValue({ data: { totp_required: true, challenge: "ch1" } });
    await expect(authorize({ email: "a@example.com", password: "p" }, { headers: {} })).rejects.toThrow("TOTP_REQUIRED:ch1");
  });
});
