import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

import AttachmentPanel, { viaLabel } from "./AttachmentPanel";

const list = vi.fn();
vi.mock("@/lib/api", () => ({
  attachmentApi: {
    list: (...a: unknown[]) => list(...a),
    download: () => Promise.reject(new Error("not in this test")),
    upload: vi.fn(), delete: vi.fn(),
  },
  aiApi: {},
}));

describe("attachments marked as stored by your AI client or Nary", () => {
  it("labels the marker", () => {
    expect(viaLabel("mcp")).toBe("via MCP");
    expect(viaLabel("nary")).toBe("via Nary");
    expect(viaLabel(null)).toBeNull();
    expect(viaLabel(undefined)).toBeNull();
  });

  it("shows via MCP on such a file and nothing on a file uploaded here", async () => {
    list.mockResolvedValue({ data: [
      { id: "a1", file_name: "Office floor plan.png", file_size: 2048, mime_type: "application/pdf",
        file_type: "file", uploaded_by_name: "Owner", created_at: "2026-10-29T10:00:00Z", uploaded_via: "mcp" },
      { id: "a2", file_name: "contract.pdf", file_size: 1000, mime_type: "application/pdf", file_type: "file",
        uploaded_by_name: "Diana", created_at: "2026-10-29T09:00:00Z" },
    ] });
    render(<AttachmentPanel entityType="crm_contact" entityId="c1" />);
    expect(await screen.findByText("Office floor plan.png")).toBeTruthy();
    expect(list).toHaveBeenCalledWith("crm_contact", "c1");
    expect(screen.getAllByText("via MCP")).toHaveLength(1);
    expect(screen.getByText("via MCP").getAttribute("title")).toBe("Uploaded by Owner");
    expect(screen.queryByText("via Nary")).toBeNull();
  });
});
