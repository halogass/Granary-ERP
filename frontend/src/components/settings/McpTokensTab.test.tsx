import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }) }));
let admin = false;
vi.mock("@/hooks/usePermissions", () => ({ usePermissions: () => ({ isAdmin: admin }) }));
const connections = vi.fn();
const levels = vi.fn();
const createToken = vi.fn();
const revokeConnection = vi.fn();
const totpStatus = vi.fn();
vi.mock("@/lib/api", () => ({
  authApi: { totpStatus: (...a: unknown[]) => totpStatus(...a) },
  mcpApi: {
    connections: (...a: unknown[]) => connections(...a),
    levels: (...a: unknown[]) => levels(...a),
    createToken: (...a: unknown[]) => createToken(...a),
    revokeConnection: (...a: unknown[]) => revokeConnection(...a),
  },
}));

import McpTokensTab, { levelText, scopesNeedPassword } from "./McpTokensTab";

const ROWS = [
  { id: "1", name: "Example Agent", kind: "oauth", token_prefix: null, scopes: ["act", "owner"], user: "Diana",
    last_used_at: null, expires_at: null, revoked: false, created_at: null },
  { id: "2", name: "Laptop", kind: "pat", token_prefix: "gpat_abcdefg", scopes: ["read"], user: "Diana",
    last_used_at: null, expires_at: null, revoked: false, created_at: null },
];

beforeEach(() => {
  admin = false;
  connections.mockReset().mockResolvedValue({ data: { items: ROWS } });
  levels.mockReset().mockResolvedValue({ data: { levels: ["read", "propose", "act"] } });
  createToken.mockReset().mockResolvedValue({ data: { token: "gpat_new", name: "Bot" } });
  revokeConnection.mockReset().mockResolvedValue({});
  totpStatus.mockReset().mockResolvedValue({ data: { enabled: false } });
});

