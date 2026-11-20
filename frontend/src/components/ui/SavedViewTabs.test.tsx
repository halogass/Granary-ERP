import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1", email: "a@b.c" } } }) }));

import { SavedViewTabs, useSavedViews, savedViewsKey, type SavedView } from "./SavedViewTabs";

type F = { stage?: string };
const BUILTINS: SavedView<F>[] = [
  { id: "all", label: "All open", filters: {} },
  { id: "mine", label: "Mine", filters: { stage: "mine" } },
];

function Harness({ current = { stage: "proposal" }, onSelect }: { current?: F; onSelect?: (v: SavedView<F>) => void }) {
  const state = useSavedViews<F>("crm.deals", BUILTINS);
  return (
    <>
      <SavedViewTabs state={state} currentFilters={current} onSelect={onSelect} />
      <output data-testid="active">{state.active?.label}</output>
    </>
  );
}

beforeEach(() => localStorage.clear());

describe("SavedViewTabs", () => {
  it("saves the current filters as a view, per user and page, and restores it", () => {
    const onSelect = vi.fn();
    const { unmount } = render(<Harness onSelect={onSelect} />);
    fireEvent.click(screen.getByRole("button", { name: /Save view/ }));
    fireEvent.change(screen.getByLabelText("View name"), { target: { value: "At proposal" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(screen.getByRole("tab", { name: "At proposal" })).toHaveAttribute("aria-selected", "true");
    expect(onSelect).toHaveBeenLastCalledWith(expect.objectContaining({ label: "At proposal", filters: { stage: "proposal" } }));
    const stored = JSON.parse(localStorage.getItem(savedViewsKey("u1", "crm.deals"))!);
    expect(stored.views).toHaveLength(1);
    expect(stored.active).toBe(stored.views[0].id);

    unmount();
    render(<Harness />);
    expect(screen.getByRole("tab", { name: "At proposal" })).toBeInTheDocument();
    expect(screen.getByTestId("active")).toHaveTextContent("At proposal");
  });

  it("remembers the chosen built-in view and removes saved views", () => {
    localStorage.setItem(savedViewsKey("u1", "crm.deals"), JSON.stringify({ views: [{ id: "v1", label: "Old", filters: {} }], active: "v1" }));
    const { unmount } = render(<Harness />);
    expect(screen.getByTestId("active")).toHaveTextContent("Old");
    fireEvent.click(screen.getByRole("button", { name: "Remove view Old" }));
    expect(screen.queryByRole("tab", { name: "Old" })).toBeNull();
    expect(screen.getByTestId("active")).toHaveTextContent("All open");

    fireEvent.click(screen.getByRole("tab", { name: "Mine" }));
    unmount();
    render(<Harness />);
    expect(screen.getByTestId("active")).toHaveTextContent("Mine");
    expect(screen.queryByRole("button", { name: /Remove view Mine/ })).toBeNull();   // built-ins stay
  });
});
