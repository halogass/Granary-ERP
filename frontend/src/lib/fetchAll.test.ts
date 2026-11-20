import { describe, expect, it } from "vitest";
import { fetchAllPages } from "./fetchAll";

const data = Array.from({ length: 451 }, (_, i) => ({ id: i }));
const get = async (p: Record<string, unknown>) => {
  const o = p.offset as number, l = p.limit as number;
  return { data: data.slice(o, o + l) };
};

describe("fetchAllPages", () => {
  it("loads every page, not only the first", async () => {
    const r = await fetchAllPages<{ id: number }>(get, { q: "x" }, 200);
    expect(r.rows.length).toBe(451);
    expect(r.capped).toBe(false);
  });
  it("accepts { items } and stops at max", async () => {
    const r = await fetchAllPages<{ id: number }>(async (p) => ({ data: { items: (await get(p)).data } }), {}, 100, 200);
    expect(r.rows.length).toBe(200);
    expect(r.capped).toBe(true);
  });
  it("stops when the endpoint ignores offset", async () => {
    const r = await fetchAllPages<{ id: number }>(async () => ({ data: data.slice(0, 50) }), {}, 50);
    expect(r.rows.length).toBe(50);
    expect(r.capped).toBe(true);
  });
});
