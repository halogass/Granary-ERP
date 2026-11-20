import { describe, it, expect, vi, beforeEach } from "vitest";
import type { AxiosResponse } from "axios";

const card = vi.fn();
const job = vi.fn();
vi.mock("@/lib/api", () => ({
  naryApi: { card: (...a: unknown[]) => card(...a) },
  salesDocumentsApi: { job: (...a: unknown[]) => job(...a) },
}));

import { isStillWorking, settleCard, settleJob } from "./longWork";
import { extractApiError } from "./apiError";

const res = <T,>(status: number, data: T) => ({ status, data }) as AxiosResponse<T>;

beforeEach(() => { card.mockReset(); job.mockReset(); });

describe("long work (fix-longapply)", () => {
  it("returns a finished answer as it is", async () => {
    const onWorking = vi.fn();
    expect(await settleJob("d1", res(200, { summary: "done" }), onWorking, 1)).toEqual({ summary: "done" });
    expect(onWorking).not.toHaveBeenCalled();
    expect(job).not.toHaveBeenCalled();
  });

  it("waits for a 202 proposal job and returns its result", async () => {
    job.mockResolvedValueOnce({ data: { status: "running", result: null, error: null } })
      .mockResolvedValueOnce({ data: { status: "done", result: { sheet: { total: 1 } }, error: null } });
    const onWorking = vi.fn();
    const out = await settleJob("d1", res(202, { status: "working", job_id: "j1", still_working: true }), onWorking, 1);
    expect(out).toEqual({ sheet: { total: 1 } });
    expect(onWorking).toHaveBeenCalledTimes(1);
    expect(job).toHaveBeenCalledWith("d1", "j1");
  });

  it("a failed job throws its reason so the page shows it", async () => {
    job.mockResolvedValue({ data: { status: "failed", result: null, error: "Nary couldn't draw that" } });
    const err = await settleJob("d1", res(202, { job_id: "j1", still_working: true }), undefined, 1).catch((e) => e);
    expect(extractApiError(err)).toBe("Nary couldn't draw that");
  });

  it("polls a card still applying until it is applied", async () => {
    card.mockResolvedValueOnce({ data: { id: "c1", status: "applying" } })
      .mockResolvedValueOnce({ data: { id: "c1", status: "applied", result: { label: "Proposal drafted" } } });
    const onWorking = vi.fn();
    const r = res(202, { id: "c1", card_id: "c1", status: "applying", still_working: true });
    expect(isStillWorking(r)).toBe(true);
    const out = await settleCard(r, onWorking, 1);
    expect(out).toMatchObject({ status: "applied" });
    expect(card).toHaveBeenCalledWith("c1");
    expect(onWorking).toHaveBeenCalledTimes(1);
  });

  it("a card decided in time is not polled", async () => {
    const out = await settleCard(res(200, { id: "c1", status: "applied" }), undefined, 1);
    expect(out.status).toBe("applied");
    expect(card).not.toHaveBeenCalled();
  });
});
