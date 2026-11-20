import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const decide = vi.fn();
const getCard = vi.fn();
vi.mock("@/lib/api", () => ({ naryApi: { decide: (...a: unknown[]) => decide(...a), card: (...a: unknown[]) => getCard(...a) } }));

import CardView from "./CardView";
import type { NaryCard } from "./types";

const card = (over: Partial<NaryCard> = {}): NaryCard => ({
  id: "c1", action: "crm.record_change", label: "Create or update a CRM record", origin: "nary", status: "pending",
  level: 1, customer_facing: false, payload: { object_type: "lead", action: "update", record_id: "l1", fields: { priority: "high" } },
  edited_payload: null, rationale: "Government buyer, ready this quarter", sop_ref: "SOP-001", sources: [],
  diff: { label: "Lead X", before: { priority: "medium" }, after: { priority: "high" } }, price: null,
  entity_type: "lead", entity_id: "l1", run_id: null, created_at: "2026-10-10T08:00:00Z", decided_at: null,
  reject_reason: null, edit_ratio: null, result: null, error: null, actual_text: null, diff_ratio: null, ...over,
});

beforeEach(() => decide.mockReset());

describe("Nary approval card", () => {
  it("shows why and what changes, and applies", async () => {
    decide.mockResolvedValue({ data: card({ status: "applied" }) });
    const done = vi.fn();
    render(<CardView card={card()} onDone={done} />);
    expect(screen.getByText("Government buyer, ready this quarter")).toBeInTheDocument();
    expect(screen.getByText("medium")).toBeInTheDocument();        // the before value, struck through
    await userEvent.click(screen.getByRole("button", { name: /^apply$/i }));
    expect(decide).toHaveBeenCalledWith("c1", { decision: "apply", edited_payload: undefined, reason: undefined });
    expect(done).toHaveBeenCalledWith(expect.objectContaining({ status: "applied" }));
  });

  it("asks why before rejecting", async () => {
    decide.mockResolvedValue({ data: card({ status: "rejected", reject_reason: "Wrong lead" }) });
    render(<CardView card={card()} onDone={() => {}} />);
    await userEvent.click(screen.getByRole("button", { name: /reject/i }));
    const confirm = screen.getAllByRole("button", { name: /reject/i }).at(-1)!;
    expect(confirm).toBeDisabled();
    await userEvent.type(screen.getByPlaceholderText(/why/i), "Wrong lead");
    await userEvent.click(confirm);
    expect(decide).toHaveBeenCalledWith("c1", { decision: "reject", edited_payload: undefined, reason: "Wrong lead" });
  });

  it("says a customer-facing draft is never sent", () => {
    render(<CardView card={card({ customer_facing: true, status: "shadow", payload: { text: "Hello" } })} onDone={() => {}} />);
    expect(screen.getByText(/a draft only: nothing was sent/i)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /apply/i })).not.toBeInTheDocument();
  });

  it("a message scheduled for the morning can be sent now or dropped", async () => {
    decide.mockResolvedValue({ data: card({ status: "applied" }) });
    render(<CardView card={card({ status: "scheduled", action: "whatsapp.reply.send", customer_facing: true,
                                  payload: { channel: "whatsapp", chat_id: "1@c.us", text: "Hello" },
                                  result: { when: "Wed 08:00" } })} onDone={() => {}} />);
    expect(screen.getByText(/goes out Wed 08:00/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /drop/i })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /send now/i }));
    expect(decide).toHaveBeenCalledWith("c1", { decision: "apply", edited_payload: undefined, reason: undefined });
  });

  // fix-longapply
  it("a failed proposal card can be tried again", async () => {
    decide.mockResolvedValue({ status: 200, data: card({ action: "proposal.from_chat", status: "applied" }) });
    const done = vi.fn();
    render(<CardView card={card({ action: "proposal.from_chat", status: "failed", error: "The work stopped before it finished" })} onDone={done} />);
    expect(screen.getByText(/the work stopped/i)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /try again/i }));
    expect(decide).toHaveBeenCalledWith("c1", { decision: "apply", edited_payload: undefined, reason: undefined });
    expect(done).toHaveBeenCalledWith(expect.objectContaining({ status: "applied" }));
  });

  it("a failed card of another kind has no Try again", () => {
    render(<CardView card={card({ status: "failed", error: "No" })} onDone={() => {}} />);
    expect(screen.queryByRole("button", { name: /try again/i })).not.toBeInTheDocument();
  });

  it("says it is working while a long apply goes on, then shows the result", async () => {
    decide.mockResolvedValue({ status: 202, data: { ...card({ status: "applying" }), card_id: "c1", still_working: true } });
    getCard.mockResolvedValue({ data: card({ status: "applied" }) });
    const done = vi.fn();
    render(<CardView card={card()} onDone={done} />);
    await userEvent.click(screen.getByRole("button", { name: /^apply$/i }));
    expect(await screen.findByText(/working\.\.\. this can take a few minutes/i)).toBeInTheDocument();
    await vi.waitFor(() => expect(done).toHaveBeenCalledWith(expect.objectContaining({ status: "applied" })), { timeout: 6000 });
    expect(getCard).toHaveBeenCalledWith("c1");
  }, 10000);

  // 21 Nov 2026: an agent's (MCP) card showed its proposal as raw JSON
  it("an agent's message card shows the text and its fields, not JSON (cards saved before the fix too)", () => {
    const proposal = { kind: "customer_message", channel: "email", to: "hiroshi@clinic.example.com", cc: [], bcc: [],
                       subject: "Re: Waiting-room furniture", text: "Dear Hiroshi,\n\nYes, in grey.", summary: "Send email",
                       thread_id: "t1", contact_id: "c9" };
    render(<CardView card={card({ action: "mcp.proposal", origin: "mcp", customer_facing: false,
      payload: { proposal, client: "Desktop AI client", proposer_id: "u1" },
      diff: { label: "Send email (by Desktop AI client for Kenji)", after: { proposal } } })} onDone={() => {}} />);
    expect(screen.getByText(/Yes, in grey\./)).toBeInTheDocument();
    expect(screen.getByText("Re: Waiting-room furniture")).toBeInTheDocument();
    expect(screen.getByText("hiroshi@clinic.example.com")).toBeInTheDocument();
    expect(screen.queryByText(/"kind"/)).not.toBeInTheDocument();
    expect(screen.queryByText(/thread/i)).not.toBeInTheDocument();
    expect(screen.queryByText("Cc")).not.toBeInTheDocument();   // empty: left out
  });

  it("an agent's merge card shows the fields Nary's own merge card shows", () => {
    render(<CardView card={card({ action: "mcp.proposal", origin: "mcp",
      payload: { proposal: { kind: "nary_action", action: "organization.merge", payload: { keep_id: "o1", merge_ids: ["o2"] } }, proposer_id: "u1" },
      diff: { label: "Merge (by agent for Kenji)", after: { moves: { contacts: 2 }, fills: ["tax_id"] } } })} onDone={() => {}} />);
    expect(screen.getByText("tax_id")).toBeInTheDocument();
    expect(screen.getByText("Contacts:")).toBeInTheDocument();
    expect(screen.queryByText(/\{"/)).not.toBeInTheDocument();
  });

  it("an agent's quotation card lists its lines", () => {
    const proposal = { kind: "quotation", total: 697400, currency: "JPY", customer: "Hoshino Design Office",
                       payload: { subject: "Chairs", currency: "JPY", items: [{ name: "Aero mesh task chair", quantity: 12, amount: 456000 }] } };
    render(<CardView card={card({ action: "mcp.proposal", origin: "mcp", payload: { proposal, proposer_id: "u1" },
      diff: { label: "Quotation", after: { proposal } } })} onDone={() => {}} />);
    expect(screen.getByText("12 × Aero mesh task chair")).toBeInTheDocument();
    expect(screen.getByText("Hoshino Design Office")).toBeInTheDocument();
  });

  it("the kickoff card shows the project, PM, customer, tasks and handoff summary without nulls", () => {
    render(<CardView card={card({ action: "handoff.kickoff", payload: { deal_id: "d1" },
      diff: { label: "Start the project for Hoshino meeting rooms", after: {
        project: "Hoshino meeting rooms", customer: "Hoshino Design Office", pm: "Aiko Tanaka",
        kickoff_tasks: ["Kickoff meeting with the customer", "Site survey"],
        handoff_summary: "Two meeting rooms, AV and furniture.", handoff: { red_flags: ["Tight deadline"] } } } })} onDone={() => {}} />);
    expect(screen.getByText("Hoshino meeting rooms")).toBeInTheDocument();
    expect(screen.getByText("Aiko Tanaka")).toBeInTheDocument();
    expect(screen.getByText("Hoshino Design Office")).toBeInTheDocument();
    expect(screen.getByText("Site survey")).toBeInTheDocument();
    expect(screen.getByText("Two meeting rooms, AV and furniture.")).toBeInTheDocument();
    expect(screen.getByText("Tight deadline")).toBeInTheDocument();
    expect(screen.queryByText(/null/)).not.toBeInTheDocument();
  });

  it("a kickoff card saved before the fix shows its package summary, not the raw package", () => {
    render(<CardView card={card({ action: "handoff.kickoff", payload: { deal_id: "d1" },
      diff: { label: "Start the project for Dental clinic supplies 2027", after: { package: {
        summary: "Reviewed: nothing unusual found.", contacts: null, red_flags: null, open_items: null, scope_sold: null,
        commitments: null, sensitivities: null, scope_not_sold: null, success_criteria: null } } } })} onDone={() => {}} />);
    expect(screen.getByText("Dental clinic supplies 2027")).toBeInTheDocument();
    expect(screen.getByText("Reviewed: nothing unusual found.")).toBeInTheDocument();
    expect(screen.queryByText(/null/)).not.toBeInTheDocument();
    expect(screen.queryByText("package")).not.toBeInTheDocument();
  });

  // 21 Nov 2026 follow-ups: titles, readable labels, money in the card's currency, quantities without ".0"
  it("an agent's quotation card has a title, the total in its currency and whole quantities", () => {
    const proposal = { kind: "quotation", total: 697400, currency: "JPY", customer: "Hoshino Design Office",
                       payload: { subject: "Chairs", currency: "JPY", items: [{ name: "Aero mesh task chair", quantity: 12.0 }] } };
    render(<CardView card={card({ action: "mcp.proposal", origin: "mcp", label: "A change an outside agent proposed (MCP)",
      payload: { proposal, proposer_id: "u1" }, diff: { label: "Quotation", after: { proposal } } })} onDone={() => {}} />);
    expect(screen.getByText("Quotation draft for Hoshino Design Office")).toBeInTheDocument();
    expect(screen.queryByText(/outside agent proposed/)).not.toBeInTheDocument();
    expect(screen.getByText("¥697,400")).toBeInTheDocument();
    expect(screen.getByText("12 × Aero mesh task chair")).toBeInTheDocument();
    expect(screen.getByText("Total")).toBeInTheDocument();
  });

  it("an agent's card uses the API's title and readable field labels, and shows the reason it gave", () => {
    render(<CardView card={card({ action: "mcp.proposal", origin: "mcp", label: "A change an outside agent proposed (MCP)",
      rationale: "They asked twice about grey chairs\nProposed by Desktop AI client for Kenji (MCP).",
      payload: { proposal: { kind: "customer_message", text: "Dear Hiroshi" }, proposer_id: "u1" },
      diff: { label: "Send email", title: "Email reply to Hiroshi Haddad",
              after: { channel: "email", to: "hiroshi@clinic.example.com", from_inbox: "sales@example.com" } } })}
      onDone={() => {}} />);
    expect(screen.getByText("Email reply to Hiroshi Haddad")).toBeInTheDocument();
    for (const label of ["To", "Channel", "From inbox"]) expect(screen.getByText(label)).toBeInTheDocument();
    expect(screen.queryByText("from_inbox")).not.toBeInTheDocument();
    expect(screen.getByText(/They asked twice about grey chairs/)).toBeInTheDocument();
  });

  it("a record change formats money in the record's currency", () => {
    render(<CardView card={card({ payload: { object_type: "deal", action: "update", record_id: "d1", fields: { value: 3400000 } },
      diff: { label: "Printer fleet", before: { value: 3980000 }, after: { value: 3400000 }, currency: "JPY" } })}
      onDone={() => {}} />);
    expect(screen.getByText("¥3,980,000")).toBeInTheDocument();
    expect(screen.getByText("¥3,400,000")).toBeInTheDocument();
    expect(screen.getByText("Value")).toBeInTheDocument();
  });

  it("a record change saved without a currency still groups the digits", () => {
    render(<CardView card={card({ diff: { label: "Deal", before: { value: 3980000 }, after: { value: 3400000 } } })} onDone={() => {}} />);
    expect(screen.getByText("3,400,000")).toBeInTheDocument();
  });
});
