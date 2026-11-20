import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { Dialog } from "./Dialog";
import SearchSelect from "./SearchSelect";
import { localSource } from "@/lib/entitySources";

// 17 Nov 2026: inside a modal dialog the list was portalled to <body>, where Radix blocks pointer events and
// focus, so an option could be seen but not picked (GitLab > Add repository > GitLab server).
describe("SearchSelect inside a Dialog", () => {
  it("opens its list inside the dialog, and an option can be picked", async () => {
    const onChange = vi.fn();
    render(
      <Dialog open onOpenChange={() => {}} title="Add repository">
        <SearchSelect value="" onChange={onChange} placeholder="Select server"
          source={localSource([{ id: "s1", label: "Company Code Lab", sublabel: "https://code-lab.example" }])} />
      </Dialog>,
    );
    fireEvent.click(screen.getByRole("combobox"));
    const option = await screen.findByText("Company Code Lab");
    expect(screen.getByRole("dialog").contains(option)).toBe(true);
    fireEvent.click(option);
    await waitFor(() => expect(onChange).toHaveBeenCalledWith("s1", expect.objectContaining({ id: "s1" })));
  });

  it("outside a dialog the list still goes to <body>", async () => {
    render(<SearchSelect value="" onChange={() => {}} source={localSource([{ id: "a", label: "Alpha" }])} />);
    fireEvent.click(screen.getByRole("combobox"));
    const option = await screen.findByText("Alpha");
    expect(option.closest('[role="dialog"]')).toBeNull();
    expect(document.body.contains(option)).toBe(true);
  });
});
