import { saveFile } from "@/lib/safeFile";

/** One CSV column: its header and how to read the cell from a row. */
export interface CsvColumn<T> {
  header: string;
  value: (row: T) => string | number | null | undefined;
}

/**
 * One CSV cell: quoted when it holds a comma, quote or line break; a text cell starting with =, +, -, @, tab or
 * carriage return gets a leading apostrophe so a spreadsheet never runs it as a formula (numbers stay numbers).
 */
export function csvCell(v: string | number | null | undefined): string {
  if (v === null || v === undefined) return "";
  if (typeof v === "number") return Number.isFinite(v) ? String(v) : "";
  let s = String(v);
  if (/^[=+\-@\t\r]/.test(s)) s = `'${s}`;
  return /[",\r\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

/** The rows as CSV text (CRLF line ends, header first). */
export function toCsv<T>(rows: T[], columns: CsvColumn<T>[]): string {
  const lines = [columns.map((c) => csvCell(c.header)).join(",")];
  for (const r of rows) lines.push(columns.map((c) => csvCell(c.value(r))).join(","));
  return lines.join("\r\n") + "\r\n";
}

/** Save the rows as a CSV file (with a BOM so spreadsheet apps read UTF-8). The file is downloaded, never opened. */
export function downloadCsv<T>(filename: string, rows: T[], columns: CsvColumn<T>[]): void {
  saveFile(new Blob(["﻿", toCsv(rows, columns)]), filename, "application/octet-stream");
}

/** "quotations-2026-11-20.csv". */
export function csvName(prefix: string, now = new Date()): string {
  return `${prefix}-${now.toISOString().slice(0, 10)}.csv`;
}
