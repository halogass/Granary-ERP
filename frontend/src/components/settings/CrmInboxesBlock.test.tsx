import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const getEmailConfig = vi.fn();
const listCrmMailboxes = vi.fn();
const createCrmMailbox = vi.fn();
const updateCrmMailbox = vi.fn();
const testImapConfig = vi.fn();
vi.mock("@/lib/api", () => ({
  systemApi: {
    getEmailConfig: (...a: unknown[]) => getEmailConfig(...a),
    updateEmailConfig: vi.fn(),
    testEmailConfig: vi.fn(),
    testImapConfig: (...a: unknown[]) => testImapConfig(...a),
    listCrmMailboxes: (...a: unknown[]) => listCrmMailboxes(...a),
    createCrmMailbox: (...a: unknown[]) => createCrmMailbox(...a),
    updateCrmMailbox: (...a: unknown[]) => updateCrmMailbox(...a),
  },
}));

import CrmInboxesBlock from "./CrmInboxesBlock";
import { EmailConfigProvider } from "./EmailServerBlocks";

const HELLO = {
  id: "m1", prefix: "crm", address: "hello@example.com", display_name: "Your Company", purpose: "general",
  signature_html: null, roles: null, active: true, is_default: true, sort: 0, connected_at: "2026-01-01T00:00:00Z",
  backfill_days: 90, backfill_done: true,
};
const SALES = {
  ...HELLO, id: "m2", prefix: "crm_sales", address: "sales@example.com", display_name: "Your Company Sales", purpose: "sales",
  is_default: false, sort: 1, active: true, backfill_done: false, backfill_days: 60,
};

function renderBlock() {
  return render(<EmailConfigProvider><CrmInboxesBlock /></EmailConfigProvider>);
}

beforeEach(() => {
  getEmailConfig.mockReset().mockResolvedValue({ data: {
    crm_imap_host: "imap.gmail.com", crm_imap_user: "hello@example.com", crm_imap_password: "••••••••",
    crm_sales_imap_host: "", crm_sales_imap_user: "", crm_sales_imap_password: "",
  } });
  listCrmMailboxes.mockReset().mockResolvedValue({ data: { mailboxes: [SALES, HELLO] } });
  createCrmMailbox.mockReset();
  updateCrmMailbox.mockReset();
  testImapConfig.mockReset().mockResolvedValue({});
});

describe("Settings → Email & WhatsApp → CRM inboxes", () => {
  it("lists each inbox with its purpose, default, state and history import", async () => {
    renderBlock();
    const list = await screen.findByRole("list");
    const rows = within(list).getAllByRole("listitem");
    expect(rows[0].textContent).toContain("hello@example.com");
    expect(within(rows[0]).getByText("Default")).toBeTruthy();
    expect(within(rows[0]).getByText(/history imported/)).toBeTruthy();
    expect(within(rows[1]).getByText("Sales")).toBeTruthy();
    expect(within(rows[1]).getByText(/importing history/)).toBeTruthy();
    expect(within(rows[1]).getByText(/history 60 days/)).toBeTruthy();
    // each inbox gets its own server fields
    expect(await screen.findByText("sales@example.com, incoming (IMAP)")).toBeTruthy();
    expect(screen.getByText("hello@example.com, outgoing (SMTP)")).toBeTruthy();
  });

  it("tests an inbox's IMAP server with its own scope", async () => {
    renderBlock();
    const section = await screen.findByRole("region", { name: "sales@example.com servers" });
    await userEvent.click(within(section).getByRole("button", { name: /Test IMAP connection/ }));
    await waitFor(() => expect(testImapConfig).toHaveBeenCalledWith(expect.objectContaining({ scope: "crm_sales" })));
  });

  it("shows why an inbox could not be switched on", async () => {
    updateCrmMailbox.mockRejectedValue({ response: { status: 400, data: { detail: "Set the IMAP host, user and password first." } } });
    listCrmMailboxes.mockResolvedValue({ data: { mailboxes: [HELLO, { ...SALES, active: false }] } });
    renderBlock();
    await userEvent.click(await screen.findByRole("switch", { name: "sales@example.com on" }));
    expect(await screen.findByText("Set the IMAP host, user and password first.")).toBeTruthy();
    expect(updateCrmMailbox).toHaveBeenCalledWith("m2", { active: true });
  });

  it("adds an inbox switched off, with its history days", async () => {
    listCrmMailboxes.mockResolvedValue({ data: { mailboxes: [HELLO] } });
    createCrmMailbox.mockResolvedValue({ data: { mailbox: { ...SALES, active: false } } });
    renderBlock();
    expect(await screen.findByText(/switched on/)).toBeTruthy();
    await userEvent.type(screen.getByLabelText("Address"), "sales@example.com");
    await userEvent.type(screen.getByLabelText("Display name"), "Your Company Sales");
    await userEvent.selectOptions(screen.getByLabelText("Purpose"), "sales");
    const days = screen.getByLabelText(/History to import/);
    await userEvent.clear(days);
    await userEvent.type(days, "60");
    await userEvent.click(screen.getByRole("button", { name: /Add inbox/ }));
    await waitFor(() => expect(createCrmMailbox).toHaveBeenCalledWith({
      address: "sales@example.com", display_name: "Your Company Sales", purpose: "sales", backfill_days: 60,
    }));
    expect(await screen.findByText("sales@example.com, incoming (IMAP)")).toBeTruthy();
    expect(getEmailConfig).toHaveBeenCalledTimes(2);
  });
});
