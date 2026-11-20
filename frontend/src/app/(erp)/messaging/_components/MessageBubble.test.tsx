import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }) }));
vi.mock("@/lib/api", () => ({ messagingApi: {} }));

import MessageBubble, { memoCanRecall, type MemoData, type Message } from "./MessageBubble";

const MEMO: MemoData = {
  id: "m1", memo_number: "MEMO/1", subject: "Stocktake", memo_type: "information", body: "Friday",
  sender_id: "me", sender_name: "Me", sender_signed_at: "2026-11-20T08:00:00Z", channel_id: "c1",
  status: "partially_acknowledged",
  acknowledgements: [{ user_id: "u2", name: "Grace", acknowledged_at: "2026-11-20T09:00:00Z" }],
  created_at: "2026-11-20T08:00:00Z",
};

function msg(memo: MemoData): Message {
  return {
    id: "x", sender_id: memo.sender_id, sender_name: "Me", content: "", message_type: "memo",
    created_at: memo.created_at, source: "channel", channel_id: "c1", memo_id: memo.id, memo,
  };
}

describe("memo recall follows the API rule", () => {
  it("the sender may recall until everyone acknowledged", () => {
    expect(memoCanRecall({ sender_id: "me", status: "sent" }, "me")).toBe(true);
    expect(memoCanRecall({ sender_id: "me", status: "partially_acknowledged" }, "me")).toBe(true);
    expect(memoCanRecall({ sender_id: "me", status: "acknowledged" }, "me")).toBe(false);
    expect(memoCanRecall({ sender_id: "me", status: "recalled" }, "me")).toBe(false);
    expect(memoCanRecall({ sender_id: "other", status: "sent" }, "me")).toBe(false);
  });

  it("shows Recall on a partly acknowledged channel memo", () => {
    render(<MessageBubble msg={msg(MEMO)} isMe showAvatar myId="me" />);
    expect(screen.getByRole("button", { name: /Recall/ })).toBeTruthy();
  });

  it("hides it once fully acknowledged", () => {
    render(<MessageBubble msg={msg({ ...MEMO, status: "acknowledged" })} isMe showAvatar myId="me" />);
    expect(screen.queryByRole("button", { name: /Recall/ })).toBeNull();
  });
});
