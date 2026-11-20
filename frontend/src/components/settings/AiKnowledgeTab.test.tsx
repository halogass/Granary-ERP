import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import AiKnowledgeTab, { rejectReason } from "./AiKnowledgeTab";

const list = vi.fn(), upload = vi.fn(), update = vi.fn(), search = vi.fn(), remove = vi.fn();
vi.mock("@/lib/api", () => ({
  aiKnowledgeApi: {
    list: (...a: unknown[]) => list(...a), upload: (...a: unknown[]) => upload(...a),
    update: (...a: unknown[]) => update(...a), search: (...a: unknown[]) => search(...a),
    remove: (...a: unknown[]) => remove(...a), reindex: vi.fn(), get: vi.fn(),
  },
}));

const DOC = { id: "d1", title: "Discount rules", source_type: "markdown", status: "ready", is_active: true,
  chunk_count: 2, use_count: 5, characters: 900, description: "Any price question" };

beforeEach(() => {
  list.mockReset().mockResolvedValue({ data: { items: [DOC] } });
  upload.mockReset().mockResolvedValue({ data: DOC });
  update.mockReset().mockResolvedValue({ data: DOC });
  search.mockReset();
  remove.mockReset().mockResolvedValue({});
});

describe("AI knowledge settings", () => {
  it("lists documents with their status and use", async () => {
    render(<AiKnowledgeTab />);
    expect(await screen.findByText("Discount rules")).toBeTruthy();
    expect(screen.getByText("Ready")).toBeTruthy();
    expect(screen.getByText("Used 5×")).toBeTruthy();
  });

  it("adds pasted Markdown", async () => {
    render(<AiKnowledgeTab />);
    await userEvent.click(await screen.findByText("Write Markdown"));
    await userEvent.type(screen.getByPlaceholderText("e.g. Discount and pricing rules"), "Support hours");
    await userEvent.type(screen.getByPlaceholderText(/# Discount rules/), "Weekdays 09-17 JST");
    await userEvent.click(screen.getByText("Add"));
    await waitFor(() => expect(upload).toHaveBeenCalled());
    const form = upload.mock.calls[0][0] as FormData;
    expect(form.get("title")).toBe("Support hours");
    expect(form.get("content")).toBe("Weekdays 09-17 JST");
  });

  it("switches a document off without deleting it", async () => {
    render(<AiKnowledgeTab />);
    await userEvent.click(await screen.findByRole("checkbox", { name: "On" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith("d1", { is_active: false }));
    expect(remove).not.toHaveBeenCalled();
  });

  it("shows what the AI would find", async () => {
    search.mockResolvedValue({ data: { results: [
      { doc_id: "d1", title: "Discount rules", content: "Distributors get 12%", distance: 0.2, used: true }] } });
    render(<AiKnowledgeTab />);
    await userEvent.type(await screen.findByPlaceholderText(/What discount/), "distributor discount");
    await userEvent.click(screen.getByText("Search"));
    expect(await screen.findByText("added automatically")).toBeTruthy();
    expect(screen.getByText("match 80%")).toBeTruthy();
  });
});

function file(name: string, bytes = 10, type = "") {
  return new File(["x".repeat(bytes)], name, { type });
}

describe("uploading several files", () => {
  it("uploads every chosen file, one by one, titled from its name", async () => {
    let active = 0, maxActive = 0;
    upload.mockImplementation(async () => {
      active += 1; maxActive = Math.max(maxActive, active);
      await new Promise((r) => setTimeout(r, 5));
      active -= 1;
      return { data: DOC };
    });
    render(<AiKnowledgeTab />);
    await userEvent.click(await screen.findByText("Upload files"));
    await userEvent.upload(screen.getByLabelText("Choose files"),
      [file("pricing_rules.md"), file("Laptop-datasheet.pdf", 10, "application/pdf")]);
    expect(screen.getByDisplayValue("pricing rules")).toBeTruthy();
    await userEvent.click(screen.getByText("Add 2 documents"));
    await waitFor(() => expect(upload).toHaveBeenCalledTimes(2));
    const titles = upload.mock.calls.map((c) => (c[0] as FormData).get("title"));
    expect(titles).toEqual(["pricing rules", "Laptop datasheet"]);
    expect(maxActive).toBe(1);
  });

  it("lists unsupported files instead of silently dropping them", async () => {
    render(<AiKnowledgeTab />);
    await userEvent.click(await screen.findByText("Upload files"));
    fireEvent.change(screen.getByLabelText("Choose files"), { target: { files: [file("notes.docx"), file("ok.md")] } });
    expect(await screen.findByText(/notes\.docx: Only Markdown/)).toBeTruthy();
    expect(screen.getByText("Add 1 document")).toBeTruthy();
  });

  it("keeps a failed file to retry while the others succeed", async () => {
    upload.mockResolvedValueOnce({ data: DOC }).mockRejectedValueOnce(new Error("boom"));
    render(<AiKnowledgeTab />);
    await userEvent.click(await screen.findByText("Upload files"));
    await userEvent.upload(screen.getByLabelText("Choose files"), [file("a.md"), file("b.md")]);
    await userEvent.click(screen.getByText("Add 2 documents"));
    expect(await screen.findByText("Retry 1")).toBeTruthy();
  });

  it("opens the upload with the files dropped anywhere on the tab", async () => {
    const { container } = render(<AiKnowledgeTab />);
    await screen.findByText("Discount rules");
    const dataTransfer = { files: [file("dropped-guide.md"), file("brochure.pdf", 10, "application/pdf")] };
    fireEvent.dragOver(container.firstChild as Element, { dataTransfer });
    fireEvent.drop(container.firstChild as Element, { dataTransfer });
    expect(await screen.findByText("Add 2 documents")).toBeTruthy();
    expect(screen.getByDisplayValue("dropped guide")).toBeTruthy();
  });

  it("applies the server's size limits before uploading", () => {
    expect(rejectReason(file("big.pdf", 26 * 1024 * 1024))).toMatch(/25 MB/);
    expect(rejectReason(file("fine.PDF"))).toBeNull();
    expect(rejectReason(file("readme.markdown"))).toBeNull();
  });
});
