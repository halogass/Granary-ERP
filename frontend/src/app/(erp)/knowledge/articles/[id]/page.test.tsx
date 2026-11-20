import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { join } from "node:path";

let search = new URLSearchParams();
let article: Record<string, unknown> = {};

vi.mock("next/navigation", () => ({
  useParams: () => ({ id: "a1" }),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  usePathname: () => "/knowledge/articles/a1",
  useSearchParams: () => search,
}));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1" } } }) }));
vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  kbApi: { getArticle: () => Promise.resolve({ data: article }) },
  attachmentApi: {},
}));
vi.mock("@/components/approvals/ApprovalPanel", () => ({ default: () => null }));
vi.mock("@/components/documents/ArticleDocuments", () => ({ ArticleDocumentCards: () => null }));

import ArticlePage from "./page";

const DRAFT = {
  id: "a1", title: "Packing guide", slug: "packing-guide", article_type: "wiki", content: "Body",
  status: "draft", version: "1.0", tags: [], access_level: "all_staff", updated_at: new Date().toISOString(),
};

describe("knowledge article page", () => {
  beforeEach(() => { search = new URLSearchParams(); article = { ...DRAFT }; });

  it("opens the author's signature dialog when it comes from Save and sign", async () => {
    search = new URLSearchParams("sign=1");
    render(<ArticlePage />);
    expect(await screen.findByRole("dialog", { name: /Sign this article/ })).toBeTruthy();
  });

  it("does not open it on a normal visit", async () => {
    render(<ArticlePage />);
    expect(await screen.findAllByText("Packing guide")).toBeTruthy();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("does not open it when the author has signed already", async () => {
    search = new URLSearchParams("sign=1");
    article = { ...DRAFT, author_signed_at: "2026-11-20T10:00:00Z", author_signed_by_name: "Ada" };
    render(<ArticlePage />);
    expect(await screen.findAllByText("Packing guide")).toBeTruthy();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("names the background worker, not the task queue product, in PDF import help", () => {
    const src = readFileSync(join(__dirname, "page.tsx"), "utf8");
    expect(src).not.toMatch(/Celery/);
  });
});
