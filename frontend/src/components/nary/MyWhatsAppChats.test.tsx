import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const staffChats = vi.fn();
const search = vi.fn();
const add = vi.fn();
const primary = vi.fn();
const remove = vi.fn();
vi.mock("@/lib/api", () => ({
  naryApi: {
    staffChats: (...a: unknown[]) => staffChats(...a),
    searchStaffChats: (...a: unknown[]) => search(...a),
    addStaffChat: (...a: unknown[]) => add(...a),
    makeStaffChatPrimary: (...a: unknown[]) => primary(...a),
    removeStaffChat: (...a: unknown[]) => remove(...a),
  },
}));

import MyWhatsAppChats from "./MyWhatsAppChats";

const LINKS = [
  { id: "1", chat_id: "15550100111@c.us", phone: "15550100111", label: "Personal", is_primary: true, is_group: false },
  { id: "2", chat_id: "15550100222@c.us", phone: "15550100222", label: "Office", is_primary: false, is_group: false },
  { id: "3", chat_id: "120363@g.us", phone: null, label: "Team group", is_primary: false, is_group: true },
];

beforeEach(() => {
  staffChats.mockReset().mockResolvedValue({ data: { links: LINKS, suggestions: [] } });
  search.mockReset();
  add.mockReset().mockResolvedValue({ data: {} });
  primary.mockReset().mockResolvedValue({ data: {} });
  remove.mockReset().mockResolvedValue({});
});

describe("staff WhatsApp chats", () => {
  it("shows several numbers with one primary, and a group that can't be primary", async () => {
    render(<MyWhatsAppChats />);
    expect(await screen.findByText("+15550100111")).toBeTruthy();
    expect(screen.getByLabelText("Primary chat")).toBeTruthy();
    expect(screen.queryByLabelText("Make 120363@g.us primary")).toBeNull();
    await userEvent.click(screen.getByLabelText("Make 15550100222@c.us primary"));
    expect(primary).toHaveBeenCalledWith("2");
  });

  it("searches the company WhatsApp and links a result, a group included", async () => {
    search.mockResolvedValue({ data: { full: true, results: [
      { chat_id: "120999@g.us", name: "Field technicians", number: null, is_group: true, saved_contact: false, has_chat: true, linked_to: null },
      { chat_id: "15550100333@c.us", name: "Robert", number: "15550100333", is_group: false, saved_contact: true, has_chat: true, linked_to: "Rachel" },
    ] } });
    render(<MyWhatsAppChats userId="u-1" />);
    await userEvent.type(await screen.findByLabelText("Search WhatsApp chats"), "tekn");
    expect(await screen.findByText("Field technicians")).toBeTruthy();
    expect(search).toHaveBeenLastCalledWith("tekn", "u-1");
    expect(screen.getByText("linked to Rachel")).toBeTruthy();                // someone else's: no Link button
    await userEvent.click(screen.getByRole("button", { name: "Link" }));
    expect(add).toHaveBeenCalledWith({ chat: "120999@g.us", label: "Field technicians", user_id: "u-1" });
  });

  it("without inbox access a full number can still be linked when no chat has it yet", async () => {
    search.mockResolvedValue({ data: { full: false, results: [] } });
    render(<MyWhatsAppChats />);
    await userEvent.type(await screen.findByLabelText("Search WhatsApp chats"), "+1 555 010 0199");
    await waitFor(() => expect(screen.getByRole("button", { name: "Link number" })).toBeTruthy());
    await userEvent.click(screen.getByRole("button", { name: "Link number" }));
    expect(add).toHaveBeenCalledWith({ chat: "+1 555 010 0199", label: undefined, user_id: undefined });
  });
});
