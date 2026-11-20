// fix-longapply: long work (drafting, Ask Nary to change, a card that drafts) can outlast the request. The API
// then answers 202 with `still_working` and the work finishes on its own; these helpers wait for its end by
// polling, so the page shows "working" and refreshes when it is done.
import type { AxiosResponse } from "axios";
import { naryApi, salesDocumentsApi } from "@/lib/api";

/** An error shaped like an API error, so extractApiError shows its text. */
export class LongWorkError extends Error {
  response: { data: { detail: string } };
  constructor(detail: string) {
    super(detail);
    this.response = { data: { detail } };
  }
}

export const POLL_MS = 3000;
/** The watchdog fails a stuck card within the server's stale limit; the page stops waiting well after it. */
const GIVE_UP_MS = 45 * 60 * 1000;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** True for a 202 "still working" answer. */
export function isStillWorking(res: { status?: number; data?: unknown } | null | undefined): boolean {
  const d = res?.data as { still_working?: boolean } | undefined;
  return res?.status === 202 || !!d?.still_working;
}

/** The answer of a proposal job: the response itself when it finished in time, else the job's result once
 * it is done (a failed job throws its error). `onWorking` is called once when the wait starts. */
export async function settleJob<T = unknown>(docId: string, res: AxiosResponse<T>, onWorking?: () => void,
  poll = POLL_MS): Promise<T> {
  if (!isStillWorking(res)) return res.data;
  const jobId = (res.data as { job_id?: string }).job_id;
  if (!jobId) return res.data;
  onWorking?.();
  const until = Date.now() + GIVE_UP_MS;
  while (Date.now() < until) {
    await sleep(poll);
    let job: { status: string; result: unknown; error: string | null };
    try { job = (await salesDocumentsApi.job(docId, jobId)).data; } catch { continue; }
    if (job.status === "done") return job.result as T;
    if (job.status === "failed") throw new LongWorkError(job.error || "Nary couldn't finish it.");
  }
  throw new LongWorkError("Nary is still working on it. Refresh the page in a few minutes.");
}

/** A Nary card that was still applying: waits until it is applied or failed and returns it. */
export async function settleCard<T extends { id: string; status: string }>(res: AxiosResponse<T>,
  onWorking?: () => void, poll = POLL_MS): Promise<T> {
  if (!isStillWorking(res) || res.data.status !== "applying") return res.data;
  onWorking?.();
  const id = (res.data as { card_id?: string }).card_id || res.data.id;
  const until = Date.now() + GIVE_UP_MS;
  while (Date.now() < until) {
    await sleep(poll);
    let card: T;
    try { card = (await naryApi.card(id)).data as T; } catch { continue; }
    if (card.status !== "applying") return card;
  }
  throw new LongWorkError("Nary is still working on this card. Check it on the Approvals page in a few minutes.");
}
