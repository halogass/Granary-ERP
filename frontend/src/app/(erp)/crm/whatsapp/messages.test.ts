import { describe, it, expect } from "vitest";
import {
  applyAck, hasMoreFromHeaders, mergeLatestMessages, mergeOlderMessages, sameMessages, tickState, type ListMessage,
} from "./messages";

const m = (id: string, timestamp: number, extra: Partial<ListMessage> = {}): ListMessage =>
  ({ id, timestamp, fromMe: false, ...extra });

describe("loading older messages", () => {
  it("puts the older page in front, oldest first", () => {
    const current = [m("c", 30), m("d", 40)];
    const older = [m("b", 20), m("a", 10)];
    expect(mergeOlderMessages(current, older).map((x) => x.id)).toEqual(["a", "b", "c", "d"]);
  });

  it("never shows a message twice or pulls in newer ones", () => {
    const current = [m("c", 30), m("d", 40)];
    const older = [m("b", 20), m("c", 30), m("z", 50)];
    expect(mergeOlderMessages(current, older).map((x) => x.id)).toEqual(["b", "c", "d"]);
  });

  it("an empty page changes nothing", () => {
    const current = [m("c", 30)];
    expect(mergeOlderMessages(current, [])).toEqual(current);
  });
});

describe("refreshing the latest page", () => {
  it("keeps the older pages the user loaded", () => {
    const current = [m("a", 10), m("b", 20), m("c", 30)];
    const latest = [m("c", 30), m("d", 40)];
    expect(mergeLatestMessages(current, latest).map((x) => x.id)).toEqual(["a", "b", "c", "d"]);
  });

  it("drops optimistic bubbles the refresh replaces", () => {
    const current = [m("a", 10), m("opt-1", 45, { fromMe: true })];
    const latest = [m("real", 45, { fromMe: true })];
    expect(mergeLatestMessages(current, latest).map((x) => x.id)).toEqual(["a", "real"]);
  });
});

describe("ticks", () => {
  it("follow WhatsApp's states", () => {
    expect(tickState(m("x", 1))).toBe("none");
    expect(tickState(m("opt-1", 1, { fromMe: true }))).toBe("pending");
    expect(tickState(m("x", 1, { fromMe: true, ack: 0 }))).toBe("pending");
    expect(tickState(m("x", 1, { fromMe: true, ack: 1 }))).toBe("sent");
    expect(tickState(m("x", 1, { fromMe: true, ack: 2 }))).toBe("delivered");
    expect(tickState(m("x", 1, { fromMe: true, ack: 3 }))).toBe("read");
    expect(tickState(m("x", 1, { fromMe: true, ack: 4 }))).toBe("read");
    expect(tickState(m("x", 1, { fromMe: true }))).toBe("sent");
  });

  it("only move forward", () => {
    const list = [m("x", 1, { fromMe: true, ack: 3 }), m("y", 2, { fromMe: true, ack: 1 })];
    expect(applyAck(list, "x", 2)).toBe(list);                       // late "delivered" ignored
    expect(applyAck(list, "y", 3).map((x) => x.ack)).toEqual([3, 3]);
    expect(applyAck(list, "missing", 3)).toBe(list);
  });

  it("a tick change counts as a changed list", () => {
    const a = [m("x", 1, { fromMe: true, ack: 1 })];
    expect(sameMessages(a, [m("x", 1, { fromMe: true, ack: 1 })])).toBe(true);
    expect(sameMessages(a, [m("x", 1, { fromMe: true, ack: 3 })])).toBe(false);
    expect(sameMessages(a, [])).toBe(false);
  });
});

describe("more chats", () => {
  it("trusts the server over the page length", () => {
    expect(hasMoreFromHeaders({ headers: { "x-has-more": "1" } }, 32, 50)).toBe(true);   // short page, more exist
    expect(hasMoreFromHeaders({ headers: { "x-has-more": "0" } }, 50, 50)).toBe(false);
  });

  it("falls back to a full page when the header is missing", () => {
    expect(hasMoreFromHeaders({ headers: {} }, 50, 50)).toBe(true);
    expect(hasMoreFromHeaders({}, 12, 50)).toBe(false);
  });
});
