// fix-walist: the WhatsApp inbox says when its list is old or missing, never a silent empty list.
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import ChatListNotice from "./ChatListNotice";
import { CHATS_SLOW, chatRetryDelay, clockTime, failedChatList, staleChatList } from "./messages";

describe("chat list problems", () => {
  it("a stale list names the time of the copy", () => {
    const at = new Date(2026, 9, 21, 5, 7).toISOString();
    expect(clockTime(at)).toBe("05:07");
    expect(staleChatList({ headers: { "x-wa-stale": at } })).toEqual({
      kind: "stale", text: "WhatsApp is slow to answer; showing the list from 05:07", retry: true,
    });
    expect(staleChatList({ headers: {} })).toBeNull();
    expect(staleChatList({ headers: { "x-wa-stale": "garbage" } })?.text).toBe("WhatsApp is slow to answer; showing an older list");
  });

  it("a slow service is retried by itself; a refusal is not", () => {
    expect(failedChatList({ response: { status: 503, data: { detail: "WhatsApp is slow to answer; try again in a moment" } } }))
      .toEqual({ kind: "failed", text: CHATS_SLOW, retry: true });
    expect(failedChatList({ response: { status: 504 } }).retry).toBe(true);
    expect(failedChatList(new TypeError("Network Error")).retry).toBe(true);
    expect(failedChatList({ response: { status: 403, data: { detail: "No access to WhatsApp inbox" } } }))
      .toEqual({ kind: "failed", text: "No access to WhatsApp inbox", retry: false });
  });

  it("backs off 5 s, 15 s, then every 30 s", () => {
    expect([0, 1, 2, 3, 9].map(chatRetryDelay)).toEqual([5000, 15000, 30000, 30000, 30000]);
  });
});

describe("ChatListNotice", () => {
  it("shows nothing for a fresh list", () => {
    const { container } = render(<ChatListNotice problem={null} onRetry={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows the stale notice without a Retry button", () => {
    render(<ChatListNotice problem={{ kind: "stale", text: "WhatsApp is slow to answer; showing the list from 05:07", retry: true }}
      onRetry={vi.fn()} />);
    expect(screen.getByRole("status")).toHaveTextContent("showing the list from 05:07");
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("a failed list says why and retries on click", async () => {
    const onRetry = vi.fn();
    render(<ChatListNotice problem={{ kind: "failed", text: CHATS_SLOW, retry: true }} onRetry={onRetry} />);
    expect(screen.getByRole("alert")).toHaveTextContent(CHATS_SLOW);
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });
});
