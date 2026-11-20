import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const list = vi.fn();
const add = vi.fn();
const remove = vi.fn();
vi.mock("@/lib/api", () => ({
  emailImageSendersApi: {
    list: (...a: unknown[]) => list(...a),
    add: (...a: unknown[]) => add(...a),
    remove: (...a: unknown[]) => remove(...a),
  },
}));

import EmailBody, { resetImageSenders } from "./EmailBody";

const HTML = '<style>body{background:red} .nav{display:none}</style><p>Hello</p><img src="https://track.example/p.gif">';
const frames = () => document.querySelectorAll("iframe");
const srcdoc = (i = 0) => frames()[i].getAttribute("srcdoc") || "";

beforeEach(() => {
  resetImageSenders();
  list.mockResolvedValue({ data: { senders: [] } });
  add.mockResolvedValue({ data: { email: "news@shop.example" } });
  remove.mockResolvedValue({});
});

describe("EmailBody (security triage 18)", () => {
  it("renders in a sandbox without scripts, and the email's styles never reach the page", async () => {
    const { container } = render(<EmailBody html={HTML} sender="news@shop.example" />);
    const f = frames()[0];
    const sandbox = f.getAttribute("sandbox") || "";
    expect(sandbox).not.toContain("allow-scripts");
    expect(sandbox).not.toContain("allow-top-navigation");
    expect(sandbox).not.toContain("allow-forms");
    expect(sandbox).toContain("allow-same-origin");
    // The <style> lives only in the frame's srcdoc, not in the ERP's DOM.
    expect(container.querySelectorAll("style")).toHaveLength(0);
    const leaked = Array.from(document.querySelectorAll("style")).filter((s) => s.textContent?.includes(".nav"));
    expect(leaked).toHaveLength(0);
    expect(srcdoc()).toContain(".nav{display:none}");
    await waitFor(() => expect(list).toHaveBeenCalledTimes(1));
  });

  it("hides remote images until Show images is clicked", async () => {
    render(<EmailBody html={HTML} sender="news@shop.example" />);
    expect(srcdoc()).not.toContain("track.example");
    expect(screen.getByText("Images are hidden to protect your privacy.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Show images" }));
    expect(srcdoc()).toContain('src="https://track.example/p.gif"');
    expect(screen.queryByText("Images are hidden to protect your privacy.")).not.toBeInTheDocument();
    expect(add).not.toHaveBeenCalled();
  });

  it("remembers a sender on the server and then shows their images without asking", async () => {
    const first = render(<EmailBody html={HTML} sender="News@Shop.Example" />);
    await userEvent.click(screen.getByRole("button", { name: "Always show images from news@shop.example" }));
    expect(add).toHaveBeenCalledWith("news@shop.example");
    await waitFor(() => expect(srcdoc()).toContain("track.example"));
    first.unmount();

    // Another email from the same sender, e.g. after a reload: the server's list decides.
    resetImageSenders();
    list.mockResolvedValue({ data: { senders: ["news@shop.example"] } });
    render(<EmailBody html={HTML} sender="news@shop.example" />);
    await waitFor(() => expect(srcdoc()).toContain("track.example"));
    expect(screen.getByText("Images from news@shop.example are always shown.")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Stop" }));
    expect(remove).toHaveBeenCalledWith("news@shop.example");
    await waitFor(() => expect(srcdoc()).not.toContain("track.example"));
  });

  it("another sender stays hidden, and our own mail shows images without a bar", async () => {
    list.mockResolvedValue({ data: { senders: ["news@shop.example"] } });
    render(<EmailBody html={HTML} sender="other@spam.example" />);
    await waitFor(() => expect(list).toHaveBeenCalled());
    expect(srcdoc()).not.toContain("track.example");
    render(<EmailBody html={HTML} sender="me@example.com" trusted />);
    expect(srcdoc(1)).toContain("track.example");
    expect(screen.getAllByText("Images are hidden to protect your privacy.")).toHaveLength(1);
  });
});
