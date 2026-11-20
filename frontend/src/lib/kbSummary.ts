/** The summary to show under an article's title, or null.
 *
 *  A summary that only repeats one of the body's headings ("Purpose", taken from the first
 *  line of an SOP template) says nothing and read as a stray word in the header, so it is
 *  not shown; the body still shows that heading in its place. */
export function displaySummary(summary: string | null | undefined, content: string | null | undefined): string | null {
  const s = (summary ?? "").trim();
  if (!s) return null;
  const norm = (t: string) => t.replace(/[*_`#:]/g, "").replace(/\s+/g, " ").trim().toLowerCase();
  const headings = (content ?? "")
    .split("\n")
    .map((line) => /^\s{0,3}#{1,6}\s+(.+?)\s*#*\s*$/.exec(line)?.[1])
    .filter((h): h is string => !!h)
    .map(norm);
  return headings.includes(norm(s)) ? null : s;
}
