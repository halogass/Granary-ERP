"use client";

import { useEffect, useRef, useState, useCallback } from "react";
import { messagingApi, channelApi, callsApi } from "@/lib/api";
import { openTicketedSocket } from "@/lib/wsTicket";
import { callPageUrl } from "@/lib/callLinks";
import { useSession } from "next-auth/react";
import { Phone, Video, MoreVertical, Search, X, Hash, Send, ScrollText, ChevronLeft, LogOut, Trash2, Users } from "lucide-react";
import Sidebar, { ChannelItem, ConversationItem } from "./_components/Sidebar";
import MessageBubble, { Message, Reaction, MemoData } from "./_components/MessageBubble";
import Composer from "./_components/Composer";
import ThreadPanel from "./_components/ThreadPanel";
import ChannelMembersDialog from "./_components/ChannelMembersDialog";
import { MentionItem } from "./_components/MentionDropdown";
import Avatar from "@/components/common/Avatar";
import {
  Button, Dialog, DropdownMenu, EmptyState, Field, IconButton, Input, MenuItem, Select, Skeleton, Textarea,
  FullHeightPage,
} from "@/components/ui";
import { cn } from "@/lib/utils";
import { useUrlParam } from "@/hooks/useUrlParam";
import { apiWsBase } from "@/lib/publicUrls";

// Types
interface ActiveRoom {
  type: "dm" | "channel";
  id: string; // user_id for DM, channel_id for channel
  name: string;
  channelItem?: ChannelItem;
}

interface TypingState {
  [roomKey: string]: { name: string; clearAt: number }[];
}

