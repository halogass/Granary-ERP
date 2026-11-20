import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// 5 Nov 2026: videos in the document library: the picker takes MP4 / MOV / WebM, the viewer plays them.
const view = vi.fn();
const upload = vi.fn();
vi.mock("@/lib/api", () => ({
  default: {
    get: () => Promise.resolve({ data: [
      { key: "documents.max_file_mb", value: "1" },
      { key: "nary.upload.video_max_mb", value: "5" },
    ] }),
  },
  peopleApi: { directory: () => Promise.resolve({ data: [] }) },
  salesApi: { listCatalog: () => Promise.resolve({ data: [] }), getProduct: () => Promise.resolve({ data: null }) },
  crmApi: {}, vendorApi: {}, hrApi: {}, rndApi: {},
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }) }));
vi.mock("@/lib/documentLibrary", async (orig) => {
  const real = await orig<typeof import("@/lib/documentLibrary")>();
  return {
    ...real,
    fetchSuggestedLabel: () => Promise.resolve("v2"),
    docLibApi: { ...real.docLibApi, view: (...a: unknown[]) => view(...a), uploadVersion: (...a: unknown[]) => upload(...a) },
  };
});

import { PDF_OR_IMAGE, UploadVersionDialog } from "./DocumentDialogs";
import DocumentViewer from "./DocumentViewer";
import { fileKind, formatDuration } from "@/lib/documentLibrary";

const DOC = { id: "d1", title: "Product demo", audiences: ["internal"] as const, current_version: { label: "v1" } };

beforeEach(() => {
  view.mockReset();
  upload.mockReset();
  (URL as unknown as { createObjectURL: (b: Blob) => string }).createObjectURL = () => "blob:video";
  (URL as unknown as { revokeObjectURL: (u: string) => void }).revokeObjectURL = () => {};
});

describe("videos in the document library", () => {
  it("the picker accepts MP4, MOV and WebM next to PDFs and images", async () => {
    for (const t of ["video/mp4", "video/quicktime", "video/webm", ".mp4", ".mov", ".webm", "application/pdf"]) {
      expect(PDF_OR_IMAGE.split(",")).toContain(t);
    }
    expect(fileKind("demo.mp4")).toBe("video");
    expect(fileKind("x", "video/webm")).toBe("video");
    expect(formatDuration(42)).toBe("0:42");
    expect(formatDuration(3723)).toBe("1:02:03");

    upload.mockResolvedValue({ data: {} });
    render(<UploadVersionDialog open onOpenChange={() => {}} doc={DOC} onUploaded={() => {}} />);
    const input = screen.getByLabelText(/^file$/i) as HTMLInputElement;
    expect(input.accept).toContain("video/mp4");
    expect(await screen.findByText(/video \(MP4, MOV, WebM\) up to 5 MB/i)).toBeInTheDocument();
    // a 2 MB video passes (videos have their own cap) where a 2 MB PDF would not
    const video = new File([new Uint8Array(2 * 1024 * 1024)], "demo.mp4", { type: "video/mp4" });
    await userEvent.upload(input, video);
    expect(screen.queryByText(/larger than/i)).not.toBeInTheDocument();
    await userEvent.type(screen.getByLabelText(/change note/i), "Demo video");
    await userEvent.click(screen.getByRole("button", { name: /upload and publish/i }));
    expect(upload).toHaveBeenCalledWith("d1", expect.objectContaining({ file: video }));
  });

  it("the viewer plays a video in a player with controls", async () => {
    view.mockResolvedValue({ data: new Blob([new Uint8Array([0, 0, 0, 24])], { type: "video/mp4" }) });
    render(<DocumentViewer documentId="d1" title="Product demo" fileName="demo.mp4" mime="video/mp4" />);
    const player = (await screen.findByTestId("document-video")) as HTMLVideoElement;
    expect(player.tagName).toBe("VIDEO");
    expect(player.controls).toBe(true);
    expect(player.getAttribute("src")).toBe("blob:video");
  });
});
