import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { Composer } from "./AssistantThread";

describe("assistant composer", () => {
  it("sends from the button, not only from Enter", async () => {
    const onSend = vi.fn();
    render(<Composer value="hello" onChange={() => {}} onSend={onSend} streaming={false} />);
    await userEvent.click(screen.getByRole("button", { name: /send/i }));
    expect(onSend).toHaveBeenCalledTimes(1);
    // The click event must never reach send() as the message text.
    expect(onSend).toHaveBeenCalledWith();
  });

  it("sends on Enter but not while an IME is composing", () => {
    const onSend = vi.fn();
    render(<Composer value="hello" onChange={() => {}} onSend={onSend} streaming={false} />);
    const box = screen.getByRole("textbox");
    fireEvent.keyDown(box, { key: "Enter", isComposing: true });
    expect(onSend).not.toHaveBeenCalled();
    fireEvent.keyDown(box, { key: "Enter" });
    expect(onSend).toHaveBeenCalledTimes(1);
  });

  it("offers Stop while a reply is streaming", async () => {
    const onStop = vi.fn();
    render(<Composer value="" onChange={() => {}} onSend={() => {}} onStop={onStop} streaming />);
    await userEvent.click(screen.getByRole("button", { name: /stop/i }));
    expect(onStop).toHaveBeenCalledTimes(1);
  });
});