// Main Page
export default function MessagingPage() {
  const { data: session } = useSession();
  const myId = (session?.user as { id?: string })?.id ?? "";
  const myName = session?.user?.name ?? "Me";
  const isSuperadmin = (session?.user as { is_superadmin?: boolean })?.is_superadmin ?? false;

  // Data
  const [channels, setChannels] = useState<ChannelItem[]>([]);
  const [conversations, setConversations] = useState<ConversationItem[]>([]);
  const [users, setUsers] = useState<{ id: string; name: string }[]>([]);
  const [online, setOnline] = useState<string[]>([]);
  const [messages, setMessages] = useState<Message[]>([]);
  const [reactionsMap, setReactionsMap] = useState<Record<string, Reaction[]>>({});
  const [loadingMsgs, setLoadingMsgs] = useState(false);

  // UI state
  const [activeRoom, setActiveRoom] = useState<ActiveRoom | null>(null);
  const [search, setSearch] = useState("");
  const [searchQuery, setSearchQuery] = useState("");
  const [showSearch, setShowSearch] = useState(false);
  const [searchResults, setSearchResults] = useState<string[]>([]); // message IDs matching search
  const [threadMsg, setThreadMsg] = useState<Message | null>(null);
  const [typingState, setTypingState] = useState<TypingState>({});

  // Mobile: show chat panel vs show sidebar list. The open room is in the URL (?room=dm:<id> or
  // channel:<id>) so the phone Back button returns to the list.
  const [mobileShowChat, setMobileShowChat] = useState(false);
  const [roomParam, setRoomParam] = useUrlParam("room");

  // More dropdown
  const [showMore, setShowMore] = useState(false);
  // Channel members (owners invite and remove there)
  const [showMembers, setShowMembers] = useState(false);

  // Memo composer state
  const [showMemoModal, setShowMemoModal] = useState(false);
  const [memoSubject, setMemoSubject] = useState("");
  const [memoType, setMemoType] = useState("information");
  const [memoBody, setMemoBody] = useState("");
  const [sendingMemo, setSendingMemo] = useState(false);

  const wsRef = useRef<WebSocket | null>(null);
  const bottomRef = useRef<HTMLDivElement>(null);

  // Data loading

  const loadChannels = useCallback(async () => {
    try { const r = await channelApi.list(); setChannels(r.data); } catch { /* empty */ }
  }, []);

  const loadConversations = useCallback(async () => {
    try { const r = await messagingApi.listConversations(); setConversations(r.data); } catch { /* empty */ }
  }, []);

  useEffect(() => {
    loadChannels();
    loadConversations();
    // The messaging directory (any internal user may read it), not the admin users list (403 for staff).
    messagingApi.people().then((r) => {
      setUsers((r.data as { id: string; name: string }[]).map((u) => ({ id: u.id, name: u.name ?? "" })));
    }).catch(() => {});
    messagingApi.onlineUsers().then((r) => setOnline(r.data.online ?? [])).catch(() => {});
  }, [loadChannels, loadConversations]);

  // WebSocket

  useEffect(() => {
    if (!myId) return;
    const wsUrl = apiWsBase();
    let ping: ReturnType<typeof setInterval> | undefined;
    // Stage 8: a single-use ticket opens the socket (no access token in the URL)
    const close = openTicketedSocket(`${wsUrl}/api/v1/messaging/ws/${myId}`, (ws) => {
      wsRef.current = ws;

      ws.onmessage = (e) => {
        try {
          const msg = JSON.parse(e.data);

          if (msg.type === "pong") return;

          if (msg.type === "typing") {
            const roomKey = msg.room;
            setTypingState((prev) => {
              const current = (prev[roomKey] ?? []).filter((t) => t.name !== msg.typer_name);
              if (msg.is_typing) {
                return { ...prev, [roomKey]: [...current, { name: msg.typer_name, clearAt: Date.now() + 3000 }] };
              }
              return { ...prev, [roomKey]: current };
            });
            return;
          }

          if (msg.type === "dm_reaction" || msg.type === "channel_reaction") {
            setReactionsMap((prev) => ({ ...prev, [msg.message_id]: msg.reactions }));
            return;
          }

          if (msg.type === "dm_thread_update" || msg.type === "channel_thread_update") {
            setMessages((prev) =>
              prev.map((m) => m.id === msg.root_id ? { ...m, thread_count: msg.thread_count } : m)
            );
            return;
          }

          // Removed from a channel, or left it in another tab: close it here too and refresh the list
          if (msg.type === "channel_membership_removed") {
            if (activeRoom?.type === "channel" && activeRoom.id === msg.channel_id) {
              setActiveRoom(null);
              setMobileShowChat(false);
              setRoomParam(null);
              setMessages([]);
            }
            loadChannels();
            return;
          }

          // Memo status updates
          if (msg.type === "memo_acknowledged" || msg.type === "memo_recalled") {
            setMessages((prev) =>
              prev.map((m) => m.memo_id === msg.memo_id ? { ...m, memo: msg.memo } : m)
            );
            return;
          }

          // Regular message (dm or channel)
          const isForCurrentRoom =
            (msg.source === "dm" &&
              activeRoom?.type === "dm" &&
              (msg.sender_id === activeRoom.id || msg.receiver_id === activeRoom.id)) ||
            (msg.source === "channel" &&
              activeRoom?.type === "channel" &&
              msg.channel_id === activeRoom.id);

          if (isForCurrentRoom) {
            setMessages((prev) => [...prev, { ...msg, source: msg.source ?? "dm" }]);
          }

          // Refresh conversations/channels for unread counts
          loadConversations();
          loadChannels();
        } catch { /* ignore parse errors */ }
      };

      ping = setInterval(() => {
        if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: "ping" }));
      }, 25000);
    });

    return () => { close(); if (ping) clearInterval(ping); };
  }, [myId, activeRoom, loadConversations, loadChannels, setRoomParam]);

  // Open room

  const openDm = useCallback(async (userId: string, userName: string) => {
    setRoomParam(`dm:${userId}`);
    setShowMore(false);
    setActiveRoom({ type: "dm", id: userId, name: userName });
    setMobileShowChat(true);
    setThreadMsg(null);
    setShowSearch(false);
    setSearchQuery("");
    setLoadingMsgs(true);
    try {
      const r = await messagingApi.getMessages(userId);
      setMessages(r.data);
    } catch { setMessages([]); }
    finally { setLoadingMsgs(false); }
  }, [setRoomParam]);

  const openChannel = useCallback(async (ch: ChannelItem) => {
    // Auto-join if not already a member
    if (!ch.is_member) {
      try { await channelApi.join(ch.id); await loadChannels(); } catch { /* already member */ }
    }
    setRoomParam(`channel:${ch.id}`);
    setActiveRoom({ type: "channel", id: ch.id, name: ch.name, channelItem: ch });
    setMobileShowChat(true);
    setThreadMsg(null);
    setShowSearch(false);
    setShowMore(false);
    setSearchQuery("");
    setLoadingMsgs(true);
    try {
      const r = await channelApi.messages(ch.id);
      setMessages(r.data);
    } catch { setMessages([]); }
    finally { setLoadingMsgs(false); }
  }, [loadChannels, setRoomParam]);

  const closeRoom = useCallback(() => { setMobileShowChat(false); setRoomParam(null); }, [setRoomParam]);

  // Back/forward: the URL says which room is open (the list is the phone's first pane).
  useEffect(() => {
    if (!roomParam) { setMobileShowChat(false); return; }
    const [type, id] = roomParam.split(":");
    if (activeRoom && activeRoom.type === type && activeRoom.id === id) { setMobileShowChat(true); return; }
    if (type === "channel") {
      const ch = channels.find((c) => c.id === id);
      if (ch) openChannel(ch);
    } else if (type === "dm") {
      const name = conversations.find((c) => c.other_user_id === id)?.other_user_name
        ?? users.find((u) => u.id === id)?.name;
      if (name) openDm(id, name);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [roomParam, channels.length > 0, conversations.length > 0, users.length > 0]);

  // Auto-scroll
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  // Typing cleanup
  useEffect(() => {
    const interval = setInterval(() => {
      const now = Date.now();
      setTypingState((prev) => {
        const next: TypingState = {};
        for (const [k, v] of Object.entries(prev)) {
          const filtered = v.filter((t) => t.clearAt > now);
          if (filtered.length) next[k] = filtered;
        }
        return next;
      });
    }, 1000);
    return () => clearInterval(interval);
  }, []);

  // Send message
  const handleSend = useCallback(async (payload: {
    content: string; message_type: string;
    file_url?: string; file_name?: string; file_size?: number; mime_type?: string;
    mentions: MentionItem[];
  }) => {
    if (!activeRoom) return;

    const mentionData = payload.mentions.map((m) => ({ type: m.type, id: m.id, name: m.name }));

    if (activeRoom.type === "dm") {
      await messagingApi.sendMessage({
        receiver_id: activeRoom.id,
        content: payload.content,
        message_type: payload.message_type,
        file_url: payload.file_url,
        file_name: payload.file_name,
        file_size: payload.file_size,
        mime_type: payload.mime_type,
        mentions: mentionData,
      });
    } else {
      await channelApi.sendMessage(activeRoom.id, {
        content: payload.content,
        message_type: payload.message_type,
        file_url: payload.file_url,
        file_name: payload.file_name,
        file_size: payload.file_size,
        mime_type: payload.mime_type,
        mentions: mentionData,
      });
    }
  }, [activeRoom]);

  // Thread send
  const handleThreadSend = useCallback(async (payload: {
    content: string; message_type: string;
    file_url?: string; file_name?: string; file_size?: number; mime_type?: string;
    mentions: MentionItem[];
  }) => {
    if (!threadMsg || !activeRoom) return;
    const mentionData = payload.mentions.map((m) => ({ type: m.type, id: m.id, name: m.name }));

    if (activeRoom.type === "dm") {
      await messagingApi.sendMessage({
        receiver_id: activeRoom.id,
        content: payload.content,
        message_type: payload.message_type,
        file_url: payload.file_url,
        file_name: payload.file_name,
        file_size: payload.file_size,
        mime_type: payload.mime_type,
        reply_to_id: threadMsg.id,
        mentions: mentionData,
      });
    } else {
      await channelApi.sendMessage(activeRoom.id, {
        content: payload.content,
        message_type: payload.message_type,
        file_url: payload.file_url,
        file_name: payload.file_name,
        file_size: payload.file_size,
        mime_type: payload.mime_type,
        reply_to_id: threadMsg.id,
        mentions: mentionData,
      });
    }
  }, [threadMsg, activeRoom]);

  // Typing indicator
  const handleTyping = useCallback((isTyping: boolean) => {
    if (!activeRoom || !wsRef.current || wsRef.current.readyState !== WebSocket.OPEN) return;
    const room = activeRoom.type === "dm" ? `dm:${activeRoom.id}` : `channel:${activeRoom.id}`;
    wsRef.current.send(JSON.stringify({ type: isTyping ? "typing_start" : "typing_stop", room }));
  }, [activeRoom]);

  // Search
  const handleSearch = useCallback(async (q: string) => {
    setSearchQuery(q);
    if (!q.trim() || !activeRoom) { setSearchResults([]); return; }
    try {
      let results: { id: string }[] = [];
      if (activeRoom.type === "dm") {
        const r = await messagingApi.searchDm(activeRoom.id, q);
        results = r.data;
      } else {
        const r = await channelApi.search(activeRoom.id, q);
        results = r.data;
      }
      setSearchResults(results.map((m) => m.id));
    } catch { setSearchResults([]); }
  }, [activeRoom]);

  // Send formal memo
  const handleSendMemo = useCallback(async () => {
    if (!activeRoom || !memoSubject.trim() || !memoBody.trim()) return;
    setSendingMemo(true);
    try {
      await messagingApi.createMemo({
        subject: memoSubject.trim(),
        memo_type: memoType,
        body: memoBody.trim(),
        ...(activeRoom.type === "dm" ? { recipient_id: activeRoom.id } : { channel_id: activeRoom.id }),
      });
      setShowMemoModal(false);
      setMemoSubject("");
      setMemoBody("");
      setMemoType("information");
    } catch { /* ignore */ } finally { setSendingMemo(false); }
  }, [activeRoom, memoSubject, memoType, memoBody]);

  // Called when a memo card's status changes (ack/recall)
  const handleMemoChange = useCallback((memoId: string, memo: MemoData) => {
    setMessages((prev) => prev.map((m) => m.memo_id === memoId ? { ...m, memo } : m));
  }, []);

  const handleStartCall = useCallback(async (audioOnly = false) => {
    if (!activeRoom || activeRoom.type !== "dm") return;
    try {
      const res = await callsApi.createRoom({
        name: `${audioOnly ? "Voice" : "Video"} call with ${activeRoom.name}`,
        invited_user_ids: [activeRoom.id],
      });
      const roomId = res.data.room_id;
      // The member call page (not the guest page /meet/<id>, where the caller would have to ask to be
      // let into their own room); ?join= joins as the room's host.
      window.open(callPageUrl(roomId, audioOnly), "_blank");
    } catch { /* ignore */ }
  }, [activeRoom]);

  const handleLeaveChannel = useCallback(async () => {
    if (!activeRoom || activeRoom.type !== "channel") return;
    try {
      await channelApi.leave(activeRoom.id);
      setActiveRoom(null);
      closeRoom();
      setMessages([]);
      await loadChannels();
    } catch { /* ignore */ }
    setShowMore(false);
  }, [activeRoom, loadChannels, closeRoom]);

  const handleDeleteChannel = useCallback(async () => {
    if (!activeRoom || activeRoom.type !== "channel") return;
    try {
      await channelApi.del(activeRoom.id);
      setActiveRoom(null);
      closeRoom();
      setMessages([]);
      await loadChannels();
    } catch { /* ignore */ }
    setShowMore(false);
  }, [activeRoom, loadChannels, closeRoom]);

  // Current room typing string
  const currentRoomKey = activeRoom
    ? activeRoom.type === "dm" ? `dm:${activeRoom.id}` : `channel:${activeRoom.id}`
    : "";
  const currentTypers = typingState[currentRoomKey] ?? [];
  const typingStr = currentTypers.length === 0 ? null
    : currentTypers.length === 1 ? `${currentTypers[0].name} is typing`
    : `${currentTypers.map((t) => t.name).join(", ")} are typing`;

  // Group messages by date
  const grouped = messages.reduce<{ date: string; items: Message[] }[]>((acc, msg) => {
    const d = new Date(msg.created_at).toLocaleDateString("en-GB", {
      day: "numeric", month: "long", year: "numeric",
    });
    const last = acc[acc.length - 1];
    if (last?.date === d) { last.items.push(msg); } else { acc.push({ date: d, items: [msg] }); }
    return acc;
  }, []);

  // Check if online
  const isRoomOnline = activeRoom?.type === "dm" ? online.includes(activeRoom.id) : false;

  return (
    <FullHeightPage className="bg-canvas">

      {showMembers && activeRoom?.type === "channel" && (
        <ChannelMembersDialog channelId={activeRoom.id} channelName={activeRoom.name}
          channelType={activeRoom.channelItem?.type} myId={myId} users={users}
          onClose={() => setShowMembers(false)} />
      )}

      {/* Formal memo composer */}
      {showMemoModal && (
        <Dialog open onOpenChange={(o) => { if (!o) setShowMemoModal(false); }} title="Compose formal memo"
          footer={<>
            <Button onClick={() => setShowMemoModal(false)}>Cancel</Button>
            <Button variant="primary" loading={sendingMemo} onClick={handleSendMemo}
              disabled={!memoSubject.trim() || !memoBody.trim()}>
              <ScrollText aria-hidden /> Send signed memo
            </Button>
          </>}>
          <div className="space-y-4">
            {activeRoom && (
              <Field label="To">
                <Input readOnly value={activeRoom.type === "channel" ? `# ${activeRoom.name}` : activeRoom.name} className="bg-surface-2" />
              </Field>
            )}
            <Field label="Memo type">
              <Select value={memoType} onChange={(e) => setMemoType(e.target.value)} options={[
                { value: "directive", label: "Directive: an order or instruction that must be followed" },
                { value: "information", label: "Information / notice: for awareness, no action required" },
                { value: "request", label: "Request: formally requesting something" },
                { value: "approval", label: "Approval / decision: documenting a decision" },
              ]} />
            </Field>
            <Field label="Subject" required>
              <Input value={memoSubject} onChange={(e) => setMemoSubject(e.target.value)}
                placeholder="e.g. Submit Q2 project reports by 30 May 2026" />
            </Field>
            <Field label="Memo content" required>
              <Textarea value={memoBody} onChange={(e) => setMemoBody(e.target.value)} rows={7} className="resize-none"
                placeholder="Write the full memo content here. This will appear in the signed document." />
            </Field>
            <p className="rounded-lg bg-surface-2 px-3 py-2 text-xs text-fg-muted">
              This memo will be <strong className="font-medium text-fg">digitally signed</strong> with your name and position upon sending.
              The recipient must acknowledge receipt.
            </p>
          </div>
        </Dialog>
      )}

      {/* Sidebar: on mobile only when no chat is open; always on desktop */}
      <div className={mobileShowChat ? "hidden md:contents" : "contents"}>
        <Sidebar
          channels={channels}
          conversations={conversations}
          online={online}
          activeRoom={activeRoom}
          myName={myName}
          myId={myId}
          search={search}
          onSearchChange={setSearch}
          onSelectChannel={openChannel}
          onSelectDm={openDm}
          onChannelsChange={loadChannels}
          users={users}
          isSuperadmin={isSuperadmin}
        />
      </div>

      {/* Chat area: on mobile only when a chat is selected */}
      <div className={cn(mobileShowChat ? "flex" : "hidden md:flex", "min-w-0 flex-1 overflow-hidden")}>
      {!activeRoom ? (
        <div className="flex flex-1 flex-col items-center justify-center bg-canvas">
          <EmptyState icon={Send} title="Messaging" description="Pick a channel or direct message to start collaborating with your team." />
        </div>
      ) : (
        <div className="flex min-w-0 flex-1 overflow-hidden">
          <div className="flex min-w-0 flex-1 flex-col bg-canvas">

            {/* Chat header */}
            <div className="flex shrink-0 items-center justify-between border-b border-line bg-surface px-3 py-2.5 md:px-5">
              <div className="flex min-w-0 items-center gap-2 md:gap-3">
                <IconButton icon={ChevronLeft} label="Back to conversations" onClick={closeRoom} className="md:hidden" />
                {activeRoom.type === "channel" ? (
                  <div className="flex size-9 shrink-0 items-center justify-center rounded-full bg-surface-2" aria-hidden>
                    <Hash className="size-4 text-fg-muted" />
                  </div>
                ) : (
                  <Avatar name={activeRoom.name} online={isRoomOnline} />
                )}
                <div className="min-w-0">
                  <h1 className="truncate text-sm font-semibold leading-tight text-fg">
                    {activeRoom.type === "channel" ? `# ${activeRoom.name}` : activeRoom.name}
                  </h1>
                  <p className="text-xs text-fg-muted">
                    {activeRoom.type === "channel"
                      ? activeRoom.channelItem?.type === "private" ? "Private channel" : "Public channel"
                      : isRoomOnline ? "Online" : "Offline"}
                  </p>
                </div>
              </div>

              <div className="flex shrink-0 items-center gap-0.5">
                <Button size="sm" onClick={() => setShowMemoModal(true)} title="Send formal memo" className="mr-1">
                  <ScrollText aria-hidden /> <span className="max-md:sr-only">Memo</span>
                </Button>
                <IconButton icon={Search} label="Search" aria-pressed={showSearch}
                  onClick={() => { setShowSearch((v) => !v); if (showSearch) { setSearchQuery(""); setSearchResults([]); } }}
                  className={showSearch ? "bg-brand/[0.07] text-brand-ink" : undefined} />
                {activeRoom.type === "dm" && (
                  <>
                    {/* On phones both calls are in the More menu, so the room name keeps its room */}
                    <IconButton icon={Phone} label="Voice call" onClick={() => handleStartCall(true)} className="max-md:hidden" />
                    <IconButton icon={Video} label="Video call" onClick={() => handleStartCall(false)} className="max-md:hidden" />
                  </>
                )}
                <DropdownMenu open={showMore} onOpenChange={setShowMore}
                  trigger={<IconButton icon={MoreVertical} label="More" />}>
                  {activeRoom.type === "dm" && (
                    <>
                      <MenuItem icon={Phone} onSelect={() => { handleStartCall(true); setShowMore(false); }}>Voice call</MenuItem>
                      <MenuItem icon={Video} onSelect={() => { handleStartCall(false); setShowMore(false); }}>Video call</MenuItem>
                    </>
                  )}
                  {activeRoom.type === "channel" && (
                    <MenuItem icon={Users} onSelect={() => { setShowMembers(true); setShowMore(false); }}>Members</MenuItem>
                  )}
                  {activeRoom.type === "channel" && activeRoom.channelItem?.is_member && activeRoom.channelItem.type !== "announcement" && (
                    <MenuItem danger icon={LogOut} onSelect={handleLeaveChannel}>Leave channel</MenuItem>
                  )}
                  {activeRoom.type === "channel" && (isSuperadmin || activeRoom.channelItem?.created_by_id === myId) && (
                    <MenuItem danger icon={Trash2} onSelect={handleDeleteChannel}>Delete channel</MenuItem>
                  )}
                </DropdownMenu>
              </div>
            </div>

            {/* Search bar (when open) */}
            {showSearch && (
              <div className="flex shrink-0 items-center gap-2 border-b border-line bg-surface px-3 py-2 md:px-5">
                <div className="flex-1">
                  <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
                    autoFocus value={searchQuery} onChange={(e) => handleSearch(e.target.value)}
                    placeholder="Search messages" aria-label="Search messages" />
                </div>
                {searchQuery && (
                  <IconButton size="sm" icon={X} label="Clear search" onClick={() => { setSearchQuery(""); setSearchResults([]); }} />
                )}
                {searchResults.length > 0 && (
                  <span role="status" className="text-xs text-fg-muted tabular">{searchResults.length} result{searchResults.length > 1 ? "s" : ""}</span>
                )}
              </div>
            )}

            {/* Messages */}
            <div className="flex-1 space-y-1 overflow-y-auto px-3 py-4 md:px-5">
              {loadingMsgs ? (
                <div className="space-y-4" aria-busy aria-label="Loading messages">
                  {[0, 1, 2, 3].map((i) => (
                    <div key={i} className="flex gap-2.5">
                      <Skeleton className="size-8 rounded-full" />
                      <div className="flex-1 space-y-1.5">
                        <Skeleton className="h-3 w-32" />
                        <Skeleton className="h-8 w-2/3" />
                      </div>
                    </div>
                  ))}
                </div>
              ) : messages.length === 0 ? (
                <EmptyState icon={activeRoom.type === "channel" ? Hash : Send} className="h-full"
                  title={activeRoom.type === "channel" ? `Welcome to #${activeRoom.name}` : "No messages yet"}
                  description={activeRoom.type === "channel" ? undefined : "Say hi."} />
              ) : (
                grouped.map(({ date, items }) => (
                  <div key={date}>
                    <div className="my-5 flex items-center gap-3">
                      <div className="h-px flex-1 bg-line" />
                      <span className="px-2 text-xs font-medium text-fg-muted">{date}</span>
                      <div className="h-px flex-1 bg-line" />
                    </div>
                    <div className="space-y-2">
                      {items.map((msg, i) => {
                        const isMe = msg.sender_id === myId;
                        const prevMsg = items[i - 1];
                        const showAvatar = !isMe && (i === 0 || prevMsg?.sender_id !== msg.sender_id);
                        const isSearchMatch = searchResults.includes(msg.id);
                        return (
                          <MessageBubble
                            key={msg.id}
                            msg={msg}
                            isMe={isMe}
                            showAvatar={showAvatar}
                            myId={myId}
                            reactions={reactionsMap[msg.id] ?? []}
                            onReplyInThread={setThreadMsg}
                            onMemoChange={handleMemoChange}
                            searchQuery={isSearchMatch ? searchQuery : undefined}
                          />
                        );
                      })}
                    </div>
                  </div>
                ))
              )}
              <div ref={bottomRef} />
            </div>

            {/* Typing indicator */}
            {typingStr && (
              <div className="flex items-center gap-2 px-5 pb-1" role="status">
                <div className="flex gap-0.5" aria-hidden>
                  {[0, 1, 2].map((i) => (
                    <div key={i} className="size-1.5 animate-bounce rounded-full bg-fg-subtle" style={{ animationDelay: `${i * 100}ms` }} />
                  ))}
                </div>
                <span className="text-xs text-fg-muted">{typingStr}</span>
              </div>
            )}

            <Composer
              placeholder={activeRoom.type === "channel" ? `Message #${activeRoom.name}` : `Message ${activeRoom.name}`}
              onSend={handleSend}
              onTyping={handleTyping}
            />
          </div>

          {threadMsg && (
            <ThreadPanel
              rootMessage={threadMsg}
              myId={myId}
              onClose={() => setThreadMsg(null)}
              onSend={handleThreadSend}
            />
          )}
        </div>
      )}
      </div>
    </FullHeightPage>
  );
}
