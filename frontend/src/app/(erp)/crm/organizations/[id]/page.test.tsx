import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const getOrg = vi.fn();
const updateOrg = vi.fn();
const me = vi.fn();

vi.mock("@/lib/api", () => ({
  crmApi: {
    getOrg: (...a: unknown[]) => getOrg(...a),
    updateOrg: (...a: unknown[]) => updateOrg(...a),
    enrichOrg: vi.fn(),
  },
  authApi: { me: (...a: unknown[]) => me(...a) },
}));
// The record shell is tested elsewhere: here the facts the page gives it.
vi.mock("@/components/crm/RecordLayout", () => ({
  default: ({ facts }: { facts?: React.ReactNode }) => <div>{facts}</div>,
}));
vi.mock("@/components/crm/Research", () => ({
  ResearchButton: () => null, ResearchPanel: () => null, useResearch: () => ({}),
}));

import OrganizationRecordPage from "./page";

beforeEach(() => {
  me.mockResolvedValue({ data: { permissions: ["crm:edit_contact"] } });
  getOrg.mockResolvedValue({ data: { id: "o1", name: "Globex Corp", quote_without_tax: false, custom: {} } });
  updateOrg.mockResolvedValue({ data: { id: "o1" } });
});

describe("Organization page (2 Nov 2026)", () => {
  it("marks a customer as quoted without tax", async () => {
    render(<OrganizationRecordPage params={{ id: "o1" }} />);
    const toggle = await screen.findByRole("switch", { name: "Quote without tax" });
    expect(toggle).toHaveAttribute("aria-checked", "false");
    await waitFor(() => expect(toggle).toBeEnabled());
    await userEvent.click(toggle);
    await waitFor(() => expect(updateOrg).toHaveBeenCalledWith("o1", { quote_without_tax: true }));
    await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "true"));
  });
});
