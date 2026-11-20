import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MoveToMenu } from "./MoveToMenu";

describe("MoveToMenu", () => {
  it("moves the card to the picked column without clicking the card", async () => {
    const onMove = vi.fn();
    const onCard = vi.fn();
    render(
      <div onClick={onCard}>
        <MoveToMenu itemLabel="Acme deal" current="new" onMove={onMove}
          options={[{ value: "new", label: "New" }, { value: "won", label: "Won" }]} />
      </div>,
    );
    await userEvent.click(screen.getByRole("button", { name: "Move Acme deal to" }));
    expect(screen.getByRole("menuitem", { name: /New/ })).toHaveAttribute("data-disabled");
    await userEvent.click(screen.getByRole("menuitem", { name: "Won" }));
    expect(onMove).toHaveBeenCalledWith("won");
    expect(onCard).not.toHaveBeenCalled();
  });
});
