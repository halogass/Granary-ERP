import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { PanelView, type Panel } from "./DealNaryInsights";
import { GoogleConnect, MeetingsList, type MeetingRow } from "./Meetings";
import { ReportView, SopRevisions, type ReportData } from "./WeeklyReport";

const panel: Panel = {
  scores: [{ id: "s1", at: "2026-10-10T01:00:00Z", budget: 3, authority: 2, need: 3, timeline: 2, total: 10, class: "hot",
             red_flags: [], green_flags: [], rationale: "Budget approved, wants it live next month.", intent: "price", cascaded: true }],
  insights: [
    { id: "i1", kind: "fit_check", status: "draft", at: "2026-10-10T01:00:00Z", content: {
      summary: "Laptops for 15 staff", proposal_trigger: "complex",
      requirements: [{ requirement: "stock alerts", status: "covered", how: "Inventory module" },
                     { requirement: "fleet tracking", status: "optional_custom", how: "custom AI" }] } },
    { id: "i2", kind: "handoff", status: "draft", at: "2026-10-10T01:00:00Z", content: {
      summary: "Won", scope_sold: ["Laptops"], scope_not_sold: ["fleet tracking"],
      success_criteria: [{ quote: "Notification within 1 minute", source: "WhatsApp" }], commitments: [], sensitivities: [],
      red_flags: [], contacts: [], open_items: [] } },
  ],
  cadence: [{ id: "c1", day: 1, channel: "whatsapp", topic: "receipt", intent: "confirm", due_at: "2026-10-11T01:00:00Z", status: "drafted", reason: null },
            { id: "c2", day: 3, channel: "email", topic: "value", intent: "differentiator", due_at: "2026-10-13T01:00:00Z", status: "skipped", reason: "the customer is in touch" }],
  meetings: [{ id: "m1", status: "booked", title: "Your Company x Acme", starts_at: "2026-10-14T03:00:00Z", meet_link: "https://meet.google.com/x", slots: [], picked: 2 }],
};

