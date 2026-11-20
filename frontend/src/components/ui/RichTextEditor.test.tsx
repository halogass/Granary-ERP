import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import RichTextEditor from "./RichTextEditor";

const editor = (c: HTMLElement) => c.querySelector("[contenteditable]") as HTMLElement;

describe("RichTextEditor", () => {
  it("sanitizes the initial value before it becomes live DOM", () => {
    const { container } = render(
      <RichTextEditor value={'<p>Hi</p><img src="x" onerror="window.__xss=1"><script>window.__xss=2</script>'} onChange={() => {}} />,
    );
    const el = editor(container);
    expect(el.innerHTML).toContain("<p>Hi</p>");
    expect(el.innerHTML).not.toMatch(/onerror/i);
    expect(el.innerHTML).not.toMatch(/<script/i);
  });

  it("sanitizes a value changed from outside (a template insert)", () => {
    const { container, rerender } = render(<RichTextEditor value="<p>a</p>" onChange={() => {}} />);
    rerender(<RichTextEditor value={'<p>a</p><img src=x onerror=alert(1)><a href="javascript:alert(1)">x</a>'} onChange={() => {}} />);
    const el = editor(container);
    expect(el.innerHTML).not.toMatch(/onerror/i);
    expect(el.innerHTML).not.toMatch(/javascript:/i);
  });
});
