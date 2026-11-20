import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import TrainingModulePage from "./page";

vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), useParams: () => ({ id: "m1" }), usePathname: () => "/partner/training/m1", useSearchParams: () => new URLSearchParams() }));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { access_token: "tok", user: { id: "u1" } } }) }));

const MODULE = {
  id: "m1", title: "Product fundamentals", description: "", pass_score: 70, chapter_count: 1,
  chapters: [{ id: "c1", title: "One", content: "Read me", video_url: null, order: 1,
    questions: [{ id: "q1", question: "Pick b", options: ["a", "b"], order: 1 }] }],
  progress: { status: "in_progress", current_chapter: 1, score: null, completed_at: null },
};
const CERT_URL = "/api/v1/partner/portal/training/certifications/c-9/pdf";

const fetchMock = vi.fn();

describe("partner training: certificate after a pass", () => {
  beforeEach(() => {
    fetchMock.mockReset().mockImplementation((url: string) => {
      const json = (b: unknown) => Promise.resolve(new Response(JSON.stringify(b), { status: 200 }));
      if (url.endsWith("/submit-quiz")) {
        return json({ score: 100, passed: true, correct: 1, total: 1, certification_issued: true, attempts_left: 4,
          certification_id: "c-9", cert_url: CERT_URL });
      }
      if (url === CERT_URL) {
        return Promise.resolve({ ok: true, status: 200, blob: () => Promise.resolve(new Blob(["%PDF"])),
          headers: new Headers({ "content-disposition": 'attachment; filename="CERT-1.pdf"' }) });
      }
      if (url.endsWith("/start")) return json({});
      return json(MODULE);
    });
    vi.stubGlobal("fetch", fetchMock);
    URL.createObjectURL = vi.fn(() => "blob:x");
    URL.revokeObjectURL = vi.fn();
  });
  afterEach(() => vi.unstubAllGlobals());

  it("downloads the certificate the server issued, with the partner's token", async () => {
    render(<TrainingModulePage />);
    fireEvent.click(await screen.findByRole("button", { name: /final quiz/i }));
    fireEvent.click(await screen.findByRole("button", { name: "b" }));
    fireEvent.click(screen.getByRole("button", { name: /submit quiz/i }));
    fireEvent.click(await screen.findByRole("button", { name: /download certificate/i }));
    await waitFor(() => expect(fetchMock).toHaveBeenCalledWith(CERT_URL, expect.anything()));
    const init = fetchMock.mock.calls.find((c) => c[0] === CERT_URL)![1] as RequestInit;
    expect(init.headers).toMatchObject({ Authorization: "Bearer tok" });
    await waitFor(() => expect(URL.createObjectURL).toHaveBeenCalled());
  });
});
