/** Scroll `target` into view inside its scrolling pane only.
 *
 *  The app shell is a full-height frame with `overflow: hidden` wrappers around one
 *  scrolling pane (`#main`). `scrollIntoView` and the browser's own #anchor jump scroll
 *  every ancestor, hidden-overflow ones included, so the whole frame slid up: the top bar
 *  left the screen and a blank strip showed at the bottom. This sets the pane's scrollTop
 *  directly and puts any other shifted ancestor (and the document) back at the top. */
export function scrollPaneTo(
  target: HTMLElement,
  pane: HTMLElement | null,
  block: "start" | "center" = "start",
): void {
  if (!pane || !pane.contains(target)) {
    target.scrollIntoView({ block });
    return;
  }
  // Undo any shift the native anchor jump made outside the pane.
  for (let el = pane.parentElement; el; el = el.parentElement) {
    if (el.scrollTop) el.scrollTop = 0;
    if (el.scrollLeft) el.scrollLeft = 0;
  }
  const doc = document.scrollingElement;
  if (doc && doc.scrollTop) doc.scrollTop = 0;

  const paneBox = pane.getBoundingClientRect();
  const box = target.getBoundingClientRect();
  const margin = parseFloat(getComputedStyle(target).scrollMarginTop) || 0;
  const offset = box.top - paneBox.top + pane.scrollTop;
  const top = block === "center"
    ? offset - (pane.clientHeight - box.height) / 2
    : offset - margin;
  pane.scrollTop = Math.max(0, top);
}
