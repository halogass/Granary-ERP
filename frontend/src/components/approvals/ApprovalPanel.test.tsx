import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const forEntity = vi.fn();
vi.mock("@/lib/api", () => ({ approvalsApi: { forEntity: (...a: unknown[]) => forEntity(...a) } }));

import ApprovalPanel from "./ApprovalPanel";

const policy = { id: "p1", name: "Quotations", sop_reference: "SOP-SAL-001", signature_flow: "none", qr_enabled: false, flags: [] };
const request = (steps: unknown[]) => ({
  id: "r1", entity_type: "quotation", entity_label: "Quotation", entity_id: "q1", document_label: "QUO-1",
  document_revision: 1, link: "/sales/quotations/q1", status: "approved", submitted_by: { id: "u1", name: "Kenji" },
  submitted_at: null, completed_at: null, current_step_seq: null, flags: [], amount: null, currency: null,
  amount_base: null, steps, actions: [], can_act: false, can_cancel: false,
});

describe("Approval panel (21 Nov 2026: no empty band when opened)", () => {
  it("says there is nothing to show instead of an empty body", async () => {
    forEntity.mockResolvedValue({ data: { policy, gate: "not_required", request: null, history: [] } });
    const { container } = render(<ApprovalPanel entityType="quotation" entityId="q1" />);
    await userEvent.click(await screen.findByRole("button", { name: /approval/i }));
    expect(screen.getByText("No approval steps for this document.")).toBeInTheDocument();
    expect(container.querySelector(".p-4.border-t")).toBeNull();
  });

  it("an approved request without steps leaves no empty step list", async () => {
    forEntity.mockResolvedValue({ data: { policy, gate: "approved", request: request([]), history: [] } });
    const { container } = render(<ApprovalPanel entityType="quotation" entityId="q1" />);
    await userEvent.click(await screen.findByRole("button", { name: /approval/i }));
    expect(container.querySelector("ol")).toBeNull();
  });
});
