import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import AttributePanel, { type AttributeDef, type AiFieldRun } from "./AttributePanel";

const runAiField = vi.fn();

vi.mock("@/lib/api", () => ({
  crmRecordsApi: { runAiField: (...a: unknown[]) => runAiField(...a) },
}));

const textDef: AttributeDef = {
  id: "1", object_type: "organization", key: "account_owner", label: "Account Owner",
  field_type: "text", options: {}, is_ai: false, ai_config: {}, sort_order: 0, is_active: true,
};

const selectDef: AttributeDef = {
  ...textDef, id: "2", key: "region", label: "Region",
  field_type: "select", options: { choices: ["North", "South"] },
};

const aiDef: AttributeDef = {
  ...textDef, id: "3", key: "icp_tier", label: "ICP Tier",
  field_type: "select", options: { choices: ["Tier 1", "Tier 2"] },
  is_ai: true, ai_kind: "classify",
};

const run: AiFieldRun = {
  value: "Tier 1", confidence: 95,
  reasoning: "Regional manufacturer with an active deal.",
  sources: [], model: "qwen3.5:4b", at: "2026-09-03T05:44:23Z",
};

function renderPanel(overrides: Partial<Parameters<typeof AttributePanel>[0]> = {}) {
  const onSave = vi.fn().mockResolvedValue(undefined);
  const onRan = vi.fn();
  render(
    <AttributePanel
      defs={[textDef, selectDef, aiDef]}
      values={{ account_owner: "Robert", region: "North", icp_tier: "Tier 1" }}
      runs={{ icp_tier: run }}
      objectType="organization"
      recordId="org-1"
      onSave={onSave}
      onRan={onRan}
      {...overrides}
    />,
  );
  return { onSave, onRan };
}

beforeEach(() => {
  runAiField.mockReset();
  runAiField.mockResolvedValue({ data: { value: "Tier 2" } });
});

describe("AttributePanel", () => {
  it("prompts to define attributes when there are none", () => {
    render(<AttributePanel defs={[]} values={{}} runs={{}} objectType="organization"
      recordId="org-1" onSave={vi.fn()} onRan={vi.fn()} />);
    expect(screen.getByText(/no custom attributes yet/i)).toBeInTheDocument();
  });

  it("renders each attribute's label and current value", () => {
    renderPanel();
    expect(screen.getByText("Account Owner")).toBeInTheDocument();
    expect(screen.getByText("Robert")).toBeInTheDocument();
    expect(screen.getByText("North")).toBeInTheDocument();
  });

  it("badges AI attributes with their confidence", () => {
    renderPanel();
    expect(screen.getByText("95%")).toBeInTheDocument();
    expect(screen.getByText(/Regional manufacturer/i)).toBeInTheDocument();
  });

  it("saves an edited text value", async () => {
    const { onSave } = renderPanel();
    await userEvent.click(screen.getByRole("button", { name: "Robert" }));
    const input = screen.getByDisplayValue("Robert");
    await userEvent.clear(input);
    await userEvent.type(input, "Rachel");
    await userEvent.click(screen.getByRole("button", { name: /save account owner/i }));
    await waitFor(() => expect(onSave).toHaveBeenCalled());
    expect(onSave).toHaveBeenCalledWith("account_owner", "Rachel");
  });

  it("offers only the defined choices for a select", async () => {
    renderPanel();
    await userEvent.click(screen.getByRole("button", { name: "North" }));
    const options = screen.getAllByRole("option").map((o) => o.textContent);
    expect(options).toEqual(["-", "North", "South"]);
  });

  it("does not let an AI attribute be typed into", () => {
    renderPanel();
    // The AI value is text, not an editable trigger button.
    expect(screen.queryByRole("button", { name: "Tier 1" })).not.toBeInTheDocument();
    expect(screen.getByText("Tier 1")).toBeInTheDocument();
  });

  it("runs an AI attribute on demand and notifies the parent", async () => {
    const { onRan } = renderPanel();
    const runBtn = screen.getByTitle(/recompute with ai/i);
    await userEvent.click(runBtn);
    await waitFor(() => expect(runAiField).toHaveBeenCalledWith("organization", "org-1", "icp_tier"));
    await waitFor(() => expect(onRan).toHaveBeenCalled());
  });

  it("shows why an AI run failed", async () => {
    runAiField.mockRejectedValue({
      response: { data: { detail: "The monthly AI budget ($5.00) is used up" } },
    });
    renderPanel();
    await userEvent.click(screen.getByTitle(/recompute with ai/i));
    expect(await screen.findByText(/monthly AI budget/i)).toBeInTheDocument();
  });
});
