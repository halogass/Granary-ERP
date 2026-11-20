import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

import MarkdownMessage, { prepareMarkdown } from "./MarkdownMessage";

vi.mock("next/link", () => ({
  default: ({ href, children }: { href: string; children: React.ReactNode }) => <a href={href}>{children}</a>,
}));

describe("assistant markdown", () => {
  it("renders lists, tables and code as real elements", () => {
    const { container } = render(<MarkdownMessage content={
      "1. First\n2. Second\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n```\nx = 1\n```"} />);
    expect(container.querySelector("ol")?.querySelectorAll("li")).toHaveLength(2);
    // Tables scroll inside the bubble instead of overflowing it.
    expect(container.querySelector("table")?.parentElement?.className).toContain("overflow-x-auto");
    expect(container.querySelector("pre")?.textContent).toContain("x = 1");
  });

  it("opens outside links in a new tab and keeps in-app links in the app", () => {
    render(<MarkdownMessage content={"[site](https://example.com) and [deal](/crm/deals/1)"} />);
    expect(screen.getByRole("link", { name: "site" })).toHaveAttribute("target", "_blank");
    expect(screen.getByRole("link", { name: "deal" })).not.toHaveAttribute("target");
  });

  it("drops think blocks, even one still streaming", () => {
    expect(prepareMarkdown("<think>plan</think>Answer")).toBe("Answer");
    expect(prepareMarkdown("<think>still going")).toBe("");
  });

  it("shows WhatsApp's one-part-at-a-time marker as a divider", () => {
    expect(prepareMarkdown("First point.\n[[next]]\nSecond point.")).toBe("First point.\n\n---\n\nSecond point.");
  });

  it("turns bracket math into $$ math but leaves prices and code alone", () => {
    expect(prepareMarkdown("area \\(\\pi r^2\\)")).toBe("area $$\\pi r^2$$");
    expect(prepareMarkdown("costs $100 and $200")).toBe("costs $100 and $200");
    expect(prepareMarkdown("`\\(x\\)`")).toBe("`\\(x\\)`");
  });
});
