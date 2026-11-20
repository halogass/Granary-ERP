import { describe, it, expect } from "vitest";
import { SIGN_IN_INCOMPLETE, TOTP_EXPIRED, firstStepMessage, readableError, secondStepMessage } from "./totpLogin";

describe("sign-in errors as the person reads them (13 Nov 2026: 'undefined' was shown)", () => {
  it("an empty, 'undefined' or bare next-auth code becomes the plain message", () => {
    for (const e of [undefined, null, "", "undefined", "null", "Configuration", "Callback", "SignInIncomplete"]) {
      expect(readableError(e)).toBe(SIGN_IN_INCOMPLETE);
    }
    expect(secondStepMessage("undefined", false)).toBe(SIGN_IN_INCOMPLETE);
    expect(firstStepMessage("undefined")).toBe(SIGN_IN_INCOMPLETE);
  });

  it("a sentence from the server passes through", () => {
    const t = "Too many login attempts. Try again in 1 minutes.";
    expect(secondStepMessage(t, false)).toBe(t);
    expect(firstStepMessage(t)).toBe(t);
  });

  it("known codes keep their own text", () => {
    expect(firstStepMessage("CredentialsSignin")).toBe("Invalid email or password.");
    expect(secondStepMessage("CredentialsSignin", false)).toMatch(/code is not valid/);
    expect(secondStepMessage("CredentialsSignin", true)).toMatch(/recovery code is not valid/);
    expect(secondStepMessage(TOTP_EXPIRED, false)).toMatch(/took too long/);
  });
});
