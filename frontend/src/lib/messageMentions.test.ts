import { describe, it, expect } from "vitest";
import { mentionsToPlainText } from "./messageMentions";

const DEAL = "2dac934f-1111-4222-8333-444455556666";
const USER = "9b1c2d3e-aaaa-4bbb-8ccc-ddddeeeeffff";

describe("mention tokens in plain-text places (previews, notifications)", () => {
  it("uses the stored names", () => {
    const text = `Check <#deal:${DEAL}> with <@user:${USER}>`;
    expect(mentionsToPlainText(text, [{ type: "deal", id: DEAL, name: "Hoshino CCTV" }, { type: "user", id: USER, name: "Grace" }]))
      .toBe("Check #Hoshino CCTV with @Grace");
  });

  it("never shows a raw token, even without names", () => {
    expect(mentionsToPlainText(`See <#deal:${DEAL}> and <@user:${USER}>`)).toBe("See #deal and @user");
  });

  it("drops a token the server's preview clip cut in half", () => {
    expect(mentionsToPlainText(`Look at <#deal:2dac934f-11`)).toBe("Look at ");
  });

  it("leaves other text alone", () => {
    expect(mentionsToPlainText("a < b and c > d")).toBe("a < b and c > d");
    expect(mentionsToPlainText(null)).toBe("");
  });
});
