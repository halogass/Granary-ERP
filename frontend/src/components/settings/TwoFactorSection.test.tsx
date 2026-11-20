import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const totpStatus = vi.fn();
const totpSetup = vi.fn();
const totpConfirm = vi.fn();
const totpDisable = vi.fn();
const totpRegenerate = vi.fn();
vi.mock("@/lib/api", () => ({
  authApi: {
    totpStatus: (...a: unknown[]) => totpStatus(...a),
    totpSetup: (...a: unknown[]) => totpSetup(...a),
    totpConfirm: (...a: unknown[]) => totpConfirm(...a),
    totpDisable: (...a: unknown[]) => totpDisable(...a),
    totpRegenerate: (...a: unknown[]) => totpRegenerate(...a),
  },
}));

import TwoFactorSection, { groupSecret } from "./TwoFactorSection";

const OFF = { enabled: false, pending: false, confirmed_at: null, recovery_codes_left: 0, required: false };
const ON = { enabled: true, pending: false, confirmed_at: "2026-11-02T10:00:00Z", recovery_codes_left: 10, required: false };
const CODES = ["abcde-fghjk", "mnpqr-stuvw", "xyz23-45678", "aaaaa-bbbbb", "ccccc-ddddd",
  "eeeee-fffff", "ggggg-hhhhh", "jjjjj-kkkkk", "mmmmm-nnnnn", "ppppp-qqqqq"];

beforeEach(() => {
  totpStatus.mockReset().mockResolvedValue({ data: OFF });
  totpSetup.mockReset().mockResolvedValue({ data: {
    secret: "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP", otpauth_uri: "otpauth://totp/x", issuer: "Granary",
    account: "me@example.com", qr_data_uri: "data:image/png;base64,AAAA",
  } });
  totpConfirm.mockReset().mockResolvedValue({ data: { enabled: true, recovery_codes: CODES } });
  totpDisable.mockReset().mockResolvedValue({ data: { enabled: false } });
  totpRegenerate.mockReset().mockResolvedValue({ data: { recovery_codes: CODES } });
});

describe("Profile > Security: two-factor sign-in", () => {
  it("turns on with the QR and a code, then shows the recovery codes once with copy and download", async () => {
    render(<TwoFactorSection />);
    // stage 8: the current password first
    const turnOn = (await screen.findByRole("button", { name: /Turn on/ })) as HTMLButtonElement;
    expect(turnOn.disabled).toBe(true);
    await userEvent.type(screen.getByLabelText("Your password"), "my-pass");
    await userEvent.click(turnOn);
    expect(totpSetup).toHaveBeenCalledWith("my-pass");
    const qr = await screen.findByAltText("QR code for your authenticator app");
    expect(qr.getAttribute("src")).toBe("data:image/png;base64,AAAA");
    expect(screen.getByLabelText("Setup key").textContent).toBe(groupSecret("JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP"));
    // no code, no confirm
    expect((screen.getByRole("button", { name: /Confirm and turn on/ }) as HTMLButtonElement).disabled).toBe(true);
    totpStatus.mockResolvedValue({ data: ON });
    await userEvent.type(screen.getByLabelText("2. Code from the app"), "123456");
    await userEvent.click(screen.getByRole("button", { name: /Confirm and turn on/ }));
    await waitFor(() => expect(totpConfirm).toHaveBeenCalledWith("123456"));
    const list = await screen.findByRole("list", { name: "Recovery codes" });
    expect(list.querySelectorAll("li")).toHaveLength(10);
    expect(screen.getByText(/not shown again/)).toBeTruthy();
    expect(screen.getByRole("button", { name: /Copy/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /Download/ })).toBeTruthy();
    expect(screen.queryByAltText("QR code for your authenticator app")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: /I saved them/ }));
    expect(screen.queryByRole("list", { name: "Recovery codes" })).toBeNull();
    expect(screen.getByText("On")).toBeTruthy();
  });

  it("shows the server's refusal of a wrong code and stays off", async () => {
    totpConfirm.mockRejectedValue({ response: { status: 403, data: {
      detail: "That code is not valid or was already used. Wait for the next code and try again.", code: "totp_invalid" } } });
    render(<TwoFactorSection />);
    await userEvent.type(await screen.findByLabelText("Your password"), "my-pass");
    await userEvent.click(await screen.findByRole("button", { name: /Turn on/ }));
    await userEvent.type(await screen.findByLabelText("2. Code from the app"), "000000");
    await userEvent.click(screen.getByRole("button", { name: /Confirm and turn on/ }));
    expect(await screen.findByText(/not valid or was already used/)).toBeTruthy();
    expect(screen.queryByRole("list", { name: "Recovery codes" })).toBeNull();
  });

  it("when on: turning off and new recovery codes both need a code", async () => {
    totpStatus.mockResolvedValue({ data: ON });
    render(<TwoFactorSection />);
    expect(await screen.findByText(/10 recovery codes left/)).toBeTruthy();
    const off = screen.getByRole("button", { name: /Turn off/ }) as HTMLButtonElement;
    const regen = screen.getByRole("button", { name: /New recovery codes/ }) as HTMLButtonElement;
    expect(off.disabled && regen.disabled).toBe(true);
    await userEvent.type(screen.getByLabelText("Code from your app"), "654321");
    await userEvent.click(screen.getByRole("button", { name: /New recovery codes/ }));
    await waitFor(() => expect(totpRegenerate).toHaveBeenCalledWith("654321"));
    await userEvent.click(await screen.findByRole("button", { name: /I saved them/ }));
    totpStatus.mockResolvedValue({ data: OFF });
    await userEvent.type(screen.getByLabelText("Code from your app"), "abcde-fghjk");
    await userEvent.click(screen.getByRole("button", { name: /Turn off/ }));
    await waitFor(() => expect(totpDisable).toHaveBeenCalledWith("abcde-fghjk"));
    expect(await screen.findByText(/Two-factor sign-in is off/)).toBeTruthy();
  });

  it("tells a person whose role approves sensitive actions that it is required", async () => {
    totpStatus.mockResolvedValue({ data: { ...OFF, required: true } });
    render(<TwoFactorSection />);
    expect(await screen.findByText(/approves sensitive actions/)).toBeTruthy();
  });
});
