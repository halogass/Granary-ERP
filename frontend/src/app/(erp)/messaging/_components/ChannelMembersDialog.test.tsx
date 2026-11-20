import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import ChannelMembersDialog from "./ChannelMembersDialog";

const get = vi.fn();
const invite = vi.fn();
const removeMember = vi.fn();
vi.mock("@/lib/api", () => ({
  channelApi: {
    get: (...a: unknown[]) => get(...a),
    invite: (...a: unknown[]) => invite(...a),
    removeMember: (...a: unknown[]) => removeMember(...a),
  },
}));

const MEMBERS = [
  { user_id: "me", name: "Ana Owner", role: "owner" },
  { user_id: "u2", name: "Budi Member", role: "member" },
];
const USERS = [
  { id: "me", name: "Ana Owner" },
  { id: "u2", name: "Budi Member" },
  { id: "u3", name: "Citra New" },
];

describe("channel members: invite and remove for owners", () => {
  beforeEach(() => {
    get.mockReset(); invite.mockReset(); removeMember.mockReset();
    get.mockResolvedValue({ data: { id: "c1", type: "private", members: MEMBERS } });
    invite.mockResolvedValue({ data: { status: "invited" } });
    removeMember.mockResolvedValue({ data: null });
    vi.spyOn(window, "confirm").mockReturnValue(true);
  });

  it("an owner invites someone who is not a member yet", async () => {
    render(<ChannelMembersDialog channelId="c1" channelName="ops" channelType="private" myId="me" users={USERS} onClose={() => {}} />);
    const select = (await screen.findByLabelText("Add a member")) as HTMLSelectElement;
    // members are not offered again
    expect(Array.from(select.options).map((o) => o.value)).toEqual(["", "u3"]);
    fireEvent.change(select, { target: { value: "u3" } });
    fireEvent.click(screen.getByRole("button", { name: /Add/ }));
    await waitFor(() => expect(invite).toHaveBeenCalledWith("c1", "u3"));
    await waitFor(() => expect(get).toHaveBeenCalledTimes(2));
  });

  it("an owner removes a member but never an owner", async () => {
    render(<ChannelMembersDialog channelId="c1" channelName="ops" channelType="private" myId="me" users={USERS} onClose={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "Remove Budi Member" }));
    await waitFor(() => expect(removeMember).toHaveBeenCalledWith("c1", "u2"));
    expect(screen.queryByRole("button", { name: "Remove Ana Owner" })).toBeNull();
  });

  it("a plain member only sees the list", async () => {
    render(<ChannelMembersDialog channelId="c1" channelName="ops" channelType="private" myId="u2" users={USERS} onClose={() => {}} />);
    expect(await screen.findByText("Ana Owner")).toBeTruthy();
    expect(screen.queryByLabelText("Add a member")).toBeNull();
    expect(screen.queryByRole("button", { name: /^Remove/ })).toBeNull();
  });

  it("shows the API's refusal", async () => {
    removeMember.mockRejectedValue({ response: { data: { detail: "Only the channel owner can remove members" } } });
    render(<ChannelMembersDialog channelId="c1" channelName="ops" channelType="private" myId="me" users={USERS} onClose={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "Remove Budi Member" }));
    expect(await screen.findByText("Only the channel owner can remove members")).toBeTruthy();
  });

  it("any member adds people to a public channel (the API's rule); outsiders cannot", async () => {
    get.mockResolvedValue({ data: { id: "c1", type: "public", members: MEMBERS } });
    render(<ChannelMembersDialog channelId="c1" channelName="ops" channelType="public" myId="u2" users={USERS} onClose={() => {}} />);
    expect(await screen.findByLabelText("Add a member")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Remove/ })).toBeNull();
  });

  it("someone outside a public channel only sees the list", async () => {
    get.mockResolvedValue({ data: { id: "c1", type: "public", members: MEMBERS } });
    render(<ChannelMembersDialog channelId="c1" channelName="ops" channelType="public" myId="u3" users={USERS} onClose={() => {}} />);
    expect(await screen.findByText("Ana Owner")).toBeTruthy();
    expect(screen.queryByLabelText("Add a member")).toBeNull();
  });
});
