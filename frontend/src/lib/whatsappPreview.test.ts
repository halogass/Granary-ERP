import { describe, it, expect } from "vitest";
import { previewLabel } from "./whatsappPreview";
import { plainTopicText } from "@/components/nary/OwnerTopics";

describe("WhatsApp list preview", () => {
  it("names a system message instead of showing WhatsApp's type", () => {
    expect(previewLabel("notification_template")).toBe("WhatsApp notice");
    expect(previewLabel("ptt")).toBe("Voice note");
    expect(previewLabel("something_new")).toBe("Message");
    expect(previewLabel("chat")).toBe("No messages");
  });
});

describe("Nary overview topics", () => {
  it("shows a topic's WhatsApp text without its bold and italic marks", () => {
    expect(plainTopicText("*Wrap-up* **Problems:** _none_ today")).toBe("Wrap-up Problems: none today");
    expect(plainTopicText("2 * 3 = 6")).toBe("2 * 3 = 6");
  });
});
