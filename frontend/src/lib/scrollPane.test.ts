import { afterEach, describe, expect, it, vi } from "vitest";
import { scrollPaneTo } from "./scrollPane";

function rect(top: number, height: number): DOMRect {
  return { top, height, bottom: top + height, left: 0, right: 0, width: 0, x: 0, y: top, toJSON: () => ({}) } as DOMRect;
}

/** The app shell: a hidden-overflow frame around the scrolling #main pane. */
function shell() {
  document.body.innerHTML = `
    <div id="frame" style="overflow:hidden">
      <div id="column" style="overflow:hidden">
        <main id="main" style="overflow-y:auto"><section id="providers">x</section></main>
      </div>
    </div>`;
  const frame = document.getElementById("frame")!;
  const column = document.getElementById("column")!;
  const main = document.getElementById("main")!;
  const target = document.getElementById("providers")!;
  main.getBoundingClientRect = () => rect(56, 600);
  Object.defineProperty(main, "clientHeight", { value: 600, configurable: true });
  return { frame, column, main, target };
}

afterEach(() => { document.body.innerHTML = ""; });

describe("scrollPaneTo (settings deep links)", () => {
  it("scrolls the inner pane to the section, not the document", () => {
    const { main, target } = shell();
    const whole = vi.fn();
    target.scrollIntoView = whole;
    main.scrollTop = 100;
    target.getBoundingClientRect = () => rect(856, 300); // 800px below the pane's top
    target.style.scrollMarginTop = "16px";
    scrollPaneTo(target, main);
    expect(main.scrollTop).toBe(100 + 800 - 16);
    expect(whole).not.toHaveBeenCalled();
  });

  it("puts a frame the browser's anchor jump shifted back at the top", () => {
    const { frame, column, main, target } = shell();
    frame.scrollTop = 120;
    column.scrollTop = 40;
    target.getBoundingClientRect = () => rect(256, 100);
    scrollPaneTo(target, main);
    expect(frame.scrollTop).toBe(0);
    expect(column.scrollTop).toBe(0);
    expect(main.scrollTop).toBe(200);
  });

  it("centres a focused setting in the pane", () => {
    const { main, target } = shell();
    target.getBoundingClientRect = () => rect(1056, 100);
    scrollPaneTo(target, main, "center");
    expect(main.scrollTop).toBe(1000 - 250);
  });

  it("falls back to scrollIntoView when there is no pane", () => {
    const { target } = shell();
    const whole = vi.fn();
    target.scrollIntoView = whole;
    scrollPaneTo(target, null);
    expect(whole).toHaveBeenCalledWith({ block: "start" });
  });
});
