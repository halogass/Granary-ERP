"use client";

import { useMemo, useState } from "react";
import { Braces, Plus, Table2, Trash2 } from "lucide-react";
import { Button, IconButton, Input, Textarea } from "@/components/ui";
import { cn } from "@/lib/utils";

type Scalar = string | number | boolean | null;
type Shape =
  | { kind: "tuples"; rows: Scalar[][]; width: number }
  | { kind: "list"; items: Scalar[] }
  | { kind: "map"; entries: [string, Scalar][] }
  | { kind: "records"; rows: Record<string, Scalar>[]; cols: string[] }
  | { kind: "raw" };

const isScalar = (v: unknown): v is Scalar => v === null || ["string", "number", "boolean"].includes(typeof v);

/** What table or list fits a JSON value; "raw" when none does (nested objects). */
export function detectShape(value: unknown): Shape {
  if (Array.isArray(value)) {
    if (value.length && value.every((r) => Array.isArray(r) && r.every(isScalar))) {
      const width = Math.max(...value.map((r) => (r as Scalar[]).length));
      if (value.every((r) => (r as Scalar[]).length === width)) return { kind: "tuples", rows: value as Scalar[][], width };
      return { kind: "raw" };
    }
    if (value.every(isScalar)) return { kind: "list", items: value as Scalar[] };
    if (value.every((r) => r && typeof r === "object" && !Array.isArray(r) && Object.values(r).every(isScalar))) {
      const cols: string[] = [];
      for (const r of value as Record<string, Scalar>[]) for (const k of Object.keys(r)) if (!cols.includes(k)) cols.push(k);
      return { kind: "records", rows: value as Record<string, Scalar>[], cols };
    }
    return { kind: "raw" };
  }
  if (value && typeof value === "object" && Object.values(value).every(isScalar)) {
    return { kind: "map", entries: Object.entries(value as Record<string, Scalar>) };
  }
  return { kind: "raw" };
}

/** A typed cell value from what was typed, keeping the column's kind (number, boolean, null). */
export function coerce(text: string, sample: Scalar | undefined): Scalar {
  const t = text.trim();
  if (t === "") return sample === undefined || sample === null || typeof sample !== "string" ? null : "";
  if (typeof sample === "boolean") return t.toLowerCase() === "true";
  if ((typeof sample === "number" || sample === null || sample === undefined) && /^-?\d+(\.\d+)?$/.test(t)) return Number(t);
  return text;
}

const show = (v: Scalar | undefined) => (v === null || v === undefined ? "" : String(v));

/** Parse a stored JSON setting; undefined when it is not valid JSON. */
export function parseJson(text: string | undefined): { ok: true; value: unknown } | { ok: false } {
  if (text === undefined || text.trim() === "") return { ok: true, value: null };
  try {
    return { ok: true, value: JSON.parse(text) };
  } catch {
    return { ok: false };
  }
}

/** Two JSON texts mean the same value (formatting ignored). */
export function sameJson(a: string | undefined, b: string | undefined) {
  if ((a ?? "") === (b ?? "")) return true;
  const x = parseJson(a), y = parseJson(b);
  return x.ok && y.ok && JSON.stringify(x.value) === JSON.stringify(y.value);
}

export interface JsonEditorProps {
  id?: string;
  value: string;
  onChange: (text: string) => void;
  /** Column headings (tuples, lists, maps). */
  columns?: string[];
  label?: string;
  invalid?: boolean;
}

/**
 * A JSON setting as a table or list (brackets, key/value maps, lists, rows of records), with a
 * raw JSON fallback for anything else and for people who prefer it.
 */
export function JsonEditor({ id, value, onChange, columns, label = "value", invalid }: JsonEditorProps) {
  const parsed = useMemo(() => parseJson(value), [value]);
  const shape = useMemo<Shape>(() => (parsed.ok ? detectShape(parsed.value ?? []) : { kind: "raw" }), [parsed]);
  const [raw, setRaw] = useState(false);
  const showRaw = raw || shape.kind === "raw";
  const emit = (v: unknown) => onChange(JSON.stringify(v));

  return (
    <div className="space-y-2">
      {showRaw ? (
        <Textarea id={id} value={value} onChange={(e) => onChange(e.target.value)} rows={Math.min(14, Math.max(3, value.split("\n").length + 1))}
          spellCheck={false} aria-invalid={invalid || !parsed.ok || undefined} aria-label={`${label} (JSON)`}
          className="font-mono text-xs" />
      ) : (
        <div id={id} className="overflow-x-auto rounded-lg border border-line">
          <TableFor shape={shape} columns={columns} label={label} emit={emit} />
        </div>
      )}
      <div className="flex items-center gap-2">
        {shape.kind !== "raw" && (
          <Button size="sm" variant="ghost" onClick={() => setRaw((r) => !r)} disabled={raw && !parsed.ok}>
            {raw ? <><Table2 aria-hidden /> Edit as table</> : <><Braces aria-hidden /> Raw JSON</>}
          </Button>
        )}
        {!parsed.ok && <span className="text-xs text-tone-red">Not valid JSON yet.</span>}
      </div>
    </div>
  );
}

const cellInput = "h-11 md:h-8 min-w-[6rem]";

