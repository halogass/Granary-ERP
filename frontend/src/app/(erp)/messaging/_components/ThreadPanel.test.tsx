import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

const getThread = vi.fn();
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }) }));
vi.mock("@/lib/api", () => ({ messagingApi: { getThread: (...a: unknown[]) => getThread(...a) } }));
vi.mock("./Composer", () => ({ default: () => null }));

import ThreadPanel from "./ThreadPanel";
import MessageBubble, { type Message } from "./MessageBubble";

// jsdom has no layout
Element.prototype.scrollIntoView = vi.fn();

const DEAL = "2dac934f-1111-4222-8333-444455556666";

const root: Message = {
  id: "r1", sender_id: "u1", sender_name: "Grace", content: `Status of <#deal:${DEAL}>?`, message_type: "text",
  mentions: [{ type: "deal", id: DEAL, name: "Hoshino CCTV" }], created_at: "2026-11-20T08:00:00Z", source: "channel", channel_id: "c1",
};

describe("the thread panel renders message text like the channel view", () => {
  it("shows the deal chip on the root message and on replies, never the raw token", async () => {
    getThread.mockResolvedValue({ data: [{ ...root, id: "r2", content: `Moved <#deal:${DEAL}> to won`, mentions: [] }] });
    render(<ThreadPanel rootMessage={root} myId="me" onClose={() => {}} onSend={async () => {}} />);
    expect(screen.getByRole("button", { name: "#Hoshino CCTV" })).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "#deal" })).toBeInTheDocument();   // a reply without names
    expect(screen.queryByText(/<#deal:/)).not.toBeInTheDocument();
  });

  it("matches the channel bubble's chip", () => {
    render(<MessageBubble msg={root} isMe={false} showAvatar myId="me" />);
    expect(screen.getByRole("button", { name: "#Hoshino CCTV" })).toBeInTheDocument();
  });
});