describe("Settings → Integrations & access → MCP access", () => {
  it("names each level; a token from before levels acts as before", () => {
    expect(levelText(["act", "owner"])).toBe("Act as me + Owner powers (old)");
    expect(levelText(["read"])).toBe("Read only");
    expect(levelText(undefined)).toBe("Act as me");
    expect(levelText(["act", "owner", "full"])).toBe("Full power (Owner)");
  });

  it("lists connections with their kind and level, and revokes one", async () => {
    render(<McpTokensTab />);
    expect(await screen.findByText("Example Agent")).toBeTruthy();
    expect(screen.getByText(/logged in by browser · Act as me \+ Owner powers \(old\)/)).toBeTruthy();
    expect(screen.getByText(/personal token · Read only/)).toBeTruthy();
    expect(screen.queryByText("All connections")).toBeNull();
    await userEvent.click(screen.getAllByTitle("Revoke")[0]);
    await waitFor(() => expect(revokeConnection).toHaveBeenCalledWith("1"));
  });

  it("creates a token at the chosen level; full power only when offered", async () => {
    render(<McpTokensTab />);
    await screen.findByText("Example Agent");
    expect(screen.queryByText("Full power (Owner)")).toBeNull();
    await userEvent.selectOptions(screen.getByLabelText("Access level"), "read");
    await userEvent.click(screen.getByRole("button", { name: /Create/ }));
    await waitFor(() => expect(createToken).toHaveBeenCalled());
    expect(createToken.mock.calls[0][0].scopes).toEqual(["read"]);
    expect(await screen.findByText("gpat_new")).toBeTruthy();
  });

  it("an admin can switch to everyone's connections", async () => {
    admin = true;
    levels.mockResolvedValue({ data: { levels: ["read", "propose", "act", "full"] } });
    render(<McpTokensTab />);
    await screen.findByText("Example Agent");
    expect(connections).toHaveBeenLastCalledWith(false);
    await userEvent.click(screen.getByRole("button", { name: "All connections" }));
    await waitFor(() => expect(connections).toHaveBeenLastCalledWith(true));
    expect(await screen.findByText(/Diana · personal token/)).toBeTruthy();
    expect(await screen.findByText("Full power (Owner)")).toBeTruthy();
    expect(screen.queryByText("Owner powers")).toBeNull();
  });

  it("full power is the one Owner level: it fixes the level at act", async () => {
    levels.mockResolvedValue({ data: { levels: ["read", "propose", "act", "full"] } });
    render(<McpTokensTab />);
    await userEvent.click(await screen.findByRole("checkbox", { name: /Full power/ }));
    expect((screen.getByLabelText("Access level") as HTMLSelectElement).disabled).toBe(true);
    await userEvent.type(screen.getByLabelText("Your password"), "s3cret");
    await userEvent.click(screen.getByRole("button", { name: /Create/ }));
    await waitFor(() => expect(createToken).toHaveBeenCalled());
    expect(createToken.mock.calls[0][0].scopes).toEqual(["full"]);
    expect(createToken.mock.calls[0][0].password).toBe("s3cret");
  });

  it("asks for the password only for levels that can act, and never keeps it", async () => {
    expect(scopesNeedPassword(["read"])).toBe(false);
    expect(scopesNeedPassword(["propose"])).toBe(false);
    expect(scopesNeedPassword(["act"])).toBe(true);
    expect(scopesNeedPassword(["full"])).toBe(true);
    render(<McpTokensTab />);
    await screen.findByText("Example Agent");
    // propose (the default) needs no password
    expect(screen.queryByLabelText("Your password")).toBeNull();
    await userEvent.selectOptions(screen.getByLabelText("Access level"), "act");
    const pw = screen.getByLabelText("Your password") as HTMLInputElement;
    expect(pw.type).toBe("password");
    // no password: refused locally, nothing sent
    await userEvent.click(screen.getByRole("button", { name: /Create/ }));
    expect(createToken).not.toHaveBeenCalled();
    expect(screen.getByRole("alert").textContent).toMatch(/Enter your password/);
    expect(screen.getByText(/expire after a limited number of days/)).toBeTruthy();
    expect(screen.getByText(/disconnects all your tokens and AI client connections/)).toBeTruthy();
  });

  it("with two-factor sign-in on, a token that can act also asks the authenticator code", async () => {
    totpStatus.mockResolvedValue({ data: { enabled: true } });
    render(<McpTokensTab />);
    await screen.findByText("Example Agent");
    expect(screen.queryByLabelText("Authenticator code")).toBeNull();
    await userEvent.selectOptions(screen.getByLabelText("Access level"), "act");
    await userEvent.type(screen.getByLabelText("Your password"), "s3cret");
    await userEvent.click(screen.getByRole("button", { name: /Create/ }));
    expect(createToken).not.toHaveBeenCalled();
    expect(screen.getByRole("alert").textContent).toMatch(/authenticator app/);
    await userEvent.type(screen.getByLabelText("Authenticator code"), "123456");
    await userEvent.click(screen.getByRole("button", { name: /Create/ }));
    await waitFor(() => expect(createToken).toHaveBeenCalled());
    expect(createToken.mock.calls[0][0]).toMatchObject({ scopes: ["act"], password: "s3cret", totp_code: "123456" });
    expect((screen.getByLabelText("Authenticator code") as HTMLInputElement).value).toBe("");
  });

  it("shows the server's refusal inline and clears the password", async () => {
    createToken.mockRejectedValue({
      isAxiosError: true,
      response: { status: 403, data: { detail: "Enter your password to create a token that can act (act or full power)." } },
    });
    render(<McpTokensTab />);
    await screen.findByText("Example Agent");
    await userEvent.selectOptions(screen.getByLabelText("Access level"), "act");
    await userEvent.type(screen.getByLabelText("Your password"), "wrong");
    await userEvent.click(screen.getByRole("button", { name: /Create/ }));
    await waitFor(() => expect(createToken).toHaveBeenCalled());
    expect(createToken.mock.calls[0][0]).toMatchObject({ scopes: ["act"], password: "wrong" });
    expect(await screen.findByText("Enter your password to create a token that can act (act or full power).")).toBeTruthy();
    expect((screen.getByLabelText("Your password") as HTMLInputElement).value).toBe("");
  });
});
