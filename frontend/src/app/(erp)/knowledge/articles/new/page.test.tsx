import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

const push = vi.fn();
const createArticle = vi.fn();
const publishArticle = vi.fn();

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push, replace: vi.fn() }),
  usePathname: () => "/knowledge/articles/new",
  useSearchParams: () => new URLSearchParams(),
}));
vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  kbApi: {
    listCategories: () => Promise.resolve({ data: [] }),
    createArticle: (...a: unknown[]) => createArticle(...a),
    publishArticle: (...a: unknown[]) => publishArticle(...a),
    importPdf: vi.fn(),
  },
}));
vi.mock("../../_components/ArticleEditor", () => ({
  ACCESS_OPTIONS: [{ value: "all_staff", label: "All staff" }],
  MarkdownEditor: () => null,
  PrivateSwitch: () => null,
  SopStepsEditor: () => null,
}));

import NewArticlePage from "./page";

describe("new knowledge article", () => {
  beforeEach(() => {
    push.mockReset(); createArticle.mockReset(); publishArticle.mockReset();
    createArticle.mockResolvedValue({ data: { id: "a1" } });
  });

  it("saves, then opens the article to sign instead of a publish that needs a signature first", async () => {
    render(<NewArticlePage />);
    expect(screen.queryByRole("button", { name: /Save and publish/ })).toBeNull();
    fireEvent.change(screen.getByPlaceholderText("e.g. Lead Qualification Process"), { target: { value: "Packing guide" } });
    fireEvent.click(screen.getByRole("button", { name: /Save and sign/ }));
    await waitFor(() => expect(push).toHaveBeenCalledWith("/knowledge/articles/a1?sign=1"));
    expect(createArticle).toHaveBeenCalledTimes(1);
    expect(publishArticle).not.toHaveBeenCalled();
  });

  it("Save draft opens the article without the signature dialog", async () => {
    render(<NewArticlePage />);
    fireEvent.change(screen.getByPlaceholderText("e.g. Lead Qualification Process"), { target: { value: "Packing guide" } });
    fireEvent.click(screen.getByRole("button", { name: /Save draft/ }));
    await waitFor(() => expect(push).toHaveBeenCalledWith("/knowledge/articles/a1"));
  });
});
