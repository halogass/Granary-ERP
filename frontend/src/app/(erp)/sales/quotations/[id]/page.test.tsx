import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const getQuotation = vi.fn();
const forEntity = vi.fn();
const withdrawQuotation = vi.fn();
const cancelQuotation = vi.fn();
const repriceQuotation = vi.fn();
const linkQuotationDeal = vi.fn();
const listDeals = vi.fn();
let allowed = true;

vi.mock("next/navigation", () => ({ useParams: () => ({ id: "q1" }), useRouter: () => ({ push: vi.fn() }) }));
vi.mock("next/link", () => ({
  default: ({ href, children }: { href: string; children: React.ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock("@/lib/api", () => ({
  salesApi: {
    getQuotation: (...a: unknown[]) => getQuotation(...a),
    withdrawQuotation: (...a: unknown[]) => withdrawQuotation(...a),
    cancelQuotation: (...a: unknown[]) => cancelQuotation(...a),
    repriceQuotation: (...a: unknown[]) => repriceQuotation(...a),
    linkQuotationDeal: (...a: unknown[]) => linkQuotationDeal(...a),
  },
  crmApi: { listDeals: (...a: unknown[]) => listDeals(...a) },
  financeApi: { listSchedules: () => Promise.resolve({ data: [] }) },
  approvalsApi: { forEntity: (...a: unknown[]) => forEntity(...a) },
}));
vi.mock("@/lib/print", () => ({ printQuotation: vi.fn() }));
vi.mock("@/lib/verifiedDownload", () => ({ downloadVerifiedCopy: vi.fn() }));
vi.mock("@/components/approvals/ApprovalPanel", () => ({ default: () => null }));
vi.mock("@/components/crm/FollowUpButton", () => ({ default: () => null }));
vi.mock("@/components/approvals/useSendDocument", () => ({
  useSendDocument: () => ({ start: vi.fn(), modal: null, preparing: false }),
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("@/hooks/usePermissions", () => ({ usePermissions: () => ({ can: () => allowed }) }));
// the deal picker: one button per deal the source offers
vi.mock("@/components/ui/SearchSelect", async () => {
  const { useEffect, useState } = await import("react");
  return {
    default: function Picker({ source, onChange }: { source: { fetch: (q: string) => Promise<{ id: string; label: string }[]> }; onChange: (id: string, o: null) => void }) {
      const [opts, setOpts] = useState<{ id: string; label: string }[]>([]);
      useEffect(() => { source.fetch("").then(setOpts); }, [source]);
      return <>{opts.map((o) => <button key={o.id} type="button" onClick={() => onChange(o.id, null)}>{`pick ${o.label}`}</button>)}</>;
    },
  };
});

import QuotationDetailPage from "./page";

const quote = (over: Record<string, unknown>) => ({
  id: "q1", number: "Q-1", full_number: "Q-2026-11-00001-1", revision: 1, subject: "Support plan", status: "draft",
  total: 1100000, subtotal: 1000000, tax_amount: 110000, discount_amount: 100000, currency: "JPY",
  issue_date: "2026-11-01", valid_until: "2026-11-15", items: [], discount_note: "Owner approved", discount_display: "pdf",
  ...over,
});

beforeEach(() => {
  getQuotation.mockReset();
  forEntity.mockReset();
  withdrawQuotation.mockReset();
  linkQuotationDeal.mockReset();
  listDeals.mockReset();
  allowed = true;
});

describe("quotation page (1 Nov 2026)", () => {
  it("offers Withdraw while an approval waits, and shows the discount note", async () => {
    getQuotation.mockResolvedValue({ data: quote({}) });
    forEntity.mockResolvedValue({ data: { request: { status: "pending" } } });
    withdrawQuotation.mockResolvedValue({ data: { label: "Withdrew quotation" } });
    render(<QuotationDetailPage />);
    const btn = await screen.findByRole("button", { name: /withdraw from approval/i });
    expect(screen.queryByRole("link", { name: /edit/i })).toBeNull();
    expect(screen.getByText("Owner approved")).toBeInTheDocument();
    await userEvent.click(btn);
    await waitFor(() => expect(withdrawQuotation).toHaveBeenCalledWith("q1"));
  });

  it("offers Edit, Reprice and Cancel on a draft nobody is deciding on", async () => {
    getQuotation.mockResolvedValue({ data: quote({}) });
    forEntity.mockResolvedValue({ data: { request: null } });
    render(<QuotationDetailPage />);
    expect(await screen.findByRole("button", { name: /reprice to today/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /cancel quotation/i })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /withdraw from approval/i })).toBeNull();
  });

  it("says a cancelled quotation is kept for history", async () => {
    getQuotation.mockResolvedValue({ data: quote({ status: "cancelled", cancel_reason: "Replaced", cancelled_at: "2026-11-01T10:00:00Z" }) });
    forEntity.mockResolvedValue({ data: { request: null } });
    render(<QuotationDetailPage />);
    expect(await screen.findByText(/kept for history/i)).toBeInTheDocument();
    expect(screen.getAllByText("Cancelled").length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: /send to client/i })).toBeNull();
  });

  // 5 Nov 2026: a sent quotation is linked to a deal without a revision
  it("links a sent quotation to one of the customer's deals, keeping the deal's value when unticked", async () => {
    getQuotation.mockResolvedValue({ data: quote({ status: "sent", customer_org_id: "org1", deal_id: null }) });
    forEntity.mockResolvedValue({ data: { request: null } });
    listDeals.mockResolvedValue({ data: [{ id: "d1", title: "Globex rollout", deal_number: "DL-1", stage: "proposal" }] });
    linkQuotationDeal.mockResolvedValue({ data: {} });
    render(<QuotationDetailPage />);
    await userEvent.click(await screen.findByRole("button", { name: /link to deal/i }));
    await waitFor(() => expect(listDeals).toHaveBeenCalledWith({ organization_id: "org1", limit: 200, offset: 0 }));
    await userEvent.click(await screen.findByRole("button", { name: "pick Globex rollout" }));
    const box = screen.getByRole("checkbox", { name: /set the deal's value/i });
    expect(box).toBeChecked();
    await userEvent.click(box);
    const dialog = screen.getByRole("dialog");
    const confirm = Array.from(dialog.querySelectorAll("button")).find((b) => b.textContent === "Link to deal")!;
    await userEvent.click(confirm);
    await waitFor(() => expect(linkQuotationDeal).toHaveBeenCalledWith("q1", "d1", false));
  });

  it("offers no Link to deal on a draft, a cancelled or superseded quotation, or without the permission", async () => {
    forEntity.mockResolvedValue({ data: { request: null } });
    for (const status of ["draft", "cancelled", "superseded"]) {
      getQuotation.mockResolvedValue({ data: quote({ status }) });
      const { unmount } = render(<QuotationDetailPage />);
      await screen.findAllByText("Q-2026-11-00001-1");
      expect(screen.queryByRole("button", { name: /link to deal|change deal/i })).toBeNull();
      unmount();
    }
    allowed = false;
    getQuotation.mockResolvedValue({ data: quote({ status: "accepted" }) });
    render(<QuotationDetailPage />);
    await screen.findAllByText("Q-2026-11-00001-1");
    expect(screen.queryByRole("button", { name: /link to deal|change deal/i })).toBeNull();
  });

  it("says Change deal when the quotation has a deal already", async () => {
    getQuotation.mockResolvedValue({ data: quote({ status: "accepted", deal_id: "d0" }) });
    forEntity.mockResolvedValue({ data: { request: null } });
    render(<QuotationDetailPage />);
    expect(await screen.findByRole("button", { name: /change deal/i })).toBeInTheDocument();
  });
});
