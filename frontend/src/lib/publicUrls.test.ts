import { describe, expect, it } from "vitest";
import { apiWsBase, signalWsUrl } from "./publicUrls";

const https = { protocol: "https:", host: "erp.example.com" };
const http = { protocol: "http:", host: "localhost:3000" };

describe("publicUrls", () => {
  it("uses a baked API URL when the build set one", () => {
    expect(apiWsBase("https://api.example.com/", https)).toBe("wss://api.example.com");
    expect(apiWsBase("http://localhost:8000", http)).toBe("ws://localhost:8000");
  });
  it("falls back to the page origin for a prebuilt image", () => {
    expect(apiWsBase(undefined, https)).toBe("wss://erp.example.com");
    expect(apiWsBase("", http)).toBe("ws://localhost:3000");
  });
  it("returns an empty origin while rendering on the server", () => {
    expect(apiWsBase(undefined, null)).toBe("");
  });
  it("derives the signalling URL from the origin unless one is baked in", () => {
    expect(signalWsUrl(undefined, https)).toBe("wss://erp.example.com/ws/signal");
    expect(signalWsUrl("wss://calls.example.com/ws/signal", https)).toBe("wss://calls.example.com/ws/signal");
  });
});
