"use client";

import { useState, useCallback, type DragEvent } from "react";

interface Options {
  /** Only accept files matching this predicate (e.g. PDFs, images). Rejected files are ignored. */
  accept?: (file: File) => boolean;
  /** Keep only the first accepted file (single-file inputs). */
  single?: boolean;
  disabled?: boolean;
}

/**
 * Drag-and-drop file support for any container. Spread `dropHandlers` onto the drop target and use
 * `dragActive` for hover styling. Works alongside a click-to-browse `<input type="file">`.
 *
 *   const { dragActive, dropHandlers } = useFileDrop((files) => upload(files), { accept: isPdf });
 *   <div {...dropHandlers} className={dragActive ? "ring-2 ring-[#1f4e79]" : ""} />
 */
export function useFileDrop(onFiles: (files: File[]) => void, opts: Options = {}) {
  const [dragActive, setDragActive] = useState(false);

  const onDragOver = useCallback((e: DragEvent) => {
    if (opts.disabled) return;
    e.preventDefault();
    setDragActive(true);
  }, [opts.disabled]);

  const onDragLeave = useCallback((e: DragEvent) => {
    e.preventDefault();
    // Ignore leave events bubbling up from children, only clear when leaving the target itself.
    if (e.currentTarget === e.target) setDragActive(false);
  }, []);

  const onDrop = useCallback((e: DragEvent) => {
    if (opts.disabled) return;
    e.preventDefault();
    setDragActive(false);
    let files = Array.from(e.dataTransfer?.files ?? []);
    if (opts.accept) files = files.filter(opts.accept);
    if (opts.single) files = files.slice(0, 1);
    if (files.length) onFiles(files);
  }, [onFiles, opts.accept, opts.single, opts.disabled]);

  return {
    dragActive,
    dropHandlers: { onDragOver, onDragEnter: onDragOver, onDragLeave, onDrop },
  };
}

/**
 * Drag-and-drop that routes dropped files into an existing hidden `<input type="file">` (by ref) and
 * fires its `change` event: so all existing onChange/submit logic keeps working unchanged. Ideal for
 * the common "hidden input + click-to-browse button/label" pattern. Add `{...dropHandlers}` to the
 * surrounding container and use `dragActive` for styling.
 */
export function useDropToInput(
  inputRef: { current: HTMLInputElement | null },
  opts: Options = {},
) {
  return useFileDrop((files) => {
    const input = inputRef.current;
    if (!input) return;
    const dt = new DataTransfer();
    // Respect the input's own `multiple` attribute.
    const chosen = input.multiple ? files : files.slice(0, 1);
    for (const f of chosen) dt.items.add(f);
    input.files = dt.files;
    input.dispatchEvent(new Event("change", { bubbles: true }));
  }, opts);
}

/** Common accept predicates. */
export const isPdf = (f: File) => f.type === "application/pdf" || f.name.toLowerCase().endsWith(".pdf");
export const isImage = (f: File) => f.type.startsWith("image/");
