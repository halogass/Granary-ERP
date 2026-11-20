// fix-walist: a dropped Nary chat stream shows the saved reply (or waits for it) instead of
// "Connection error".
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";

const chatStream = vi.fn();
const getConversation = vi.fn();
const listConversations = vi.fn();
vi.mock("@/lib/api", () => ({
  aiApi: {
    chatStream: (...a: unknown[]) => chatStream(...a),
    getConversation: (...a: unknown[]) => getConversation(...a),
    listConversations: (...a: unknown[]) => listConversations(...a),
    deleteConversation: vi.fn(), renameConversation: vi.fn(), setProposalStatus: vi.fn(),
  },
}));
vi.mock("next/navigation", () => ({ usePathname: () => "/nary/chat" }));

import {
  DROPPED_GAVE_UP, DROPPED_NO_ID, DROPPED_STILL_WORKING, RECOVER_FOR_MS, RECOVER_POLL_MS,
  replySaved, toChatMessages, useAssistantChat,
} from "./useAssistantChat";

type Ev = Record<string, unknown>;
type Handlers = { onEvent: (ev: Ev) => void; signal?: AbortSignal };

const earlier = [
  { role: "user", content: "hi" },
  { role: "assistant", content: "Hello." },
];
const saved = [
  ...earlier,
  { role: "user", content: "the long question" },
  { role: "assistant", content: "The long answer.", cards: [{ type: "records", title: "Deals", items: [] }],
    proposals: [{ proposal_id: "p1", kind: "quotation" }] },
];

/** A stream that sends `events` and then drops (fetch's network error). */
const dropsAfter = (events: Ev[]) => async (_body: unknown, h: Handlers) => {
  for (const ev of events) h.onEvent(ev);
  throw new TypeError("network error");
};

async function openExisting() {
  getConversation.mockResolvedValueOnce({ data: { messages: earlier } });
  const hook = renderHook(() => useAssistantChat());
  await act(async () => { await hook.result.current.openConversation("c1"); });
  expect(hook.result.current.messages).toHaveLength(2);
  return hook;
}

beforeEach(() => {
  listConversations.mockResolvedValue({ data: [] });
});
afterEach(() => {
  vi.useRealTimers();
});

describe("replySaved", () => {
  it("needs the asked question followed by an answer, past what was known", () => {
    expect(replySaved(saved as never, "the long question", 2)).toBe(true);
    expect(replySaved(earlier as never, "the long question", 2)).toBe(false);
    expect(replySaved([...earlier, { role: "user", content: "the long question" }] as never, "the long question", 2)).toBe(false);
    // An older answer to the same question does not count once the server is known to hold it.
    expect(replySaved(earlier as never, "hi", 2)).toBe(false);
    expect(replySaved(earlier as never, "hi", 0)).toBe(true);
  });

  it("keeps records and proposals when it rebuilds the thread", () => {
    const out = toChatMessages("c1", saved as never);
    expect(out[3].cards).toHaveLength(1);
    expect(out[3].proposals?.[0].proposal_id).toBe("p1");
    expect(out[3].id).toBe("server-c1-3");
  });
});