function TableFor({ shape, columns, label, emit }: { shape: Shape; columns?: string[]; label: string; emit: (v: unknown) => void }) {
  if (shape.kind === "tuples") {
    const heads = Array.from({ length: shape.width }, (_, i) => columns?.[i] ?? `Column ${i + 1}`);
    const sample = (c: number) => shape.rows.find((r) => r[c] !== null)?.[c];
    const set = (r: number, c: number, text: string) =>
      emit(shape.rows.map((row, i) => (i === r ? row.map((v, j) => (j === c ? coerce(text, sample(c)) : v)) : row)));
    return (
      <Grid heads={heads} onAdd={() => {
        // A last row without a bound ("above") stays last.
        const blank = heads.map(() => null);
        const last = shape.rows[shape.rows.length - 1];
        emit(last && last[0] === null ? [...shape.rows.slice(0, -1), blank, last] : [...shape.rows, blank]);
      }}>
        {shape.rows.map((row, r) => (
          <tr key={r}>
            {row.map((v, c) => (
              <td key={c} className="px-2 py-1">
                <Input value={show(v)} onChange={(e) => set(r, c, e.target.value)} aria-label={`${heads[c]}, row ${r + 1}`}
                  placeholder={v === null ? "No limit" : undefined} numeric={typeof sample(c) === "number"} className={cellInput} />
              </td>
            ))}
            <RemoveCell label={`Remove row ${r + 1}`} onClick={() => emit(shape.rows.filter((_, i) => i !== r))} />
          </tr>
        ))}
      </Grid>
    );
  }
  if (shape.kind === "list") {
    const sample = shape.items.find((v) => v !== null);
    return (
      <Grid heads={[columns?.[0] ?? "Item"]} onAdd={() => emit([...shape.items, typeof sample === "number" ? 0 : ""])}>
        {shape.items.map((v, i) => (
          <tr key={i}>
            <td className="px-2 py-1">
              <Input value={show(v)} onChange={(e) => emit(shape.items.map((x, j) => (j === i ? coerce(e.target.value, sample ?? "") : x)))}
                aria-label={`${label}, item ${i + 1}`} numeric={typeof sample === "number"} className={cellInput} />
            </td>
            <RemoveCell label={`Remove item ${i + 1}`} onClick={() => emit(shape.items.filter((_, j) => j !== i))} />
          </tr>
        ))}
      </Grid>
    );
  }
  if (shape.kind === "map") {
    const sample = shape.entries.find(([, v]) => v !== null)?.[1];
    const write = (entries: [string, Scalar][]) => emit(Object.fromEntries(entries));
    return (
      <Grid heads={[columns?.[0] ?? "Key", columns?.[1] ?? "Value"]}
        onAdd={() => write([...shape.entries, [nextKey(shape.entries.map(([k]) => k)), typeof sample === "number" ? 0 : ""]])}>
        {shape.entries.map(([k, v], i) => (
          <tr key={i}>
            <td className="px-2 py-1">
              <Input value={k} onChange={(e) => write(shape.entries.map((en, j) => (j === i ? [e.target.value, en[1]] : en)))}
                aria-label={`Key, row ${i + 1}`} className={cn(cellInput, "font-mono text-[13px]")} />
            </td>
            <td className="px-2 py-1">
              <Input value={show(v)} onChange={(e) => write(shape.entries.map((en, j) => (j === i ? [en[0], coerce(e.target.value, sample ?? v)] : en)))}
                aria-label={`Value of ${k}`} numeric={typeof (sample ?? v) === "number"} className={cellInput} />
            </td>
            <RemoveCell label={`Remove ${k}`} onClick={() => write(shape.entries.filter((_, j) => j !== i))} />
          </tr>
        ))}
      </Grid>
    );
  }
  if (shape.kind === "records") {
    const sample = (c: string) => shape.rows.find((r) => r[c] !== null && r[c] !== undefined)?.[c];
    const set = (r: number, c: string, text: string) =>
      emit(shape.rows.map((row, i) => (i === r ? { ...row, [c]: coerce(text, sample(c)) } : row)));
    return (
      <Grid heads={shape.cols} onAdd={() => emit([...shape.rows, Object.fromEntries(shape.cols.map((c) => [c, typeof sample(c) === "number" ? 0 : typeof sample(c) === "boolean" ? false : ""]))])}>
        {shape.rows.map((row, r) => (
          <tr key={r}>
            {shape.cols.map((c) => (
              <td key={c} className="px-2 py-1">
                <Input value={show(row[c])} onChange={(e) => set(r, c, e.target.value)} aria-label={`${c}, row ${r + 1}`}
                  numeric={typeof sample(c) === "number"} className={cellInput} />
              </td>
            ))}
            <RemoveCell label={`Remove row ${r + 1}`} onClick={() => emit(shape.rows.filter((_, i) => i !== r))} />
          </tr>
        ))}
      </Grid>
    );
  }
  return null;
}

function nextKey(keys: string[]) {
  let i = keys.length + 1;
  while (keys.includes(`new_${i}`)) i++;
  return `new_${i}`;
}

function Grid({ heads, onAdd, children }: { heads: string[]; onAdd: () => void; children: React.ReactNode }) {
  return (
    <table className="w-full text-sm">
      <thead className="bg-surface-2 text-left text-xs text-fg-muted">
        <tr>
          {heads.map((h) => <th key={h} scope="col" className="px-3 py-2 font-medium">{h}</th>)}
          <th scope="col" className="w-10"><span className="sr-only">Remove</span></th>
        </tr>
      </thead>
      <tbody className="divide-y divide-line-soft">{children}</tbody>
      <tfoot>
        <tr>
          <td colSpan={heads.length + 1} className="border-t border-line px-2 py-1.5">
            <Button size="sm" variant="ghost" onClick={onAdd}><Plus aria-hidden /> Add row</Button>
          </td>
        </tr>
      </tfoot>
    </table>
  );
}

function RemoveCell({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <td className="px-1 py-1 text-right">
      <IconButton label={label} icon={Trash2} size="sm" onClick={onClick} />
    </td>
  );
}
