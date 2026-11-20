import { describe, it, expect } from "vitest";
import { readFileSync, readdirSync, statSync } from "fs";
import { join } from "path";

/**
 * Search boxes must be type="search" with autofill switched off, or Chrome's password manager
 * (and 1Password / LastPass) treat the first one on a page as a username field and fill it.
 */
function files(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const p = join(dir, name);
    return statSync(p).isDirectory() ? files(p) : p.endsWith(".tsx") ? [p] : [];
  });
}

/** The full JSX tag starting at `start` — `>` inside {…} (arrow functions) doesn't end it. */
function tagAt(src: string, start: number): string {
  let depth = 0;
  let quote: string | null = null;
  for (let i = start; i < src.length; i++) {
    const c = src[i];
    if (quote) { if (c === "\\") i++; else if (c === quote) quote = null; continue; }
    if (c === '"' || c === "'" || c === "`") quote = c;
    else if (c === "{") depth++;
    else if (c === "}") depth--;
    else if (c === ">" && depth === 0) return src.slice(start, i + 1);
  }
  return src.slice(start);
}

describe("search boxes", () => {
  it("are never mistaken for a username field", () => {
    const offenders: string[] = [];
    let seen = 0;
    for (const file of files(join(__dirname, ".."))) {
      if (file.endsWith(".test.tsx")) continue;
      const src = readFileSync(file, "utf8");
      for (const m of src.matchAll(/<[Ii]nput\b/g)) {   // raw inputs and the ui Input
        const tag = tagAt(src, m.index!);
        if (!/placeholder=\{?\s*["'`]Search/.test(tag)) continue;
        seen++;
        if (!/type="search"/.test(tag) || !/autoComplete="off"/.test(tag)) {
          offenders.push(`${file.split("/src/")[1]}:${src.slice(0, m.index).split("\n").length}`);
        }
      }
    }
    expect(seen).toBeGreaterThan(30);          // the scan really found the app's search boxes
    expect(offenders).toEqual([]);
  });
});