describe("a dropped chat stream", () => {
  it("shows the reply the server saved instead of Connection error", async () => {
    const hook = await openExisting();
    chatStream.mockImplementationOnce(dropsAfter([{ type: "status", text: "Searching" }, { type: "text", text: "The lo" }]));
    getConversation.mockResolvedValueOnce({ data: { messages: saved } });
    await act(async () => { await hook.result.current.send("the long question"); });
    const msgs = hook.result.current.messages;
    expect(msgs.map((m) => m.content)).toEqual(["hi", "Hello.", "the long question", "The long answer."]);
    expect(msgs[3].cards).toHaveLength(1);
    expect(msgs[3].proposals).toHaveLength(1);
    expect(msgs.some((m) => m.content.includes("Connection error"))).toBe(false);
    expect(getConversation).toHaveBeenLastCalledWith("c1");
    expect(hook.result.current.streaming).toBe(false);
  });

  it("says Nary is still working and polls until the reply is saved", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const hook = await openExisting();
    chatStream.mockImplementationOnce(dropsAfter([]));
    getConversation
      .mockResolvedValueOnce({ data: { messages: earlier } })
      .mockResolvedValueOnce({ data: { messages: [...earlier, { role: "user", content: "the long question" }] } })
      .mockResolvedValueOnce({ data: { messages: saved } });
    let done = false;
    act(() => { hook.result.current.send("the long question").then(() => { done = true; }); });
    await waitFor(() => expect(hook.result.current.messages[3]?.content).toContain(DROPPED_STILL_WORKING));
    expect(hook.result.current.status).toBe(DROPPED_STILL_WORKING);
    expect(hook.result.current.streaming).toBe(true);
    await act(async () => { await vi.advanceTimersByTimeAsync(RECOVER_POLL_MS); });
    expect(done).toBe(false);
    await act(async () => { await vi.advanceTimersByTimeAsync(RECOVER_POLL_MS); });
    await waitFor(() => expect(done).toBe(true));
    expect(hook.result.current.messages[3].content).toBe("The long answer.");
    expect(getConversation).toHaveBeenCalledTimes(4); // open + three polls
  });

  it("gives up after three minutes with a clear message", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const hook = await openExisting();
    chatStream.mockImplementationOnce(dropsAfter([{ type: "text", text: "Partial" }]));
    getConversation.mockResolvedValue({ data: { messages: earlier } });
    let done = false;
    act(() => { hook.result.current.send("the long question").then(() => { done = true; }); });
    await waitFor(() => expect(hook.result.current.messages[3]?.content).toContain(DROPPED_STILL_WORKING));
    await act(async () => { await vi.advanceTimersByTimeAsync(RECOVER_FOR_MS + RECOVER_POLL_MS); });
    await waitFor(() => expect(done).toBe(true));
    const last = hook.result.current.messages[3].content;
    expect(last).toContain("Partial");
    expect(last).toContain(DROPPED_GAVE_UP);
    expect(last).not.toContain(DROPPED_STILL_WORKING);
  });

  it("recovers a stream that ended without done", async () => {
    const hook = await openExisting();
    chatStream.mockImplementationOnce(async (_b: unknown, h: Handlers) => { h.onEvent({ type: "text", text: "The" }); });
    getConversation.mockResolvedValueOnce({ data: { messages: saved } });
    await act(async () => { await hook.result.current.send("the long question"); });
    expect(hook.result.current.messages[3].content).toBe("The long answer.");
  });

  it("a new chat dropped before its id arrived asks to reload the chat list", async () => {
    const { result } = renderHook(() => useAssistantChat());
    chatStream.mockImplementationOnce(dropsAfter([{ type: "status", text: "Thinking" }]));
    await act(async () => { await result.current.send("new question"); });
    expect(result.current.messages[1].content).toContain(DROPPED_NO_ID);
    expect(getConversation).not.toHaveBeenCalled();
  });

  it("a new chat keeps the id from the first event that names it", async () => {
    const { result } = renderHook(() => useAssistantChat());
    chatStream.mockImplementationOnce(dropsAfter([{ type: "status", text: "x", conversation_id: "new-1" }]));
    getConversation.mockResolvedValueOnce({ data: { messages: [
      { role: "user", content: "new question" }, { role: "assistant", content: "Saved." }] } });
    await act(async () => { await result.current.send("new question"); });
    expect(getConversation).toHaveBeenCalledWith("new-1");
    expect(result.current.conversationId).toBe("new-1");
    expect(result.current.messages.map((m) => m.content)).toEqual(["new question", "Saved."]);
  });

  it("Stop is still a cancel, and a refused turn is not recovered", async () => {
    const hook = await openExisting();
    chatStream.mockImplementationOnce(async (_b: unknown, h: Handlers) => {
      await new Promise((_, reject) => h.signal?.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError"))));
    });
    let p: Promise<void> = Promise.resolve();
    act(() => { p = hook.result.current.send("q1"); });
    act(() => hook.result.current.stop());
    await act(async () => { await p; });
    expect(hook.result.current.messages[3].content).toBe("*(stopped)*");

    chatStream.mockImplementationOnce(async () => { throw new Error("AI chat failed (403)"); });
    await act(async () => { await hook.result.current.send("q2"); });
    expect(hook.result.current.messages[5].content).toContain("Connection error");
    expect(getConversation).toHaveBeenCalledTimes(1); // only the open
  });

  it("a turn refused because this chat is still busy says so", async () => {
    const hook = await openExisting();
    const busy = "Nary is still working on your last message in this chat. Wait for the answer, or start a new chat.";
    chatStream.mockImplementationOnce(async () => {
      throw Object.assign(new Error("AI chat failed (409)"), { status: 409, detail: busy });
    });
    await act(async () => { await hook.result.current.send("same decisions again"); });
    expect(hook.result.current.messages[3].content).toContain(busy);
    expect(hook.result.current.messages[3].content).not.toContain("Connection error");
    expect(getConversation).toHaveBeenCalledTimes(1); // not recovered: nothing was started
  });
});
