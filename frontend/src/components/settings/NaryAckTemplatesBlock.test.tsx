import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const update = vi.fn();
vi.mock("@/lib/api", () => ({ systemApi: { updateSetting: (...a: unknown[]) => update(...a) } }));

import NaryAckTemplatesBlock from "./NaryAckTemplatesBlock";

const T = {
  in_hours: "Bonjour {name}, merci.", after_hours: "Bonjour {name}, nous répondrons demain.", late_prefix: "Désolé.",
  name_fallback: "Madame, Monsieur", reply_window: "2 heures ouvrées", email_subject: "Merci",
};
const VALUE = JSON.stringify({ fr: T, en: { ...T, in_hours: "Hi {name}, thanks." } });

beforeEach(() => { update.mockReset().mockResolvedValue({ data: {} }); });

describe("Acknowledgement texts", () => {
  it("edits a language's text and saves the whole setting as JSON", async () => {
    const onSaved = vi.fn();
    render(<NaryAckTemplatesBlock value={VALUE} onSaved={onSaved} />);
    const field = screen.getByLabelText("English In working hours");
    await userEvent.clear(field);
    await userEvent.type(field, "Hi {{name}, got it.");   // user-event: "{{" types one "{"
    await userEvent.click(screen.getByRole("button", { name: /save texts/i }));
    expect(update).toHaveBeenCalledTimes(1);
    const [key, json] = update.mock.calls[0];
    expect(key).toBe("nary.ack.templates");
    expect(JSON.parse(json).en.in_hours).toBe("Hi {name}, got it.");
    expect(JSON.parse(json).fr.in_hours).toBe(T.in_hours);
    expect(onSaved).toHaveBeenCalled();
  });

  it("won't save with an empty text", async () => {
    render(<NaryAckTemplatesBlock value={VALUE} onSaved={() => {}} />);
    await userEvent.clear(screen.getByLabelText("French Late prefix (an apology)"));
    expect(screen.getByRole("button", { name: /save texts/i })).toBeDisabled();
    expect(screen.getByText(/Fill in: French: Late prefix/)).toBeInTheDocument();
  });

  it("says so when the setting isn't valid JSON", () => {
    render(<NaryAckTemplatesBlock value="{oops" onSaved={() => {}} />);
    expect(screen.getByText(/not valid JSON/)).toBeInTheDocument();
  });
});
