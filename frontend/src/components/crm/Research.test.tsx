import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { ResearchButton, ResearchPanel, useResearch } from "./Research";
import type { ResearchObject, ResearchPerson, ResearchRun } from "@/lib/api";

const state = vi.fn();
const start = vi.fn();
const addPerson = vi.fn();
const dismissPerson = vi.fn();

vi.mock("@/lib/api", () => ({
  researchApi: {
    state: (...a: unknown[]) => state(...a),
    start: (...a: unknown[]) => start(...a),
    addPerson: (...a: unknown[]) => addPerson(...a),
    dismissPerson: (...a: unknown[]) => dismissPerson(...a),
  },
}));
vi.mock("next/link", () => ({
  default: ({ href, children, ...rest }: { href: string; children: React.ReactNode }) => <a href={href} {...rest}>{children}</a>,
}));

const DONE: ResearchRun = {
  id: "r1", object_type: "organization", record_id: "o1", depth: "deep", status: "done",
  summary: "Contoso Steel makes structural steel in Osaka.",
  sources: [{ url: "https://contoso-steel.example/board", title: "Board of Directors" }],
  news: [], channels: { phone: "+1 555 010 0199" },
  written: { city: "Osaka" }, suggested: { employee_count: 1200 },
};
const ROBERT: ResearchPerson = { id: "p1", name: "Robert Smith", title: "President Director", status: "pending",
  source_url: "https://contoso-steel.example/board" };
const DIANA: ResearchPerson = { id: "p2", name: "Diana Prince", title: "Finance Director", status: "pending",
  matched_contact_id: "c9" };

function Harness({ objectType = "organization" as ResearchObject }) {
  const s = useResearch(objectType, "o1");
  return (<><ResearchButton state={s} label="Deep research" /><ResearchPanel state={s} objectType={objectType} /></>);
}

beforeEach(() => {
  state.mockReset().mockResolvedValue({ data: { run: DONE, people: [ROBERT, DIANA] } });
  start.mockReset();
  addPerson.mockReset();
  dismissPerson.mockReset();
});

describe("ResearchPanel", () => {
  it("shows what was filled, what was only suggested, and the people found", async () => {
    render(<Harness />);
    expect(await screen.findByText(/makes structural steel/)).toBeTruthy();
    expect(screen.getByText("city")).toBeTruthy();
    expect(screen.getByText(/employees \(1200\)/)).toBeTruthy();
    expect(screen.getByText("Robert Smith")).toBeTruthy();
    expect(screen.getByText("Already a contact")).toBeTruthy();
    expect(screen.queryByLabelText("Add Diana Prince")).toBeNull();
  });

  it("adds a person only after the name and title are confirmed", async () => {
    addPerson.mockResolvedValue({ data: { contact_id: "c1", person: { ...ROBERT, status: "added", contact_id: "c1" } } });
    render(<Harness />);
    await userEvent.click(await screen.findByLabelText("Add Robert Smith"));
    expect(addPerson).not.toHaveBeenCalled();

    const title = screen.getByLabelText("Title");
    await userEvent.clear(title);
    await userEvent.type(title, "President Director");
    await userEvent.click(screen.getByText("Add as contact"));

    await waitFor(() => expect(addPerson).toHaveBeenCalledWith("p1", { name: "Robert Smith", title: "President Director" }));
    expect((await screen.findByText("Added")).getAttribute("href")).toBe("/crm/contacts/c1");
  });

  it("dismisses a person", async () => {
    dismissPerson.mockResolvedValue({ data: { ...ROBERT, status: "dismissed" } });
    render(<Harness />);
    await userEvent.click(await screen.findByLabelText("Dismiss Robert Smith"));
    await waitFor(() => expect(screen.queryByText("Robert Smith")).toBeNull());
    expect(dismissPerson).toHaveBeenCalledWith("p1");
  });

  it("shows a failed run's reason", async () => {
    state.mockResolvedValue({ data: { run: { ...DONE, status: "failed", error: "The monthly AI budget is used up." }, people: [] } });
    render(<Harness />);
    expect(await screen.findByText(/budget is used up/)).toBeTruthy();
  });
});

describe("ResearchButton", () => {
  it("starts research and shows it running", async () => {
    state.mockResolvedValue({ data: { run: null, people: [] } });
    start.mockResolvedValue({ data: { run: { ...DONE, status: "queued" }, queued: true } });
    render(<Harness objectType="contact" />);
    await userEvent.click(await screen.findByText("Deep research"));
    await waitFor(() => expect(start).toHaveBeenCalledWith("contact", "o1"));
    expect(await screen.findByText("Researching…")).toBeTruthy();
  });
});
