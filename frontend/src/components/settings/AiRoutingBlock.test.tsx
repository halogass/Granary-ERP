import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("@/lib/api", () => ({ systemApi: { updateAiSettings: vi.fn(), testAiJob: vi.fn() } }));

import AiRoutingBlock, { SELECT_MIN } from "./AiRoutingBlock";

describe("Model per job", () => {
  it("gives the Provider, Effort and Cache selects room for their values", () => {
    render(<AiRoutingBlock onSaved={() => {}} providers={{}}
      routing={{ default: { provider: "anthropic", model: "claude-sonnet-5-5", effort: "medium", max_tokens: 4000, cache_ttl: "1h" } }} />);
    expect(screen.getByLabelText("Provider for default")).toHaveClass(SELECT_MIN.provider);
    expect(screen.getByLabelText("Effort for default")).toHaveClass(SELECT_MIN.effort);
    expect(screen.getByLabelText("Cache for default")).toHaveClass(SELECT_MIN.cache);
    // Room for "anthropic" + the chevron padding (pr-9) at text-xs: at least 8rem.
    expect(parseFloat(SELECT_MIN.provider.match(/\[([\d.]+)rem\]/)![1])).toBeGreaterThanOrEqual(8);
  });
});
