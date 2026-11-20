import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

import DropPage from "./page";
import { precheck, putFile, reasonOf, slotsLeft, uploadUrl, type DropStatus } from "./drop";

const STATUS: DropStatus = {
  target_label: "Deal: Acme Ltd", state: "open", reason: null, expires_at: "2026-11-05T10:30:00+00:00",
  max_files: 3, files_used: 0, files: [],
  allowed: "PNG, JPEG, WebP or GIF images, PDF, Word (.docx) or Excel (.xlsx) files, MP4, MOV or WebM videos",
  accept: ".png,.mp4", file_max_bytes: 10 * 1024 * 1024, file_max_label: "10.0 MB",
  video_max_bytes: 100 * 1024 * 1024, video_max_label: "100.0 MB",
};

/** A fake XMLHttpRequest that reports half, then answers `status` with `body`. */
function fakeXhr(status: number, body: object, sent: { url: string; method: string }[]) {
  return class {
    status = 0;
    responseText = "";
    upload: { onprogress: ((e: ProgressEvent) => void) | null } = { onprogress: null };
    onload: (() => void) | null = null;
    onerror: (() => void) | null = null;
    private url = "";
    private method = "";
    open(method: string, url: string) { this.method = method; this.url = url; }
    setRequestHeader() {}
    send() {
      sent.push({ url: this.url, method: this.method });
      setTimeout(() => {
        this.upload.onprogress?.({ lengthComputable: true, loaded: 5, total: 10 } as ProgressEvent);
        this.status = status;
        this.responseText = JSON.stringify(body);
        this.onload?.();
      }, 0);
    }
  } as unknown as typeof XMLHttpRequest;
}

describe("drop page logic (5 Nov 2026)", () => {
  it("refuses an SVG, an oversized picture and an oversized video before sending", () => {
    expect(precheck({ name: "plan.svg", size: 10 }, STATUS)).toMatch(/\.svg files can't be uploaded/);
    expect(precheck({ name: "a.png", size: 11 * 1024 * 1024 }, STATUS)).toMatch(/over the 10.0 MB limit/);
    expect(precheck({ name: "site.mp4", size: 50 * 1024 * 1024 }, STATUS)).toBeNull();
    expect(precheck({ name: "site.mov", size: 101 * 1024 * 1024, type: "video/quicktime" }, STATUS))
      .toMatch(/video is 101.0 MB, over the 100.0 MB limit/);
    expect(precheck({ name: "empty.pdf", size: 0 }, STATUS)).toBe("The file is empty.");
  });

  it("counts the files left and builds the upload address", () => {
    expect(slotsLeft({ max_files: 10, files_used: 7 }, 2)).toBe(1);
    expect(slotsLeft({ max_files: 1, files_used: 1 }, 1)).toBe(0);
    expect(uploadUrl("tok_1", "site photo.png")).toBe("/api/v1/public/drop/tok_1/files?name=site%20photo.png");
  });

  it("gives the server's reason, else a plain line", () => {
    expect(reasonOf(410, JSON.stringify({ detail: "This upload link expired." }))).toBe("This upload link expired.");
    expect(reasonOf(413, "<html>nginx</html>")).toBe("The file is too large for this link.");
    expect(reasonOf(0, "")).toMatch(/connection was lost/);
  });

  it("PUTs the raw file and reports progress", async () => {
    const sent: { url: string; method: string }[] = [];
    const progress: number[] = [];
    const out = await putFile("tok", new File(["x"], "a.png"), (f) => progress.push(f),
      fakeXhr(200, { name: "a.png", size: 1, size_label: "1 B", type: "PNG image", files_used: 1, max_files: 3 }, sent));
    expect(out.type).toBe("PNG image");
    expect(progress).toEqual([0.5]);
    expect(sent).toEqual([{ method: "PUT", url: "/api/v1/public/drop/tok/files?name=a.png" }]);
    await expect(putFile("tok", new File(["x"], "b.png"), () => {},
      fakeXhr(410, { detail: "This upload link was closed." }, sent))).rejects.toThrow("This upload link was closed.");
  });
});

describe("drop page", () => {
  const realXhr = globalThis.XMLHttpRequest;
  beforeEach(() => vi.restoreAllMocks());
  afterEach(() => { globalThis.XMLHttpRequest = realXhr; });

  it("shows the record, uploads a picked file and closes on Done", async () => {
    const calls: string[] = [];
    globalThis.fetch = vi.fn(async (url: RequestInfo | URL, init?: RequestInit) => {
      calls.push(`${init?.method ?? "GET"} ${String(url)}`);
      return new Response(JSON.stringify(String(url).endsWith("/close") ? { closed: true, files_used: 1 } : STATUS),
        { status: 200 });
    }) as typeof fetch;
    const sent: { url: string; method: string }[] = [];
    globalThis.XMLHttpRequest = fakeXhr(200, { name: "site.png", size: 2048, size_label: "2 KB", type: "PNG image",
      files_used: 1, max_files: 3 }, sent);

    render(<DropPage params={{ token: "tok_9" }} />);
    expect(await screen.findByText("Deal: Acme Ltd")).toBeInTheDocument();
    expect(screen.getByText(/3 of 3 files left/)).toBeInTheDocument();

    fireEvent.change(screen.getByTestId("drop-input"), { target: { files: [new File(["png"], "site.png", { type: "image/png" })] } });
    expect(await screen.findByText("2 KB")).toBeInTheDocument();
    expect(sent[0].url).toBe("/api/v1/public/drop/tok_9/files?name=site.png");
    await waitFor(() => expect(screen.getByText(/2 of 3 files left/)).toBeInTheDocument());

    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(await screen.findByText("All set")).toBeInTheDocument();
    expect(calls).toContain("POST /api/v1/public/drop/tok_9/close");
  });

  it("says so for an unknown link and for one that takes no more files", async () => {
    globalThis.fetch = vi.fn(async () => new Response("{}", { status: 404 })) as typeof fetch;
    const { unmount } = render(<DropPage params={{ token: "nope" }} />);
    expect(await screen.findByText("Link not found")).toBeInTheDocument();
    unmount();

    globalThis.fetch = vi.fn(async () => new Response(JSON.stringify({ ...STATUS, state: "expired",
      reason: "This upload link expired. Ask for a new one." }), { status: 200 })) as typeof fetch;
    render(<DropPage params={{ token: "old" }} />);
    expect(await screen.findByText("This link takes no more files")).toBeInTheDocument();
    expect(screen.getByText(/expired/)).toBeInTheDocument();
  });
});
