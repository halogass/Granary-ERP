import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import SupportPage from "./page";

// 20 Nov 2026: the public support form reads the public team and category lists (no sign-in);
// the staff lists need helpdesk:view and came back empty for visitors.
describe("public support form", () => {
  const fetchMock = vi.fn();

  beforeEach(() => {
    fetchMock.mockReset();
    fetchMock.mockImplementation((url: string) => {
      const body = url.startsWith("/api/v1/helpdesk/public/teams")
        ? [{ id: "t1", name: "Billing" }]
        : [];
      return Promise.resolve({ ok: true, json: () => Promise.resolve(body) });
    });
    vi.stubGlobal("fetch", fetchMock);
  });
  afterEach(() => vi.unstubAllGlobals());

  it("loads the teams from the public endpoint", async () => {
    render(<SupportPage />);
    expect(await screen.findByRole("option", { name: "Billing" })).toBeInTheDocument();
    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    const urls = fetchMock.mock.calls.map((c) => String(c[0]));
    expect(urls).toContain("/api/v1/helpdesk/public/teams");
    expect(urls.some((u) => u === "/api/v1/helpdesk/teams" || u.startsWith("/api/v1/helpdesk/categories"))).toBe(false);
  });
});
