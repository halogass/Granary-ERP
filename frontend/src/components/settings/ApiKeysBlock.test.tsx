import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const update = vi.fn();
const testProvider = vi.fn();
vi.mock("@/lib/api", () => ({
  systemApi: {
    updateAiSettings: (...a: unknown[]) => update(...a),
    testAiProvider: (...a: unknown[]) => testProvider(...a),
  },
}));

import ApiKeysBlock from "./ApiKeysBlock";

const PROVIDERS = {
  anthropic: { key_set: false, keyless: false, base_url: "", jobs: ["assistant", "default"] },
  gemini: { key_set: true, keyless: false, base_url: "", jobs: ["embedding"] },
  openai: { key_set: false, keyless: false, base_url: "", jobs: [] },
  deepseek: { key_set: false, keyless: false, base_url: "", jobs: [] },
  ollama: { key_set: false, keyless: true, base_url: "", jobs: [] },
};

beforeEach(() => {
  update.mockReset().mockResolvedValue({ data: { status: "saved" } });
  testProvider.mockReset().mockResolvedValue({ data: { ok: true, model: "claude-opus-5-5", reply: "OK" } });
});

describe("API keys: one row per provider", () => {
  it("says which provider is missing a key its jobs need, and which are saved", () => {
    render(<ApiKeysBlock providers={PROVIDERS} onSaved={() => {}} />);
    expect(screen.getByText("No key: jobs below need it")).toBeTruthy();     // anthropic
    expect(screen.getByText("Key saved")).toBeTruthy();                      // gemini
    expect(screen.getByText("No key needed")).toBeTruthy();                  // ollama
    expect(screen.queryByLabelText("Ollama (self-hosted) API key")).toBeNull();
  });

  it("saves only that provider's key, then can test it", async () => {
    const onSaved = vi.fn();
    render(<ApiKeysBlock providers={PROVIDERS} onSaved={onSaved} />);
    const box = screen.getByLabelText("Anthropic (Claude) API key") as HTMLInputElement;
    expect(box.readOnly).toBe(true);                                      // no browser autofill before a click
    await userEvent.click(box);
    expect(box.readOnly).toBe(false);
    await userEvent.type(box, "sk-ant-123");
    expect(screen.getByText("Not saved yet: Test uses the saved key.")).toBeTruthy();
    await userEvent.click(screen.getAllByRole("button", { name: "Save" })[0]);
    expect(update).toHaveBeenCalledWith({ provider_keys: { anthropic: "sk-ant-123" } });
    expect(onSaved).toHaveBeenCalled();
    await userEvent.click(screen.getAllByRole("button", { name: "Test" })[1]);      // gemini, which has a key
    expect(testProvider).toHaveBeenCalledWith("gemini");
    expect(await screen.findByText(/Works \(claude-opus-5-5\)/)).toBeTruthy();
  });

  it("can't test a provider without a saved key", () => {
    render(<ApiKeysBlock providers={PROVIDERS} onSaved={() => {}} />);
    expect((screen.getAllByRole("button", { name: "Test" })[0] as HTMLButtonElement).disabled).toBe(true);
  });
});
