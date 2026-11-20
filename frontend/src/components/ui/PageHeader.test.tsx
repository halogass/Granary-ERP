import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { PageHeader } from "./PageHeader";

describe("PageHeader", () => {
  // 21 Nov 2026: a quotation number was cut to "QUO-2026-11-0000..." by its button row
  it("never truncates the title; the title block keeps its smallest width so the buttons wrap instead", () => {
    render(<PageHeader title={<span className="whitespace-nowrap">QUO-2026-11-00011-1</span>}
      actions={<button type="button">Send to client</button>} />);
    const h1 = screen.getByRole("heading", { level: 1 });
    expect(h1.className).not.toMatch(/truncate/);
    expect(h1.textContent).toBe("QUO-2026-11-00011-1");
    const block = h1.closest("[data-slot=page-title]")!;
    expect(block.className).toMatch(/sm:min-w-min/);
    expect(block.parentElement!.className).toMatch(/flex-wrap/);
  });
});
