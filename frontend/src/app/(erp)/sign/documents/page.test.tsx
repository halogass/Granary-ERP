import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import SignDocumentsPage from "./page";

const get = vi.fn();
const post = vi.fn();
vi.mock("@/lib/api", () => ({
  default: {
    get: (...a: unknown[]) => get(...a),
    post: (...a: unknown[]) => post(...a),
    delete: vi.fn(),
  },
}));
vi.mock("next/dynamic", () => ({ default: () => () => null }));
vi.mock("@/components/sign/SignerPickerModal", () => ({ default: () => null }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/sign/documents", useSearchParams: () => new URLSearchParams() }));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1", email: "me@example.test" } } }) }));

const DOC = {
  id: "d1", title: "Service agreement", status: "draft", source_type: null, document_hash: null,
  certificate_url: null, file_url: null, created_at: "2026-11-20T00:00:00Z", signer_count: 0, signed_count: 0,
  completed_at: null, message: null, signers: [], fields: [],
};

describe("sign documents: add an external signer", () => {
  beforeEach(() => {
    get.mockReset(); post.mockReset();
  });

  it("enables Next: place fields as soon as the signer is added, even if the reload fails", async () => {
    let detailCalls = 0;
    get.mockImplementation((url: string) => {
      if (url === "/sign/documents") return Promise.resolve({ data: [DOC] });
      if (url === "/sign/documents/d1") {
        detailCalls += 1;
        return detailCalls === 1 ? Promise.resolve({ data: DOC }) : Promise.reject(new Error("offline"));
      }
      return Promise.resolve({ data: [] });
    });
    post.mockResolvedValue({ data: {
      id: "s1", name: "Dana Lee", email: "dana@example.test", sign_order: 1, role: "signer",
      status: "pending", signed_at: null,
    } });
    render(<SignDocumentsPage />);
    fireEvent.click((await screen.findAllByText("Service agreement"))[0]);
    const next = await screen.findByRole("button", { name: /Next: place fields/ });
    expect((next as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(screen.getByLabelText(/Full name/), { target: { value: "Dana Lee" } });
    fireEvent.change(screen.getByLabelText(/Email/), { target: { value: "dana@example.test" } });
    fireEvent.click(screen.getByRole("button", { name: "Add external signer" }));
    await waitFor(() => expect(post).toHaveBeenCalledWith("/sign/documents/d1/signers", expect.objectContaining({
      name: "Dana Lee", email: "dana@example.test", signer_type: "external",
    })));
    await waitFor(() => expect((screen.getByRole("button", { name: /Next: place fields/ }) as HTMLButtonElement).disabled).toBe(false));
    // The Fields step in the step bar opens too.
    expect((screen.getByRole("button", { name: /Fields/ }) as HTMLButtonElement).disabled).toBe(false);
  });
});
