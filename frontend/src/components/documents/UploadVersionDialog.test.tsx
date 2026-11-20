import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const suggest = vi.fn();
const upload = vi.fn();
vi.mock("@/lib/api", () => ({
  default: { get: () => Promise.reject(new Error("no settings")) },
  peopleApi: { directory: () => Promise.resolve({ data: [] }) },
  salesApi: { listCatalog: () => Promise.resolve({ data: [] }), getProduct: () => Promise.resolve({ data: null }) },
  crmApi: {}, vendorApi: {}, hrApi: {}, rndApi: {},
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }) }));
vi.mock("@/lib/documentLibrary", async (orig) => {
  const real = await orig<typeof import("@/lib/documentLibrary")>();
  return {
    ...real,
    fetchMaxFileMb: () => Promise.resolve(1),
    fetchSuggestedLabel: (id: string, prev?: string) =>
      suggest(id).then((r: { data: { label: string } }) => r.data.label).catch(() => real.nextLabel(prev)),
    docLibApi: { ...real.docLibApi, suggestLabel: (...a: unknown[]) => suggest(...a), uploadVersion: (...a: unknown[]) => upload(...a) },
  };
});

import { UploadVersionDialog } from "./DocumentDialogs";

const DOC = { id: "d1", title: "Product brochure", audiences: ["customer"] as const, current_version: { label: "v33" } };

beforeEach(() => { suggest.mockReset(); upload.mockReset(); });

describe("upload new version", () => {
  it("pre-fills the suggested label, says it needs approval and requires a change note", async () => {
    suggest.mockResolvedValue({ data: { label: "v34 EN" } });
    upload.mockResolvedValue({ data: {} });
    const onUploaded = vi.fn();
    render(<UploadVersionDialog open onOpenChange={() => {}} doc={DOC} onUploaded={onUploaded} />);

    expect(await screen.findByDisplayValue("v34 EN")).toBeInTheDocument();
    expect(suggest).toHaveBeenCalledWith("d1");
    expect(screen.getByText(/needs executive approval/i)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /upload and send for approval/i }));
    expect(await screen.findByText(/say what changed/i)).toBeInTheDocument();
    expect(upload).not.toHaveBeenCalled();

    await userEvent.type(screen.getByLabelText(/change note/i), "New laptop specs");
    const file = new File(["%PDF-1.7"], "brochure.pdf", { type: "application/pdf" });
    await userEvent.upload(screen.getByLabelText(/^file$/i), file);
    await userEvent.click(screen.getByRole("button", { name: /upload and send for approval/i }));
    expect(upload).toHaveBeenCalledWith("d1", expect.objectContaining({ label: "v34 EN", change_note: "New laptop specs", file }));
    expect(onUploaded).toHaveBeenCalled();
  });

  it("falls back to the local label rule and refuses a file over the size limit", async () => {
    suggest.mockRejectedValue(new Error("offline"));
    render(<UploadVersionDialog open onOpenChange={() => {}} doc={{ ...DOC, audiences: ["internal"] }} onUploaded={() => {}} />);
    expect(await screen.findByDisplayValue("v34")).toBeInTheDocument();
    expect(screen.getByText(/goes live as soon as it is uploaded/i)).toBeInTheDocument();
    const big = new File([new Uint8Array(2 * 1024 * 1024)], "big.pdf", { type: "application/pdf" });
    await userEvent.upload(screen.getByLabelText(/^file$/i), big);
    expect(await screen.findByText(/larger than 1 MB/i)).toBeInTheDocument();
  });
});
