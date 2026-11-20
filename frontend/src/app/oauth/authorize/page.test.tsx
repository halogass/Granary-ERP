import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

let params = new URLSearchParams();
vi.mock("next/navigation", () => ({ useSearchParams: () => params }));
const oauthClient = vi.fn();
const oauthAuthorize = vi.fn();
const totpStatus = vi.fn();
vi.mock("@/lib/api", () => ({
  mcpApi: {
    oauthClient: (...a: unknown[]) => oauthClient(...a),
    oauthAuthorize: (...a: unknown[]) => oauthAuthorize(...a),
  },
  authApi: { totpStatus: (...a: unknown[]) => totpStatus(...a) },
}));

import OAuthAuthorizePage from "./page";

const assign = vi.fn();
const REDIRECT = "http://127.0.0.1:33418/callback";

function link(extra: Record<string, string> = {}) {
  params = new URLSearchParams({
    response_type: "code", client_id: "mcp_abc", redirect_uri: REDIRECT, code_challenge: "c".repeat(43),
    code_challenge_method: "S256", state: "xyz", ...extra,
  });
}

beforeEach(() => {
  oauthClient.mockReset();
  totpStatus.mockReset().mockResolvedValue({ data: { enabled: false } });
  oauthAuthorize.mockReset().mockResolvedValue({ data: { redirect_to: `${REDIRECT}?code=one&state=xyz` } });
  assign.mockReset();
  Object.defineProperty(window, "location", { value: { ...window.location, assign }, writable: true });
});

describe("MCP browser login: the consent page", () => {
  it("offers only the levels this person may give, starting from one that can't act", async () => {
    link({ scope: "act owner" });
    oauthClient.mockResolvedValue({ data: { client_name: "Example Agent", redirect_host: "127.0.0.1:33418",
                                            levels: ["read", "propose", "act"] } });
    render(<OAuthAuthorizePage />);
    expect(await screen.findByText("Example Agent")).toBeTruthy();
    expect(screen.queryByText("Owner powers")).toBeNull();
    // stage 8: the agent asked for act, but nothing that can act is picked for the person
    expect((screen.getByDisplayValue("act") as HTMLInputElement).checked).toBe(false);
    expect((screen.getByDisplayValue("propose") as HTMLInputElement).checked).toBe(true);
    expect(screen.queryByLabelText("Your password")).toBeNull();
    await userEvent.click(screen.getByDisplayValue("read"));
    await userEvent.click(screen.getByRole("button", { name: /Allow/ }));
    await waitFor(() => expect(assign).toHaveBeenCalledWith(`${REDIRECT}?code=one&state=xyz`));
    expect(oauthAuthorize.mock.calls[0][0]).toMatchObject({ client_id: "mcp_abc", scopes: ["read"], state: "xyz" });
  });

  it("never ticks full power for an agent that asks for it or the old Owner powers", async () => {
    link({ scope: "act owner full" });
    oauthClient.mockResolvedValue({ data: { client_name: "My AI client", redirect_host: "x", levels: ["read", "propose", "act", "full"] } });
    render(<OAuthAuthorizePage />);
    expect(((await screen.findByRole("checkbox", { name: /Full power/ })) as HTMLInputElement).checked).toBe(false);
    expect(screen.queryByText("Owner powers")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: /Allow/ }));
    await waitFor(() => expect(oauthAuthorize).toHaveBeenCalled());
    expect(oauthAuthorize.mock.calls[0][0].scopes).toEqual(["propose"]);
    expect(oauthAuthorize.mock.calls[0][0].password).toBeUndefined();
  });

  it("gives full power when the approver ticks it and types the password and the code", async () => {
    link();
    totpStatus.mockResolvedValue({ data: { enabled: true } });
    oauthClient.mockResolvedValue({ data: { client_name: "My AI client", redirect_host: "x",
                                            levels: ["read", "propose", "act", "full"] } });
    render(<OAuthAuthorizePage />);
    await userEvent.click(await screen.findByRole("checkbox", { name: /Full power/ }));
    await userEvent.click(screen.getByRole("button", { name: /Allow/ }));
    expect(await screen.findByText(/Enter your password/)).toBeTruthy();
    expect(oauthAuthorize).not.toHaveBeenCalled();
    await userEvent.type(screen.getByLabelText("Your password"), "s3cret-pass");
    await userEvent.click(screen.getByRole("button", { name: /Allow/ }));
    expect(await screen.findByText(/Enter the code from your authenticator app/)).toBeTruthy();
    expect(oauthAuthorize).not.toHaveBeenCalled();
    await userEvent.type(screen.getByLabelText("Authenticator code"), " 123456 ");
    await userEvent.click(screen.getByRole("button", { name: /Allow/ }));
    await waitFor(() => expect(oauthAuthorize).toHaveBeenCalled());
    expect(oauthAuthorize.mock.calls[0][0]).toMatchObject({ scopes: ["full"], password: "s3cret-pass", totp_code: "123456" });
  });

  it("asks only the password for act when two-factor sign-in is off", async () => {
    link();
    oauthClient.mockResolvedValue({ data: { client_name: "Example Agent", redirect_host: "x", levels: ["read", "propose", "act"] } });
    render(<OAuthAuthorizePage />);
    await userEvent.click(await screen.findByDisplayValue("act"));
    expect(screen.queryByLabelText("Authenticator code")).toBeNull();
    await userEvent.type(screen.getByLabelText("Your password"), "pw-1");
    await userEvent.click(screen.getByRole("button", { name: /Allow/ }));
    await waitFor(() => expect(oauthAuthorize).toHaveBeenCalled());
    expect(oauthAuthorize.mock.calls[0][0]).toMatchObject({ scopes: ["act"], password: "pw-1" });
    expect(oauthAuthorize.mock.calls[0][0].totp_code).toBeUndefined();
  });

  it("Deny sends access_denied back to the agent", async () => {
    link();
    oauthClient.mockResolvedValue({ data: { client_name: "Example Agent", redirect_host: "x", levels: ["read", "propose", "act"] } });
    render(<OAuthAuthorizePage />);
    await userEvent.click(await screen.findByRole("button", { name: "Deny" }));
    const url = new URL(assign.mock.calls[0][0]);
    expect(url.searchParams.get("error")).toBe("access_denied");
    expect(url.searchParams.get("state")).toBe("xyz");
    expect(oauthAuthorize).not.toHaveBeenCalled();
  });

  it("refuses an incomplete link without asking the server", async () => {
    link({ code_challenge: "" });
    render(<OAuthAuthorizePage />);
    expect(await screen.findByText(/incomplete/)).toBeTruthy();
    expect(oauthClient).not.toHaveBeenCalled();
  });
});
