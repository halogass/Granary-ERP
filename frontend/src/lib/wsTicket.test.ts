import { describe, it, expect, vi, beforeEach } from "vitest";

const wsTicket = vi.fn();
vi.mock("@/lib/api", () => ({ authApi: { wsTicket: (...a: unknown[]) => wsTicket(...a) } }));

import { openTicketedSocket } from "./wsTicket";

const opened: { url: string; close: ReturnType<typeof vi.fn> }[] = [];
class FakeSocket {
  url: string;
  close = vi.fn();
  constructor(url: string) { this.url = url; opened.push(this); }
}

beforeEach(() => {
  opened.length = 0;
  wsTicket.mockReset().mockResolvedValue({ data: { ticket: "t/1+2", expires_in: 30 } });
  vi.stubGlobal("WebSocket", FakeSocket);
});

describe("WebSocket tickets", () => {
  it("opens the socket with a ticket, never the access token", async () => {
    const setup = vi.fn();
    const close = openTicketedSocket("ws://api/api/v1/messaging/ws/u1", setup);
    await vi.waitFor(() => expect(setup).toHaveBeenCalled());
    expect(opened[0].url).toBe("ws://api/api/v1/messaging/ws/u1?ticket=t%2F1%2B2");
    expect(opened[0].url).not.toContain("token=");
    close();
    expect(opened[0].close).toHaveBeenCalled();
  });

  it("adds the ticket to a URL that has a query already", async () => {
    openTicketedSocket("ws://api/x?a=1", () => {});
    await vi.waitFor(() => expect(opened).toHaveLength(1));
    expect(opened[0].url).toBe("ws://api/x?a=1&ticket=t%2F1%2B2");
  });

  it("doesn't open a socket when the page left before the ticket came", async () => {
    let give: (v: unknown) => void = () => {};
    wsTicket.mockReturnValue(new Promise((r) => { give = r; }));
    const setup = vi.fn();
    const close = openTicketedSocket("ws://api/x", setup);
    close();
    give({ data: { ticket: "late" } });
    await new Promise((r) => setTimeout(r, 0));
    expect(opened).toHaveLength(0);
    expect(setup).not.toHaveBeenCalled();
  });
});
