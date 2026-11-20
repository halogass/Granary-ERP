"use client";

import { useEffect, useRef, useState, useCallback } from "react";
import { useSearchParams } from "next/navigation";
import { callsApi, messagingApi } from "@/lib/api";
import { guestPeerId, withPeerName } from "@/lib/callPeers";
import { useSession } from "next-auth/react";
import {
  Video, VideoOff, Mic, MicOff, PhoneOff, Monitor, Users, Plus,
  Copy, Check, Wifi, WifiOff, Send, MessageSquare, Circle,
  UserX, Bell, ChevronRight, Download, Link2, UserPlus, X, Search,
} from "lucide-react";
import Avatar from "@/components/common/Avatar";
import {
  Button, CountBadge, Dialog, EmptyState, IconButton, Input, Pill,
  FullHeightPage,
} from "@/components/ui";
import { cn } from "@/lib/utils";
import { useUrlParam } from "@/hooks/useUrlParam";

interface Room { room_id: string; name: string; is_active: boolean; participants: Participant[]; max_participants: number; is_host: boolean; has_recording: boolean }
interface Participant { user_id: string; name: string; joined_at: string; left_at?: string; is_guest?: boolean }
interface IceServer { urls: string; username?: string; credential?: string }
interface WaitingGuest { id: string; name: string; email?: string; joined_at: string; is_erp_user: boolean; crm_contact_name?: string; crm_org_name?: string }
interface ChatMsg { from: string; display_name?: string; text: string; ts: string; isMe: boolean }
interface InviteUser { id: string; name: string; avatar_url?: string; position?: string }

type CallState = "idle" | "creating" | "lobby" | "in-call" | "ended";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const dbg = process.env.NODE_ENV === "development" ? (...a: any[]) => console.log("[CALL]", ...a) : () => {};

function ControlBtn({ active, onClick, icon: Icon, offIcon: OffIcon, tone = "normal", label, badge }: {
  active: boolean; onClick: () => void; icon: React.ElementType; offIcon?: React.ElementType;
  /** on: a panel or share that is open; warn: muted, camera off, recording. */
  tone?: "normal" | "on" | "warn"; label: string; badge?: number;
}) {
  const Ic = !active && OffIcon ? OffIcon : Icon;
  return (
    <button type="button" onClick={onClick} title={label} aria-label={label}
      className={cn("relative flex size-11 items-center justify-center rounded-lg transition-colors md:size-12",
        "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[rgb(var(--focus)/0.7)]",
        tone === "warn" ? "bg-tone-red-soft text-tone-red hover:opacity-90"
        : tone === "on" ? "bg-brand/[0.1] text-brand-ink hover:bg-brand/[0.15]"
        : "bg-surface-2 text-fg hover:bg-line-soft")}>
      <Ic className="size-5" aria-hidden />
      {badge != null && badge > 0 && (
        <span className="absolute -right-1 -top-1 flex size-5 items-center justify-center rounded-full bg-brand text-xs font-semibold text-brand-on tabular">{badge}</span>
      )}
    </button>
  );
}

function VideoTile({ name, stream, muted = false, micMuted = false, isMe = false, isGuest = false, isHost = false, onKick }: {
  name: string; stream?: MediaStream | null; muted?: boolean; micMuted?: boolean; isMe?: boolean; isGuest?: boolean; isHost?: boolean; onKick?: () => void;
}) {
  const ref = useRef<HTMLVideoElement>(null);
  useEffect(() => { if (ref.current && stream) ref.current.srcObject = stream; }, [stream]);
  return (
    <div className={cn("group relative aspect-video overflow-hidden rounded-lg border bg-surface-2",
      isMe ? "border-brand/40" : "border-line")}>
      {stream
        ? <video ref={ref} autoPlay playsInline muted={muted} className="size-full bg-black object-cover" />
        : <div className="flex size-full items-center justify-center">
            <Avatar name={name} size="xl" />
          </div>
      }
      <div className="absolute inset-x-0 bottom-0 bg-gradient-to-t from-black/70 to-transparent px-3 py-2">
        <div className="flex items-center gap-1.5">
          <span className="truncate text-xs font-medium text-white">{name}{isMe ? " (you)" : ""}</span>
          {isGuest && <Pill tone="orange">Guest</Pill>}
          {micMuted && <MicOff className="size-3 shrink-0 text-white" aria-label="Mic muted" />}
        </div>
      </div>
      {!isMe && isHost && onKick && (
        <IconButton size="sm" icon={UserX} label="Remove from call" onClick={onKick} variant="secondary"
          className="absolute right-2 top-2 text-tone-red transition-opacity focus:opacity-100 group-hover:opacity-100 [@media(hover:hover)]:opacity-0" />
      )}
    </div>
  );
}

