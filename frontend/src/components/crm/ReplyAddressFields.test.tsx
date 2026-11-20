import { describe, it, expect, vi } from "vitest";
import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const replyDefaults = vi.fn();
vi.mock("@/lib/api", () => ({
  crmInboxApi: { replyDefaults: (...a: unknown[]) => replyDefaults(...a) },
}));

import ReplyAddressFields, {
  EMPTY_ADDRESSING, addressingFor, addressingSummary, appendAddressing, type ReplyAddressing,
} from "./ReplyAddressFields";
import type { InboxMailbox } from "@/lib/api";

const BOXES: InboxMailbox[] = [
  { id: "m1", address: "hello@example.com", name: "Your Company", purpose: "general", active: true, default: true },
  { id: "m2", address: "sales@example.com", name: "Your Company Sales", purpose: "sales", active: true, default: false },
  { id: "m3", address: "old@example.com", name: "Old", purpose: "general", active: false, default: false },
];

let latest: ReplyAddressing = EMPTY_ADDRESSING;
function Harness() {
  const [v, setV] = useState<ReplyAddressing>(EMPTY_ADDRESSING);
  latest = v;
  return <ReplyAddressFields threadId="t1" mailboxes={BOXES} value={v} onChange={setV} />;
}

describe("email reply addressing", () => {
  it("prefills From, To, Cc and Subject from the reply defaults and sends what the user leaves", async () => {
    replyDefaults.mockResolvedValue({ data: {
      from_inbox: "sales@example.com", to: ["robert@cust.example"],
      cc: ["rachel@cust.example", "john@cust.example"], subject: "Re: ERP rollout",
    } });
    render(<Harness />);
    const from = await screen.findByLabelText("From inbox");
    expect(replyDefaults).toHaveBeenCalledWith("t1");
    expect((from as HTMLSelectElement).value).toBe("sales@example.com");
    // only active inboxes are offered
    expect(screen.queryByRole("option", { name: /old@example/ })).toBeNull();
    expect((screen.getByLabelText("To") as HTMLInputElement).value).toBe("robert@cust.example");
    expect((screen.getByLabelText("Cc") as HTMLInputElement).value).toBe("rachel@cust.example, john@cust.example");
    expect((screen.getByLabelText("Bcc") as HTMLInputElement).value).toBe("");
    expect((screen.getByLabelText("Subject") as HTMLInputElement).value).toBe("Re: ERP rollout");

    // remove one Cc, add a Bcc, send from hello@
    const cc = screen.getByLabelText("Cc");
    await userEvent.clear(cc);
    await userEvent.type(cc, "rachel@cust.example");
    await userEvent.type(screen.getByLabelText("Bcc"), "owner@example.com");
    await userEvent.selectOptions(from, "hello@example.com");

    const fd = new FormData();
    appendAddressing(fd, latest);
    expect(fd.get("to")).toBe("robert@cust.example");
    expect(fd.get("cc")).toBe("rachel@cust.example");
    expect(fd.get("bcc")).toBe("owner@example.com");
    expect(fd.get("from_inbox")).toBe("hello@example.com");
    expect(fd.get("subject")).toBe("Re: ERP rollout");
  });

  it("adds nothing before the defaults load, so the server's own apply", () => {
    const fd = new FormData();
    appendAddressing(fd, EMPTY_ADDRESSING);
    expect([...fd.keys()]).toEqual([]);
  });

  it("folds into one summary line with Bcc hidden until asked for", async () => {
    const value: ReplyAddressing = { loaded: true, from_inbox: "hello@example.com", to: "rachel@university.example",
      cc: "a@uni.example, b@uni.example, c@uni.example", bcc: "", subject: "Re: University tender" };
    function Folded() {
      const [v, setV] = useState<ReplyAddressing>(value);
      return <ReplyAddressFields threadId="t1" mailboxes={BOXES} value={v} onChange={setV} autoLoad={false} summary
        names={{ "rachel@university.example": "Rachel Green" }} />;
    }
    render(<Folded />);
    expect(replyDefaults).not.toHaveBeenCalled();
    const line = screen.getByRole("button", { name: /To Rachel Green \+3 cc · Re: University tender/ });
    expect(screen.queryByLabelText("To")).toBeNull();

    await userEvent.click(line);
    expect(screen.getByLabelText("To")).toBeInTheDocument();
    expect(screen.queryByLabelText("Bcc")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Bcc" }));
    expect(screen.getByLabelText("Bcc")).toBeInTheDocument();
  });

  it("shapes the addressing for Reply, Reply all and Forward", () => {
    const d = { from_inbox: "sales@example.com", to: ["robert@cust.example"], cc: ["rachel@cust.example"], subject: "Re: ERP" };
    expect(addressingFor("all", d, "ERP")).toMatchObject({ to: "robert@cust.example", cc: "rachel@cust.example" });
    expect(addressingFor("reply", d, "ERP")).toMatchObject({ to: "robert@cust.example", cc: "" });
    expect(addressingFor("forward", d, "Re: ERP")).toMatchObject({ to: "", cc: "", subject: "Fwd: ERP", from_inbox: "sales@example.com" });
    expect(addressingFor("reply", null, "ERP").loaded).toBe(false);
    expect(addressingSummary({ ...EMPTY_ADDRESSING, loaded: true, to: "x@y.example, z@y.example", subject: "Hi" }))
      .toBe("To x@y.example +1 · Hi");
  });
});
