import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

import ContractsPage from "./page";

vi.mock("@/lib/api", () => ({
  salesApi: {
    listContracts: () => Promise.resolve({ data: [
      { id: "k1", number: "CTR-0001", status: "draft", contract_type: "sales", value: 5000, currency: "USD",
        auto_renew: false, created_at: "2026-11-01T00:00:00Z" },
    ] }),
  },
  crmApi: { listOrgs: () => Promise.resolve({ data: [] }) },
  systemApi: { getCurrencySettings: () => Promise.resolve({ data: { functional: "USD" } }) },
  attachmentApi: {},
}));
vi.mock("@/lib/entitySources", () => ({ orgSource: {} }));
vi.mock("@/components/ui/AttachmentPanel", () => ({ default: () => null }));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1", email: "a@b.c" } } }) }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/sales/contracts", useSearchParams: () => new URLSearchParams() }));
vi.mock("@/components/approvals/ApprovalPanel", () => ({
  default: ({ entityType, entityId }: { entityType: string; entityId: string }) => <div>approval panel {entityType} {entityId}</div>,
}));

// Batch E: a contract's details carry the approval panel (sending for signing goes through the approval gate),
// like a quotation's page.
describe("contracts approval panel", () => {
  it("shows the approval panel in the contract's details", async () => {
    render(<ContractsPage />);
    const number = await screen.findAllByText("CTR-0001");
    fireEvent.click(number[0]);
    expect(await screen.findByText("approval panel sales_contract k1")).toBeInTheDocument();
  });
});
