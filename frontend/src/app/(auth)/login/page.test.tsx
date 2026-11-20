import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const push = vi.fn();
vi.mock("next/navigation", () => ({ useRouter: () => ({ push }) }));
vi.mock("next/link", () => ({ default: ({ href, children, ...p }: { href: string; children: React.ReactNode }) => <a href={href} {...p}>{children}</a> }));
const signIn = vi.fn();
const getSession = vi.fn();
vi.mock("next-auth/react", () => ({
  signIn: (...a: unknown[]) => signIn(...a),
  getSession: (...a: unknown[]) => getSession(...a),
}));
vi.mock("@/hooks/useBranding", () => ({
  useBranding: () => ({
    login_features: "A\nB", login_badge: "Acme", login_headline: "H", login_headline_accent: "A", login_tagline: "T",
    login_footer: "F", login_subtitle: "S", login_copyright: "C",
  }),
}));

import LoginPage from "./page";

async function password() {
  await userEvent.type(screen.getByLabelText("Email address"), "me@example.com");
  await userEvent.type(document.querySelector("input[type=password]") as HTMLInputElement, "pw-12345");
  await userEvent.click(screen.getByRole("button", { name: /^Sign in$/ }));
}

beforeEach(() => {
  push.mockReset();
  signIn.mockReset();
  getSession.mockReset().mockResolvedValue({ user: { portal_type: "internal" } });
  window.history.replaceState(null, "", "/login");
});

describe("Login: two-factor second step", () => {
  it("without TOTP the password signs in as before", async () => {
    signIn.mockResolvedValue({ ok: true, error: null });
    render(<LoginPage />);
    await password();
    await waitFor(() => expect(push).toHaveBeenCalledWith("/dashboard"));
    expect(signIn).toHaveBeenCalledTimes(1);
  });

  it("asks the code after a right password and signs in with the challenge and the code", async () => {
    signIn.mockResolvedValueOnce({ ok: false, error: "TOTP_REQUIRED:the.challenge" })
      .mockResolvedValueOnce({ ok: false, error: "CredentialsSignin" })
      .mockResolvedValueOnce({ ok: true, error: null });
    render(<LoginPage />);
    await password();
    const code = await screen.findByLabelText("Authentication code");
    expect(push).not.toHaveBeenCalled();
    await userEvent.type(code, "000000");
    await userEvent.click(screen.getByRole("button", { name: /Verify and sign in/ }));
    expect(await screen.findByText(/not valid or was already used/)).toBeTruthy();
    expect(signIn.mock.calls[1][1]).toEqual({ challenge: "the.challenge", code: "000000", redirect: false });
    await userEvent.type(screen.getByLabelText("Authentication code"), "123 456");
    await userEvent.click(screen.getByRole("button", { name: /Verify and sign in/ }));
    await waitFor(() => expect(push).toHaveBeenCalledWith("/dashboard"));
    expect(signIn.mock.calls[2][1]).toEqual({ challenge: "the.challenge", code: "123456", redirect: false });
  });

  it("a recovery code instead, and back to an agent's consent page (MCP browser login)", async () => {
    window.history.replaceState(null, "", "/login?callbackUrl=%2Foauth%2Fauthorize%3Fclient_id%3Dmcp_x");
    signIn.mockResolvedValueOnce({ ok: false, error: "TOTP_REQUIRED:ch" }).mockResolvedValueOnce({ ok: true, error: null });
    render(<LoginPage />);
    await password();
    await userEvent.click(await screen.findByRole("button", { name: /Use a recovery code/ }));
    await userEvent.type(screen.getByLabelText("Recovery code"), "abcde-23456");
    await userEvent.click(screen.getByRole("button", { name: /Verify and sign in/ }));
    await waitFor(() => expect(push).toHaveBeenCalledWith("/oauth/authorize?client_id=mcp_x"));
    expect(signIn.mock.calls[1][1]).toEqual({ challenge: "ch", recovery_code: "abcde-23456", redirect: false });
  });

  it("an expired challenge goes back to the password with a message", async () => {
    signIn.mockResolvedValueOnce({ ok: false, error: "TOTP_REQUIRED:ch" }).mockResolvedValueOnce({ ok: false, error: "TOTP_EXPIRED" });
    render(<LoginPage />);
    await password();
    await userEvent.type(await screen.findByLabelText("Authentication code"), "123456");
    await userEvent.click(screen.getByRole("button", { name: /Verify and sign in/ }));
    expect(await screen.findByText(/took too long/)).toBeTruthy();
    expect(screen.getByLabelText("Email address")).toBeTruthy();
    expect(push).not.toHaveBeenCalled();
  });
});