describe("Nary Phase 3 screens", () => {
  it("shows the score, the fit check, the handoff in the customer's words and the cadence", () => {
    render(<PanelView panel={panel} canApprove={false} />);
    expect(screen.getByText("HOT")).toBeInTheDocument();
    expect(screen.getByText("BANT 10/12")).toBeInTheDocument();
    expect(screen.getByText("checked by a second model")).toBeInTheDocument();
    expect(screen.getByText("optional custom")).toBeInTheDocument();
    expect(screen.getByText(/A Solution Proposal may be needed/)).toBeInTheDocument();
    expect(screen.getByText("Notification within 1 minute")).toBeInTheDocument();
    expect(screen.getByText("the customer is in touch")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Meet" })).toHaveAttribute("href", "https://meet.google.com/x");
    expect(screen.queryByRole("button", { name: /debrief/i })).toBeNull();                 // no nary:approve
  });

  it("sends a typed debrief and asks for meeting times", async () => {
    const onDebrief = vi.fn();
    const onMeeting = vi.fn();
    render(<PanelView panel={panel} canApprove onDebrief={onDebrief} onProposeMeeting={onMeeting} />);
    await userEvent.click(screen.getByRole("button", { name: "Propose meeting times" }));
    expect(onMeeting).toHaveBeenCalled();
    await userEvent.type(screen.getByLabelText("Debrief note"), "They approved the budget");
    await userEvent.click(screen.getByRole("button", { name: "Send debrief" }));
    const form = onDebrief.mock.calls[0][0] as FormData;
    expect(form.get("text")).toBe("They approved the budget");
  });

  it("connects Google only once the credentials are saved", async () => {
    const onSave = vi.fn();
    const onConnect = vi.fn();
    const base = { account: "owner@example.com", status: "not_connected", last_error: null,
                   redirect_uri: "https://erp.test/api/v1/nary/google/callback" };
    const { rerender } = render(<GoogleConnect status={{ ...base, credentials_set: false }} canManage onSave={onSave} onConnect={onConnect} />);
    expect(screen.getByRole("button", { name: /Connect Google/ })).toBeDisabled();
    expect(screen.getByText("https://erp.test/api/v1/nary/google/callback")).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("OAuth client ID"), "cid.apps.googleusercontent.com");
    await userEvent.type(screen.getByLabelText("Client secret"), "s3cret");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(onSave).toHaveBeenCalledWith("cid.apps.googleusercontent.com", "s3cret");
    rerender(<GoogleConnect status={{ ...base, credentials_set: true }} canManage onSave={onSave} onConnect={onConnect} />);
    await userEvent.click(screen.getByRole("button", { name: /Connect Google/ }));
    expect(onConnect).toHaveBeenCalled();
  });

  it("lists meetings and offers the debrief and no-show only after a booking", async () => {
    const onNoShow = vi.fn();
    const base: MeetingRow = { id: "a", status: "slots_sent", title: "Your Company x Acme", chat_id: null, channel: "whatsapp",
      deal_id: "d1", lead_id: null, starts_at: null, meet_link: null, attendee_email: null, customer_tz: null, picked: null,
      slots: [{ start: "2026-10-14T02:00:00Z", company: "Wed 14 Oct 09:00" }, { start: "x", company: "Thu 15 Oct 19:00", extended: true }],
      created_at: "2026-10-10T01:00:00Z" };
    render(<MeetingsList canAct onDebrief={vi.fn()} onNoShow={onNoShow}
      rows={[base, { ...base, id: "b", status: "booked", starts_at: "2026-10-14T02:00:00Z", meet_link: "https://meet.google.com/y" }]} />);
    expect(screen.getByText(/2. Thu 15 Oct 19:00 \(extended\)/)).toBeInTheDocument();
    const noShow = screen.getAllByRole("button", { name: "They didn't show" });
    expect(noShow).toHaveLength(1);
    await userEvent.click(noShow[0]);
    expect(onNoShow).toHaveBeenCalledWith("b");
  });

  it("marks KPIs below target and lists promotion candidates", () => {
    const data: ReportData = {
      week_ending: "2026-10-12", kpis: [{ kpi: "win_rate_pct", value: 30, target: 40, ok: false },
                                        { kpi: "quote_to_decision_days", value: 21, target: 30, ok: true }],
      pipeline: [{ stage: "proposal", currency: "JPY", deals: 2, value: "300000000" }], at_risk: [{ title: "Globex Corp", stage: "proposal" }],
      approval_latency_hours: 3.5, quality: { "whatsapp.reply.send": { decided: 10, unedited: 9, rejected: 0, unedited_pct: 90 } },
      promotion_candidates: [{ action: "whatsapp.reply.send" }], filtered: 4, budget: { spent: "12.00", cap: "100.00" } };
    render(<ReportView data={data} />);
    expect(screen.getByLabelText("below target")).toBeInTheDocument();
    expect(screen.getByLabelText("on target")).toBeInTheDocument();
    expect(screen.getByText("Ready to promote: whatsapp.reply.send")).toBeInTheDocument();
    expect(screen.getByText(/At risk: Globex Corp/)).toBeInTheDocument();
  });

  it("drafts only the SOPs that were ticked", async () => {
    const onDraft = vi.fn();
    render(<SopRevisions canManage onDraft={onDraft} drafts={[]}
      briefs={[{ code: "SOP-001", brief: "BDR is Nary" }, { code: "SOP-002", brief: "New: Nary operating procedure" }]} />);
    await userEvent.click(screen.getByLabelText("Draft SOP-002"));
    await userEvent.click(screen.getByRole("button", { name: /Draft 1 with Nary/ }));
    expect(onDraft).toHaveBeenCalledWith(["SOP-002"]);
  });
});
