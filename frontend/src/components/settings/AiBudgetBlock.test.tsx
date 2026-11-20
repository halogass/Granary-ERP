import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const update = vi.fn();
const usage = vi.fn();
vi.mock("@/lib/api", () => ({
  systemApi: { updateAiSettings: (...a: unknown[]) => update(...a), aiUsage: () => usage() },
}));

import AiBudgetBlock from "./AiBudgetBlock";

const BUDGET = { monthly_usd: "100", warn_pct: "80", stop_pct: "100", hard_stop_pct: "150",
                 essential_jobs: ["nary.reply_draft"], job_caps: {} };
const PRICING = { "claude-opus-5-5": { input: 4, output: 20, cache_read: 0.2, cache_write: 5 } };

beforeEach(() => {
  update.mockReset().mockResolvedValue({ data: { status: "saved" } });
  usage.mockReset().mockResolvedValue({ data: {
    since: "2026-10-01", total: { calls: 12, cost: 85, input_tokens: 1, output_tokens: 1, cache_read_tokens: 0, cache_write_tokens: 0 },
    by_job: [], by_model: [], by_day: [], unpriced_calls: 0, cache_hit_pct: 40,
    budget: { cap: 100, spent: 85, pct: 85, level: "warn", warn_pct: 80, stop_pct: 100, hard_stop_pct: 150 },
  } });
});

describe("AI budget settings", () => {
  it("shows the month against the cap and saves every number from the form", async () => {
    render(<AiBudgetBlock budget={BUDGET} pricing={PRICING} batchDiscount="50" jobs={["assistant", "nary.reply_draft"]} onSaved={() => {}} />);
    expect(await screen.findByText("Nearly spent")).toBeInTheDocument();
    expect(screen.getByText("$85.00 of $100.00")).toBeInTheDocument();
    const capInput = screen.getByLabelText(/monthly cap/i);
    await userEvent.clear(capInput);
    await userEvent.type(capInput, "250");
    await userEvent.click(screen.getByRole("button", { name: "assistant" }));      // make it essential
    await userEvent.click(screen.getByRole("button", { name: /save budget/i }));
    expect(update).toHaveBeenCalledWith(expect.objectContaining({
      budget: expect.objectContaining({ monthly_usd: "250", essential_jobs: ["nary.reply_draft", "assistant"] }),
      pricing: PRICING, batch_discount_pct: "50",
    }));
  });
});