export default function CallsPage() {
  const { data: session } = useSession();
  const searchParams = useSearchParams();
  const myName = session?.user?.name ?? "Me";
  // Peer id + signed signal ticket are issued by the backend (create/join/get room).
  const myPeerId = useRef("");
  const signalTicket = useRef("");

  const [callState, setCallState] = useState<CallState>("idle");
  const [room, setRoom] = useState<Room | null>(null);
  const [joinRoomId, setJoinRoomId] = useState("");
  const [iceServers, setIceServers] = useState<IceServer[]>([]);
  const iceServersRef = useRef<IceServer[]>([]);
  const [copied, setCopied] = useState(false);
  const [linkCopied, setLinkCopied] = useState(false);

  const [micOn, setMicOn] = useState(true);
  const [camOn, setCamOn] = useState(true);
  const [screenSharing, setScreenSharing] = useState(false);
  const [connected, setConnected] = useState(false);
  const [isHost, setIsHost] = useState(false);

  // Panels
  const [showWaiting, setShowWaiting] = useState(false);
  const [showChat, setShowChat] = useState(false);
  // The side panels cover the call on phones; they live in the URL so the phone Back button closes
  // them instead of leaving the call.
  const [panelParam, setPanelParam] = useUrlParam("panel");
  useEffect(() => {
    setPanelParam(showChat ? "chat" : showWaiting ? "waiting" : null);
  }, [showChat, showWaiting, setPanelParam]);
  useEffect(() => {
    if (!panelParam) { setShowChat(false); setShowWaiting(false); }
  }, [panelParam]);
  const [waitingGuests, setWaitingGuests] = useState<WaitingGuest[]>([]);
  const [unreadWaiting, setUnreadWaiting] = useState(0);
  const [chatMsgs, setChatMsgs] = useState<ChatMsg[]>([]);
  const [chatInput, setChatInput] = useState("");
  const [unreadChat, setUnreadChat] = useState(0);

  // Recording
  const [recording, setRecording] = useState(false);
  const [recordingReady, setRecordingReady] = useState(false);
  const [uploading, setUploading] = useState(false);
  const mediaRecorderRef = useRef<MediaRecorder | null>(null);
  const recordingChunks = useRef<Blob[]>([]);

  // Remote mic/cam state tracking
  const [remoteMicStates, setRemoteMicStates] = useState<Map<string, boolean>>(new Map());

  // Invite modal (shared for lobby pre-invite and in-call invite)
  const [showInviteModal, setShowInviteModal] = useState(false);
  const [inviteQuery, setInviteQuery] = useState("");
  const [inviteResults, setInviteResults] = useState<InviteUser[]>([]);
  const [inviteLoading, setInviteLoading] = useState(false);
  const [lobbyInvitees, setLobbyInvitees] = useState<InviteUser[]>([]);   // set at lobby, sent with createRoom
  const [callInvitees, setCallInvitees] = useState<InviteUser[]>([]);     // set during call, sent via invite endpoint
  const inviteSearchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const localStream = useRef<MediaStream | null>(null);
  const screenStream = useRef<MediaStream | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const pcsRef = useRef<Map<string, RTCPeerConnection>>(new Map());
  const waitingPollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const [localStreamState, setLocalStreamState] = useState<MediaStream | null>(null);
  const [remoteStreams, setRemoteStreams] = useState<Map<string, MediaStream>>(new Map());
  const [remotePeerNames, setRemotePeerNames] = useState<Map<string, string>>(new Map());

  // Auto-join from ?join= param (notification link)
  const hasAutoJoined = useRef(false);
  useEffect(() => {
    if (!session || hasAutoJoined.current) return;
    const joinId = searchParams.get("join");
    if (joinId) {
      hasAutoJoined.current = true;
      joinRoom(joinId);
    }
  }, [session]); // eslint-disable-line

  // A voice call (?audio=1, the Voice call button in Messages) starts with the camera off; it can be
  // turned on from the call.
  const audioOnly = searchParams.get("audio") === "1";

  async function startMedia() {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ video: true, audio: true });
      if (audioOnly) {
        stream.getVideoTracks().forEach((t) => { t.enabled = false; });
        setCamOn(false);
      }
      localStream.current = stream;
      setLocalStreamState(stream);
    } catch { /* no cam */ }
  }

  async function createRoom() {
    setCallState("creating");
    try {
      const res = await callsApi.createRoom({
        name: `Call ${new Date().toLocaleTimeString()}`,
        invited_user_ids: lobbyInvitees.map((u) => u.id),
      });
      setRoom({
        room_id: res.data.room_id, name: "Call room", is_active: true, participants: [],
        max_participants: res.data.max_participants, is_host: true, has_recording: false,
      });
      myPeerId.current = res.data.peer_id;
      signalTicket.current = res.data.signal_ticket;
      setIceServers(res.data.iceServers);
      iceServersRef.current = res.data.iceServers;
      dbg("room created | ICE servers:", res.data.iceServers, "| signal URL:", process.env.NEXT_PUBLIC_SIGNAL_URL);
      setIsHost(true);
      await startMedia();
      setCallState("lobby");
    } catch { setCallState("idle"); }
  }

  async function joinRoom(overrideId?: string) {
    const rid = (overrideId ?? joinRoomId).trim();
    if (!rid) return;
    setCallState("creating");
    try {
      const res = await callsApi.joinRoom(rid);
      const roomRes = await callsApi.getRoom(rid);
      setRoom(roomRes.data);
      myPeerId.current = res.data.peer_id;
      signalTicket.current = res.data.signal_ticket;
      setIceServers(res.data.iceServers);
      iceServersRef.current = res.data.iceServers;
      dbg("room joined | ICE servers:", res.data.iceServers, "| signal URL:", process.env.NEXT_PUBLIC_SIGNAL_URL);
      setIsHost(res.data.is_host ?? false);
      await startMedia();
      setCallState("lobby");
    } catch { setCallState("idle"); }
  }

  // Poll waiting room (host only, while in-call)
  useEffect(() => {
    if (callState !== "in-call" || !isHost || !room) return;
    waitingPollRef.current = setInterval(async () => {
      try {
        const res = await callsApi.listWaiting(room.room_id);
        const prev = waitingGuests.length;
        setWaitingGuests(res.data.waiting ?? []);
        if ((res.data.count ?? 0) > prev) {
          setUnreadWaiting((res.data.count ?? 0) - prev);
        }
      } catch { /* ignore */ }
    }, 4000);
    return () => clearInterval(waitingPollRef.current!);
  }, [callState, isHost, room]); // eslint-disable-line

  const reconnectTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const reconnectAttempts = useRef(0);

  const connectSignal = useCallback((signalUrl?: string) => {
    if (!room) return;
    const wsUrl = signalUrl ?? (process.env.NEXT_PUBLIC_SIGNAL_URL ?? "ws://localhost:3003");
    dbg("[signal] connecting to", wsUrl, "| room:", room.room_id, "| peer:", myPeerId.current);
    const ws = new WebSocket(`${wsUrl}?room=${room.room_id}&user=${encodeURIComponent(myPeerId.current)}&ticket=${encodeURIComponent(signalTicket.current)}`);
    wsRef.current = ws;
    ws.onopen = () => {
      dbg("[signal] connected → registering");
      setConnected(true);
      reconnectAttempts.current = 0;
      ws.send(JSON.stringify({ type: "register", roomId: room.room_id, userId: myPeerId.current }));
    };
    ws.onclose = (e) => {
      dbg("[signal] closed", e.code, e.reason);
      setConnected(false);
      const attempts = reconnectAttempts.current;
      if (attempts < 8) {
        reconnectAttempts.current += 1;
        const delay = Math.min(1000 * Math.pow(2, attempts), 16000);
        dbg("[signal] reconnecting in", delay, "ms (attempt", attempts + 1, ")");
        reconnectTimerRef.current = setTimeout(() => connectSignal(signalUrl), delay);
      }
    };
    ws.onerror = (e) => dbg("[signal] error", e);
    ws.onmessage = async (e) => {
      try {
        const msg = JSON.parse(e.data);
        dbg("[signal] ←", msg.type, msg);
        if (msg.type === "offer") {
          const pc = createPeerConnection(msg.from);
          await pc.setRemoteDescription(new RTCSessionDescription(msg.sdp));
          const answer = await pc.createAnswer();
          await pc.setLocalDescription(answer);
          dbg("[signal] → answer to", msg.from);
          ws.send(JSON.stringify({ type: "answer", to: msg.from, sdp: answer, display_name: myName }));
          setRemotePeerNames((prev) => withPeerName(prev, msg.from, msg.display_name));
        } else if (msg.type === "answer") {
          dbg("[signal] setting remote answer from", msg.from);
          setRemotePeerNames((prev) => withPeerName(prev, msg.from, msg.display_name));
          await pcsRef.current.get(msg.from)?.setRemoteDescription(new RTCSessionDescription(msg.sdp));
        } else if (msg.type === "ice") {
          const pc = pcsRef.current.get(msg.from);
          if (pc && msg.candidate) await pc.addIceCandidate(new RTCIceCandidate(msg.candidate));
        } else if (msg.type === "registered") {
          dbg("[signal] registered | peers in room:", msg.peers);
          for (const existingPeer of (msg.peers ?? [])) {
            const pc = createPeerConnection(existingPeer);
            const offer = await pc.createOffer();
            await pc.setLocalDescription(offer);
            dbg("[signal] → offer to existing peer", existingPeer);
            ws.send(JSON.stringify({ type: "offer", to: existingPeer, sdp: offer, display_name: myName }));
          }
        } else if (msg.type === "peer_joined") {
          dbg("[signal] peer_joined:", msg.userId, ", waiting for their offer");
        } else if (msg.type === "peer_left") {
          pcsRef.current.get(msg.userId)?.close();
          pcsRef.current.delete(msg.userId);
          setRemoteStreams((prev) => { const m = new Map(prev); m.delete(msg.userId); return m; });
          setRemoteMicStates((prev) => { const m = new Map(prev); m.delete(msg.userId); return m; });
        } else if (msg.type === "media_state") {
          dbg("[signal] media_state from", msg.from, "| micOn:", msg.micOn, "| camOn:", msg.camOn);
          setRemoteMicStates((prev) => new Map(prev).set(msg.from, msg.micOn !== false));
        } else if (msg.type === "chat") {
          const newMsg: ChatMsg = { from: msg.from, display_name: msg.display_name, text: msg.text, ts: msg.ts, isMe: false };
          setChatMsgs((prev) => [...prev, newMsg]);
          setUnreadChat((n) => showChat ? n : n + 1);
        } else if (msg.type === "kick" && msg.target === myPeerId.current) {
          endCall();
        }
      } catch (err) { dbg("[signal] message handler error", err); }
    };
  }, [room, myName, showChat]); // eslint-disable-line

  function createPeerConnection(peerId: string): RTCPeerConnection {
    dbg("creating PC for", peerId, "| ICE servers:", iceServersRef.current.length, iceServersRef.current);
    const pc = new RTCPeerConnection({ iceServers: iceServersRef.current });
    pcsRef.current.set(peerId, pc);
    if (localStream.current) {
      localStream.current.getTracks().forEach((t) => {
        dbg("adding", t.kind, "track for", peerId);
        pc.addTrack(t, localStream.current!);
      });
    } else {
      dbg("⚠ no local stream when creating PC for", peerId);
    }
    pc.onicecandidate = (e) => {
      if (e.candidate) dbg(`[pc:${peerId}] ICE candidate:`, e.candidate.type, e.candidate.address);
      if (e.candidate && wsRef.current)
        wsRef.current.send(JSON.stringify({ type: "ice", to: peerId, candidate: e.candidate }));
    };
    pc.oniceconnectionstatechange = () => dbg(`[pc:${peerId}] ICE connection:`, pc.iceConnectionState);
    pc.onicegatheringstatechange = () => dbg(`[pc:${peerId}] ICE gathering:`, pc.iceGatheringState);
    pc.onconnectionstatechange = () => dbg(`[pc:${peerId}] connection:`, pc.connectionState);
    pc.onsignalingstatechange = () => dbg(`[pc:${peerId}] signaling:`, pc.signalingState);
    pc.ontrack = (e) => {
      dbg(`[pc:${peerId}] track received:`, e.track.kind);
      setRemoteStreams((prev) => new Map(prev).set(peerId, e.streams[0]));
    };
    return pc;
  }

  async function startCall() {
    setCallState("in-call");
    connectSignal();
  }

  async function endCall() {
    clearInterval(waitingPollRef.current!);
    clearTimeout(reconnectTimerRef.current!);
    reconnectAttempts.current = 8; // prevent reconnect after intentional leave
    if (recording && mediaRecorderRef.current) {
      mediaRecorderRef.current.stop();
    }
    pcsRef.current.forEach((pc) => pc.close());
    pcsRef.current.clear();
    wsRef.current?.close();
    localStream.current?.getTracks().forEach((t) => t.stop());
    screenStream.current?.getTracks().forEach((t) => t.stop());
    localStream.current = null;
    setLocalStreamState(null);
    setRemoteStreams(new Map());
    setRemoteMicStates(new Map());
    if (room) { try { await callsApi.leaveRoom(room.room_id); } catch { /* */ } }
    setRoom(null);
    setCallState("ended");
    setTimeout(() => setCallState("idle"), 2500);
  }

  function toggleMic() {
    const newMicOn = !micOn;
    localStream.current?.getAudioTracks().forEach((t) => { t.enabled = newMicOn; });
    setMicOn(newMicOn);
    // Broadcast new state to all connected peers
    pcsRef.current.forEach((_, peerId) => {
      wsRef.current?.send(JSON.stringify({ type: "media_state", to: peerId, micOn: newMicOn, camOn }));
    });
  }

  function toggleCam() {
    const newCamOn = !camOn;
    localStream.current?.getVideoTracks().forEach((t) => { t.enabled = newCamOn; });
    setCamOn(newCamOn);
    pcsRef.current.forEach((_, peerId) => {
      wsRef.current?.send(JSON.stringify({ type: "media_state", to: peerId, micOn, camOn: newCamOn }));
    });
  }

  async function toggleScreen() {
    if (screenSharing) {
      // Restore camera track in all peer connections
      const camTrack = localStream.current?.getVideoTracks()[0];
      await Promise.all(Array.from(pcsRef.current.values()).map((pc) => {
        const sender = pc.getSenders().find((s) => s.track?.kind === "video");
        return sender && camTrack ? sender.replaceTrack(camTrack) : Promise.resolve();
      }));
      screenStream.current?.getTracks().forEach((t) => t.stop());
      screenStream.current = null;
      setLocalStreamState(localStream.current);
      setScreenSharing(false);
    } else {
      try {
        const display = await navigator.mediaDevices.getDisplayMedia({ video: true, audio: false });
        screenStream.current = display;
        const screenTrack = display.getVideoTracks()[0];
        // Replace video track in all peer connections
        await Promise.all(Array.from(pcsRef.current.values()).map((pc) => {
          const sender = pc.getSenders().find((s) => s.track?.kind === "video");
          return sender ? sender.replaceTrack(screenTrack) : Promise.resolve();
        }));
        // Fix stale-closure: capture refs directly in onended, don't rely on component state
        screenTrack.onended = async () => {
          const camTrack = localStream.current?.getVideoTracks()[0];
          await Promise.all(Array.from(pcsRef.current.values()).map((pc) => {
            const sender = pc.getSenders().find((s) => s.track?.kind === "video");
            return sender && camTrack ? sender.replaceTrack(camTrack) : Promise.resolve();
          }));
          screenStream.current = null;
          setLocalStreamState(localStream.current);
          setScreenSharing(false);
        };
        // Show screen in local preview tile
        const previewStream = new MediaStream([
          screenTrack,
          ...(localStream.current?.getAudioTracks() ?? []),
        ]);
        setLocalStreamState(previewStream);
        setScreenSharing(true);
      } catch { /* denied or cancelled by user */ }
    }
  }

  function sendChat() {
    const text = chatInput.trim();
    if (!text || !wsRef.current) return;
    const ts = new Date().toISOString();
    wsRef.current.send(JSON.stringify({ type: "chat", from: myPeerId.current, display_name: myName, text, ts }));
    setChatMsgs((prev) => [...prev, { from: myPeerId.current, display_name: myName, text, ts, isMe: true }]);
    setChatInput("");
  }

  async function admitGuest(waitingId: string) {
    if (!room) return;
    const guest = waitingGuests.find((g) => g.id === waitingId);
    await callsApi.admitGuest(room.room_id, waitingId);
    // The guest's tile is keyed by its signalling id: name it from the waiting room, not "guest_xxxxxxxx"
    if (guest) setRemotePeerNames((prev) => withPeerName(prev, guestPeerId(waitingId), guest.name));
    setWaitingGuests((prev) => prev.filter((g) => g.id !== waitingId));
    setUnreadWaiting((n) => Math.max(0, n - 1));
  }

  async function rejectGuest(waitingId: string) {
    if (!room) return;
    await callsApi.rejectGuest(room.room_id, waitingId);
    setWaitingGuests((prev) => prev.filter((g) => g.id !== waitingId));
    setUnreadWaiting((n) => Math.max(0, n - 1));
  }

  async function kickPeer(peerId: string) {
    if (!room) return;
    wsRef.current?.send(JSON.stringify({ type: "kick", target: peerId }));
    await callsApi.kickParticipant(room.room_id, peerId);
    pcsRef.current.get(peerId)?.close();
    pcsRef.current.delete(peerId);
    setRemoteStreams((prev) => { const m = new Map(prev); m.delete(peerId); return m; });
  }

  // Invite search, debounced
  function handleInviteSearch(q: string) {
    setInviteQuery(q);
    if (inviteSearchTimer.current) clearTimeout(inviteSearchTimer.current);
    if (!q.trim()) { setInviteResults([]); return; }
    inviteSearchTimer.current = setTimeout(async () => {
      setInviteLoading(true);
      try {
        const res = await messagingApi.mentionUsers(q);
        setInviteResults(res.data ?? []);
      } catch { /* */ } finally { setInviteLoading(false); }
    }, 300);
  }

  function toggleInvitee(user: InviteUser) {
    if (callState === "lobby") {
      setLobbyInvitees((prev) =>
        prev.find((u) => u.id === user.id) ? prev.filter((u) => u.id !== user.id) : [...prev, user]
      );
    } else {
      setCallInvitees((prev) =>
        prev.find((u) => u.id === user.id) ? prev.filter((u) => u.id !== user.id) : [...prev, user]
      );
    }
  }

  function isSelected(userId: string) {
    return callState === "lobby"
      ? lobbyInvitees.some((u) => u.id === userId)
      : callInvitees.some((u) => u.id === userId);
  }

  async function sendCallInvites() {
    if (!room || callInvitees.length === 0) return;
    try {
      await callsApi.inviteUsers(room.room_id, callInvitees.map((u) => u.id));
      setCallInvitees([]);
      setShowInviteModal(false);
      setInviteQuery("");
      setInviteResults([]);
    } catch { /* */ }
  }

  function openInviteModal() {
    setInviteQuery("");
    setInviteResults([]);
    setShowInviteModal(true);
  }

  function closeInviteModal() {
    setShowInviteModal(false);
    setInviteQuery("");
    setInviteResults([]);
    if (callState !== "lobby") setCallInvitees([]);
  }

  // Recording
  async function startRecording() {
    if (!localStream.current) return;
    const canvas = document.createElement("canvas");
    canvas.width = 640; canvas.height = 360;
    const ctx = canvas.getContext("2d")!;
    const videoEls = document.querySelectorAll<HTMLVideoElement>("video");

    const drawInterval = setInterval(() => {
      ctx.fillStyle = "#111827";
      ctx.fillRect(0, 0, 640, 360);
      const count = videoEls.length;
      if (count === 0) return;
      const cols = count <= 1 ? 1 : 2;
      const rows = Math.ceil(count / cols);
      const w = 640 / cols, h = 360 / rows;
      videoEls.forEach((v, i) => {
        const x = (i % cols) * w, y = Math.floor(i / cols) * h;
        try { ctx.drawImage(v, x, y, w, h); } catch { /* */ }
      });
    }, 1000 / 15);

    const audioCtx = new AudioContext();
    const dest = audioCtx.createMediaStreamDestination();
    const allStreams = [localStream.current, ...Array.from(remoteStreams.values())];
    allStreams.forEach((s) => {
      if (s) { try { audioCtx.createMediaStreamSource(s).connect(dest); } catch { /* */ } }
    });

    const canvasStream = canvas.captureStream(15);
    const combined = new MediaStream([...canvasStream.getVideoTracks(), ...dest.stream.getAudioTracks()]);

    const mimeType = MediaRecorder.isTypeSupported("video/webm;codecs=vp9,opus")
      ? "video/webm;codecs=vp9,opus" : "video/webm";

    const mr = new MediaRecorder(combined, { mimeType, videoBitsPerSecond: 300_000, audioBitsPerSecond: 96_000 });
    recordingChunks.current = [];
    mr.ondataavailable = (e) => { if (e.data.size > 0) recordingChunks.current.push(e.data); };
    mr.onstop = async () => {
      clearInterval(drawInterval);
      audioCtx.close();
      if (room && recordingChunks.current.length > 0) {
        setUploading(true);
        const blob = new Blob(recordingChunks.current, { type: "video/webm" });
        try { await callsApi.uploadRecording(room.room_id, blob); setRecordingReady(true); } catch { /* */ }
        setUploading(false);
      }
    };
    mr.start(3000);
    mediaRecorderRef.current = mr;
    setRecording(true);
  }

  function stopRecording() {
    mediaRecorderRef.current?.stop();
    mediaRecorderRef.current = null;
    setRecording(false);
  }

  async function downloadRecording() {
    if (!room) return;
    const res = await callsApi.getRecording(room.room_id);
    window.open(res.data.download_url, "_blank");
  }

  function copyLink() {
    if (room) {
      navigator.clipboard.writeText(`${window.location.origin}/meet/${room.room_id}`);
      setLinkCopied(true);
      setTimeout(() => setLinkCopied(false), 2000);
    }
  }

  const remoteEntries = Array.from(remoteStreams.entries());
  const totalTiles = 1 + remoteEntries.length;
  // Phones: one column for a two-person call, two beyond; the grid scrolls instead of shrinking tiles.
  const gridCols = totalTiles <= 1 ? "grid-cols-1" : totalTiles <= 2 ? "grid-cols-1 md:grid-cols-2" : totalTiles <= 4 ? "grid-cols-2" : "grid-cols-2 md:grid-cols-3";

  // Invite dialog (shared for lobby + in-call)
  const invitees = callState === "lobby" ? lobbyInvitees : callInvitees;
  const InviteModal = showInviteModal ? (
    <Dialog open onOpenChange={(o) => { if (!o) closeInviteModal(); }}
      title={callState === "lobby" ? "Pre-invite team members" : "Invite to this call"}
      footer={callState === "lobby" ? (
        <>
          <p className="mr-auto text-xs text-fg-muted">
            {lobbyInvitees.length > 0
              ? `${lobbyInvitees.length} member${lobbyInvitees.length > 1 ? "s" : ""} will be notified when call starts`
              : "Selected members get notified when you join"}
          </p>
          <Button variant="primary" onClick={() => setShowInviteModal(false)}>Done</Button>
        </>
      ) : (
        <>
          <span className="mr-auto text-xs text-fg-muted tabular">{callInvitees.length} selected</span>
          <Button variant="primary" onClick={sendCallInvites} disabled={callInvitees.length === 0}>
            <Send aria-hidden /> Send invite
          </Button>
        </>
      )}>
      {invitees.length > 0 && (
        <div className="mb-3 flex flex-wrap gap-1.5">
          {invitees.map((u) => (
            <span key={u.id} className="flex items-center gap-1 rounded-full bg-brand/[0.07] py-0.5 pl-2.5 pr-1 text-xs text-fg">
              {u.name}
              <button type="button" onClick={() => toggleInvitee(u)} aria-label={`Remove ${u.name}`}
                className="rounded-full p-0.5 text-fg-muted hover:bg-surface-2 hover:text-fg">
                <X className="size-3" aria-hidden />
              </button>
            </span>
          ))}
        </div>
      )}

      <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
        autoFocus value={inviteQuery} onChange={(e) => handleInviteSearch(e.target.value)}
        placeholder="Search team members" aria-label="Search team members" className="mb-3" />

      <div className="max-h-64 space-y-0.5 overflow-y-auto">
        {inviteLoading && (
          <p className="py-4 text-center text-xs text-fg-muted" role="status">Searching</p>
        )}
        {!inviteLoading && inviteQuery && inviteResults.length === 0 && (
          <p className="py-4 text-center text-xs text-fg-muted">No team members found</p>
        )}
        {!inviteLoading && inviteResults.map((user) => {
          const selected = isSelected(user.id);
          return (
            <button key={user.id} type="button" onClick={() => toggleInvitee(user)} aria-pressed={selected}
              className={cn("flex min-h-11 w-full items-center gap-3 rounded-lg px-3 py-2 text-left transition-colors",
                selected ? "bg-brand/[0.07]" : "hover:bg-surface-2")}>
              <Avatar name={user.name} size="sm" />
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-medium text-fg">{user.name}</p>
                {user.position && <p className="truncate text-xs text-fg-muted">{user.position}</p>}
              </div>
              {selected && <Check className="size-4 shrink-0 text-brand-ink" aria-hidden />}
            </button>
          );
        })}
      </div>
    </Dialog>
  ) : null;

  const shareUrl = typeof window !== "undefined" ? `${window.location.origin}/meet/${room?.room_id ?? ""}` : `/meet/${room?.room_id ?? ""}`;

  return (
    <FullHeightPage className="flex-col bg-canvas">
      {InviteModal}

      {/* Idle */}
      {(callState === "idle" || callState === "ended") && (
        <div className="flex flex-1 items-center justify-center px-4">
          <div className="w-full max-w-md rounded-lg border border-line bg-surface p-6 shadow-card">
            <div className="mb-6 text-center">
              {callState === "ended" ? (
                <>
                  <span className="mx-auto mb-3 flex size-12 items-center justify-center rounded-lg bg-surface-2">
                    <PhoneOff className="size-6 text-fg-muted" aria-hidden />
                  </span>
                  <h1 className="text-lg font-semibold text-fg">Call ended</h1>
                </>
              ) : (
                <>
                  <span className="mx-auto mb-3 flex size-12 items-center justify-center rounded-lg bg-brand/[0.07]">
                    <Video className="size-6 text-brand-ink" aria-hidden />
                  </span>
                  <h1 className="text-lg font-semibold text-fg">Video calls</h1>
                  <p className="mt-0.5 text-[13px] text-fg-muted">Secure, self-hosted WebRTC conferencing</p>
                </>
              )}
            </div>
            <div className="space-y-3">
              <Button variant="primary" size="lg" className="w-full" onClick={createRoom}>
                <Plus aria-hidden /> Start new call
              </Button>
              <div className="flex items-center gap-3">
                <div className="h-px flex-1 bg-line" />
                <span className="text-xs text-fg-muted">or join</span>
                <div className="h-px flex-1 bg-line" />
              </div>
              <div className="flex gap-2">
                <div className="flex-1">
                  <Input value={joinRoomId} onChange={(e) => setJoinRoomId(e.target.value)} autoComplete="off"
                    onKeyDown={(e) => { if (e.key === "Enter") joinRoom(); }}
                    placeholder="Enter room ID" aria-label="Room ID" className="font-mono" />
                </div>
                <Button onClick={() => joinRoom()}>Join</Button>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Creating */}
      {callState === "creating" && (
        <div className="flex flex-1 items-center justify-center">
          <div className="text-center" role="status">
            <div className="mb-4 flex justify-center gap-2" aria-hidden>
              {[0, 1, 2].map((i) => <div key={i} className="size-2.5 animate-bounce rounded-full bg-brand" style={{ animationDelay: `${i * 150}ms` }} />)}
            </div>
            <p className="text-sm text-fg-muted">Setting up your room</p>
          </div>
        </div>
      )}

      {/* Lobby */}
      {callState === "lobby" && room && (
        <div className="flex flex-1 flex-col items-center justify-center overflow-y-auto px-4 py-6">
          <div className="w-full max-w-lg">
            <div className="mb-5 text-center">
              <h1 className="text-lg font-semibold text-fg">Ready to join?</h1>
              <p className="mt-0.5 text-[13px] text-fg-muted">Check your camera and mic before entering</p>
            </div>
            <div className="mb-4">
              <VideoTile name={myName} stream={localStreamState} muted isMe />
            </div>
            <div className="mb-4 flex justify-center gap-2">
              <Button onClick={toggleMic} aria-pressed={micOn}
                className={micOn ? undefined : "border-tone-red/40 bg-tone-red-soft text-tone-red hover:bg-tone-red-soft"}>
                {micOn ? <Mic aria-hidden /> : <MicOff aria-hidden />}
                {micOn ? "Mic on" : "Mic off"}
              </Button>
              <Button onClick={toggleCam} aria-pressed={camOn}
                className={camOn ? undefined : "border-tone-red/40 bg-tone-red-soft text-tone-red hover:bg-tone-red-soft"}>
                {camOn ? <Video aria-hidden /> : <VideoOff aria-hidden />}
                {camOn ? "Camera on" : "Camera off"}
              </Button>
            </div>

            {/* Invite team members */}
            <div className="mb-3 rounded-lg border border-line bg-surface px-4 py-3">
              <div className="mb-2 flex items-center justify-between">
                <p className="text-[13px] font-medium text-fg">Invite team members</p>
                <Button size="sm" variant="ghost" onClick={openInviteModal}>
                  <UserPlus aria-hidden /> Add
                </Button>
              </div>
              {lobbyInvitees.length > 0 ? (
                <div className="flex flex-wrap gap-1.5">
                  {lobbyInvitees.map((u) => (
                    <span key={u.id} className="flex items-center gap-1 rounded-full bg-brand/[0.07] py-0.5 pl-2.5 pr-1 text-xs text-fg">
                      {u.name}
                      <button type="button" onClick={() => setLobbyInvitees((prev) => prev.filter((x) => x.id !== u.id))}
                        aria-label={`Remove ${u.name}`} className="rounded-full p-0.5 text-fg-muted hover:bg-surface-2 hover:text-fg">
                        <X className="size-3" aria-hidden />
                      </button>
                    </span>
                  ))}
                </div>
              ) : (
                <p className="text-xs text-fg-muted">They&apos;ll be notified and auto-admitted when the call starts</p>
              )}
            </div>

            {/* Share link */}
            <div className="mb-5 space-y-2 rounded-lg border border-line bg-surface px-4 py-3">
              <p className="text-[13px] font-medium text-fg">Share this link with external guests</p>
              <div className="flex items-center gap-2">
                <span className="flex-1 truncate font-mono text-xs text-fg-muted">{shareUrl}</span>
                <Button size="sm" variant="ghost" onClick={copyLink} className="shrink-0">
                  {linkCopied ? <Check aria-hidden /> : <Link2 aria-hidden />}
                  {linkCopied ? "Copied" : "Copy"}
                </Button>
              </div>
              <p className="flex items-center text-xs text-fg-muted">Room ID: <span className="ml-1 font-mono">{room.room_id}</span>
                <IconButton size="sm" icon={copied ? Check : Copy} label={copied ? "Copied" : "Copy room ID"} className="ml-1"
                  onClick={() => { navigator.clipboard.writeText(room.room_id); setCopied(true); setTimeout(() => setCopied(false), 2000); }} />
              </p>
            </div>

            <Button variant="primary" size="lg" className="w-full" onClick={startCall}>Join call</Button>
          </div>
        </div>
      )}

      {/* In call */}
      {callState === "in-call" && room && (
        <div className="flex flex-1 overflow-hidden">

          {/* Waiting room */}
          {showWaiting && (
            <aside aria-label="Waiting room" className="fixed inset-0 z-40 flex w-full flex-col bg-surface pb-[env(safe-area-inset-bottom)] md:static md:z-auto md:w-72 md:shrink-0 md:border-r md:border-line md:pb-0">
              <div className="flex items-center justify-between border-b border-line px-4 py-2.5">
                <h2 className="flex items-center gap-2 text-sm font-semibold text-fg">
                  <Bell className="size-4 text-fg-muted" aria-hidden /> Waiting room
                  <CountBadge count={waitingGuests.length} />
                </h2>
                <IconButton icon={ChevronRight} label="Close waiting room" onClick={() => setShowWaiting(false)} />
              </div>
              <div className="flex-1 space-y-2 overflow-y-auto p-3">
                {waitingGuests.length === 0 ? (
                  <EmptyState compact title="No one waiting" />
                ) : waitingGuests.map((g) => (
                  <div key={g.id} className="rounded-lg border border-line bg-surface p-3">
                    <div className="mb-2 flex items-start gap-2">
                      <Avatar name={g.name} size="sm" />
                      <div className="min-w-0 flex-1">
                        <p className="truncate text-sm font-medium text-fg">{g.name}</p>
                        {g.crm_contact_name && <p className="text-xs text-tone-blue">CRM: {g.crm_contact_name}</p>}
                        {g.is_erp_user && <p className="text-xs text-tone-violet">Internal user</p>}
                        {g.email && !g.crm_contact_name && !g.is_erp_user && (
                          <p className="truncate text-xs text-fg-muted">{g.email}</p>
                        )}
                      </div>
                    </div>
                    <div className="flex gap-2">
                      <Button size="sm" variant="primary" className="flex-1" onClick={() => admitGuest(g.id)}>Admit</Button>
                      <Button size="sm" variant="danger" className="flex-1" onClick={() => rejectGuest(g.id)}>Reject</Button>
                    </div>
                  </div>
                ))}
              </div>
            </aside>
          )}

          {/* Main call area */}
          <div className="flex flex-1 flex-col overflow-hidden">
            {/* Top bar */}
            <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-b border-line bg-surface px-3 py-2.5 md:px-5">
              <div className="flex items-center gap-2">
                <Pill tone={connected ? "blue" : "orange"} icon={connected ? Wifi : WifiOff} role="status">
                  {connected ? "Connected" : "Connecting"}
                </Pill>
                <Pill icon={Users}><span className="tabular">{1 + remoteEntries.length}</span></Pill>
                {recording && <Pill tone="red" icon={Circle}>Recording</Pill>}
                {uploading && <Pill tone="blue">Uploading</Pill>}
              </div>
              <div className="flex items-center gap-2">
                {(recordingReady || room.has_recording) && !recording && !uploading && (
                  <Button size="sm" onClick={downloadRecording}>
                    <Download aria-hidden /> Recording
                  </Button>
                )}
                <Button size="sm" onClick={copyLink}>
                  {linkCopied ? <Check aria-hidden /> : <Link2 aria-hidden />}
                  {linkCopied ? "Copied" : "Share link"}
                </Button>
                <span className="flex items-center gap-1.5 text-xs font-medium text-fg-muted">
                  <span className="size-1.5 animate-pulse rounded-full bg-brand" aria-hidden /> Live
                </span>
              </div>
            </div>

            {/* Video grid */}
            <div className={cn("grid flex-1 content-start gap-2 overflow-y-auto px-3 py-3 md:content-stretch md:gap-3 md:overflow-hidden md:px-4", gridCols)}>
              <VideoTile name={myName} stream={localStreamState} muted micMuted={!micOn} isMe isHost={isHost} />
              {remoteEntries.map(([peerId, stream]) => (
                <VideoTile key={peerId}
                  name={remotePeerNames.get(peerId) ?? peerId}
                  stream={stream}
                  micMuted={remoteMicStates.get(peerId) === false}
                  isGuest={peerId.startsWith("guest_")}
                  isHost={isHost}
                  onKick={() => kickPeer(peerId)} />
              ))}
            </div>

            {/* Control bar */}
            <div className="flex shrink-0 justify-center px-2 pb-2 md:px-0 md:pb-5">
              <div className="flex flex-wrap items-center justify-center gap-1.5 rounded-lg border border-line bg-surface-raised px-2 py-2 shadow-pop md:gap-2 md:px-4 md:py-2.5" role="toolbar" aria-label="Call controls">
                <ControlBtn active={micOn} tone={micOn ? "normal" : "warn"} onClick={toggleMic} icon={Mic} offIcon={MicOff} label={micOn ? "Mute mic" : "Unmute mic"} />
                <ControlBtn active={camOn} tone={camOn ? "normal" : "warn"} onClick={toggleCam} icon={Video} offIcon={VideoOff} label={camOn ? "Stop camera" : "Start camera"} />
                <ControlBtn active={screenSharing} tone={screenSharing ? "on" : "normal"} onClick={toggleScreen} icon={Monitor} label={screenSharing ? "Stop sharing" : "Share screen"} />
                <ControlBtn active={!recording} tone={recording ? "warn" : "normal"} onClick={recording ? stopRecording : startRecording}
                  icon={Circle} label={recording ? "Stop recording" : "Start recording"} />
                <ControlBtn active={false} onClick={openInviteModal} icon={UserPlus} label="Invite team member" />
                {isHost && (
                  <ControlBtn active={showWaiting} tone={showWaiting ? "on" : "normal"} onClick={() => { setShowWaiting((v) => !v); setUnreadWaiting(0); }}
                    icon={Bell} label="Waiting room" badge={showWaiting ? 0 : unreadWaiting} />
                )}
                <ControlBtn active={showChat} tone={showChat ? "on" : "normal"} onClick={() => { setShowChat((v) => !v); setUnreadChat(0); }}
                  icon={MessageSquare} label="Chat" badge={showChat ? 0 : unreadChat} />
                <div className="mx-1 hidden h-8 w-px bg-line md:block" aria-hidden />
                <button type="button" onClick={endCall} aria-label="End call" title="End call"
                  className="flex size-11 items-center justify-center rounded-lg bg-brand text-brand-on transition-colors hover:bg-brand-hover focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[rgb(var(--focus)/0.7)] md:size-12">
                  <PhoneOff className="size-5" aria-hidden />
                </button>
              </div>
            </div>
          </div>

          {/* Chat */}
          {showChat && (
            <aside aria-label="Call chat" className="fixed inset-0 z-40 flex w-full flex-col bg-surface pb-[env(safe-area-inset-bottom)] md:static md:z-auto md:w-72 md:shrink-0 md:border-l md:border-line md:pb-0">
              <div className="flex items-center justify-between border-b border-line px-4 py-2.5">
                <h2 className="text-sm font-semibold text-fg">Chat</h2>
                <IconButton icon={X} label="Close chat" onClick={() => setShowChat(false)} />
              </div>
              <div className="flex-1 space-y-2 overflow-y-auto p-3">
                {chatMsgs.map((m, i) => (
                  <div key={i} className={cn("flex flex-col", m.isMe ? "items-end" : "items-start")}>
                    <span className="mb-0.5 text-xs text-fg-muted">{m.isMe ? "You" : (m.display_name ?? m.from)}</span>
                    <div className={cn("max-w-[90%] rounded-lg border px-3 py-2 text-sm text-fg",
                      m.isMe ? "border-brand/20 bg-brand/[0.07]" : "border-line bg-surface-2")}>
                      {m.text}
                    </div>
                  </div>
                ))}
              </div>
              <div className="flex gap-2 border-t border-line p-3">
                <div className="flex-1">
                  <Input value={chatInput} onChange={(e) => setChatInput(e.target.value)} autoComplete="off"
                    onKeyDown={(e) => { if (e.key === "Enter") sendChat(); }}
                    placeholder="Type a message" aria-label="Chat message" />
                </div>
                <IconButton icon={Send} label="Send" variant="primary" onClick={sendChat} disabled={!chatInput.trim()} />
              </div>
            </aside>
          )}
        </div>
      )}
    </FullHeightPage>
  );
}
