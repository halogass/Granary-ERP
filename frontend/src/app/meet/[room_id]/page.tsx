"use client";

import { useEffect, useRef, useState, useCallback } from "react";
import { useParams } from "next/navigation";
import { guestCallsApi } from "@/lib/api";
import { withPeerName } from "@/lib/callPeers";
import {
  Video, VideoOff, Mic, MicOff, PhoneOff, Monitor, Users,
  Clock, CheckCircle, XCircle, AlertCircle, Send, MessageSquare, X,
} from "lucide-react";
import Avatar from "@/components/common/Avatar";
import { PortalBrand } from "@/components/portal/PortalBrand";
import { Button, Field, IconButton, Input, Pill } from "@/components/ui";
import { cn } from "@/lib/utils";
import { useUrlParam } from "@/hooks/useUrlParam";
import { signalWsUrl } from "@/lib/publicUrls";

interface IceServer { urls: string; username?: string; credential?: string }

type GuestState =
  | "loading"        // checking room
  | "room_not_found"
  | "room_ended"
  | "form"           // name/email entry
  | "waiting"        // in waiting room, polling
  | "rejected"       // host rejected
  | "lobby"          // admitted, previewing cam
  | "in-call"
  | "ended";

interface ChatMsg { from: string; name: string; text: string; ts: string; isMe: boolean }

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const dbg = process.env.NODE_ENV === "development" ? (...a: any[]) => console.log("[MEET]", ...a) : () => {};

function VideoTile({ name, stream, muted = false, isMe = false }: {
  name: string; stream?: MediaStream | null; muted?: boolean; isMe?: boolean;
}) {
  const ref = useRef<HTMLVideoElement>(null);
  useEffect(() => {
    if (ref.current && stream) ref.current.srcObject = stream;
  }, [stream]);
  return (
    <div className={cn("relative aspect-video overflow-hidden rounded-lg border bg-surface-2", isMe ? "border-brand/40" : "border-line")}>
      {stream
        ? <video ref={ref} autoPlay playsInline muted={muted} className="size-full bg-black object-cover" />
        : <div className="flex size-full items-center justify-center">
            <Avatar name={name} size="xl" />
          </div>
      }
      <div className="absolute inset-x-0 bottom-0 bg-gradient-to-t from-black/70 to-transparent px-3 py-2">
        <span className="text-xs font-medium text-white">{name}{isMe ? " (you)" : ""}</span>
        {muted && <MicOff className="ml-1 inline size-3 text-white" aria-label="Muted" />}
      </div>
    </div>
  );
}

const ctrl = (tone: "normal" | "on" | "warn") => cn(
  "relative flex size-11 items-center justify-center rounded-lg transition-colors md:size-12",
  "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[rgb(var(--focus)/0.7)]",
  tone === "warn" ? "bg-tone-red-soft text-tone-red hover:opacity-90"
  : tone === "on" ? "bg-brand/[0.1] text-brand-ink hover:bg-brand/[0.15]"
  : "bg-surface-2 text-fg hover:bg-line-soft");

function StatusScreen({ icon: Icon, tone = "neutral", title, text, children }: {
  icon: React.ElementType; tone?: "neutral" | "red" | "orange" | "brand"; title: string; text?: string; children?: React.ReactNode;
}) {
  const box = { neutral: "bg-surface-2 text-fg-muted", red: "bg-tone-red-soft text-tone-red", orange: "bg-tone-orange-soft text-tone-orange", brand: "bg-brand/[0.07] text-brand-ink" }[tone];
  return (
    <div className="flex flex-1 items-center justify-center px-4">
      <div className="text-center">
        <span className={cn("mx-auto mb-4 flex size-14 items-center justify-center rounded-lg", box)}>
          <Icon className="size-7" aria-hidden />
        </span>
        <h1 className="text-lg font-semibold text-fg">{title}</h1>
        {text && <p className="mt-1 text-sm text-fg-muted">{text}</p>}
        {children}
      </div>
    </div>
  );
}

export default function GuestMeetPage() {
  const { room_id: roomId } = useParams<{ room_id: string }>();
  const [state, setState] = useState<GuestState>("loading");
  const [roomName, setRoomName] = useState("");
  const [guestName, setGuestName] = useState("");
  // The signalling handler is memoised before the name is typed: read the name through a ref
  const guestNameRef = useRef("");
  guestNameRef.current = guestName;
  const [guestEmail, setGuestEmail] = useState("");
  const [waitingId, setWaitingId] = useState("");
  const [waitSeconds, setWaitSeconds] = useState(0);
  const iceServersRef = useRef<IceServer[]>([]);
  const signalUrl = signalWsUrl();

  const [micOn, setMicOn] = useState(true);
  const [camOn, setCamOn] = useState(true);
  const [screenSharing, setScreenSharing] = useState(false);
  const [showChat, setShowChat] = useState(false);
  // The chat covers the call on phones; it is in the URL so the phone Back button closes it
  // instead of leaving the call.
  const [chatParam, setChatParam] = useUrlParam("chat");
  useEffect(() => { setChatParam(showChat ? "1" : null); }, [showChat, setChatParam]);
  useEffect(() => { if (!chatParam) setShowChat(false); }, [chatParam]);
  const [chatInput, setChatInput] = useState("");
  const [chatMsgs, setChatMsgs] = useState<ChatMsg[]>([]);
  const [unreadChat, setUnreadChat] = useState(0);
  const [notice, setNotice] = useState("");

  const localStream = useRef<MediaStream | null>(null);
  const screenStream = useRef<MediaStream | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const pcsRef = useRef<Map<string, RTCPeerConnection>>(new Map());
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const waitTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const [localStreamState, setLocalStreamState] = useState<MediaStream | null>(null);
  const [remoteStreams, setRemoteStreams] = useState<Map<string, MediaStream>>(new Map());
  const [remotePeerNames, setRemotePeerNames] = useState<Map<string, string>>(new Map());
  // Peer id + signed signal ticket are issued by the backend once admitted.
  const peerId = useRef("");
  const signalTicket = useRef("");

  // Check room on mount
  useEffect(() => {
    guestCallsApi.getRoomPublic(roomId).then((data) => {
      if (data.detail) { setState("room_not_found"); return; }
      if (!data.is_active) { setState("room_ended"); return; }
      setRoomName(data.name);
      setState("form");
    }).catch(() => setState("room_not_found"));
  }, [roomId]);

  async function enterWaiting() {
    if (!guestName.trim()) return;
    const res = await guestCallsApi.enterWaiting(roomId, {
      name: guestName.trim(),
      email: guestEmail.trim() || undefined,
    });
    if (res.detail) return;
    setWaitingId(res.waiting_id);
    setState("waiting");
    setWaitSeconds(0);
    waitTimerRef.current = setInterval(() => setWaitSeconds((s) => s + 1), 1000);
  }

  // Poll status while waiting
  useEffect(() => {
    if (state !== "waiting" || !waitingId) return;
    pollRef.current = setInterval(async () => {
      const res = await guestCallsApi.pollStatus(roomId, waitingId);
      if (res.status === "admitted") {
        clearInterval(pollRef.current!);
        clearInterval(waitTimerRef.current!);
        iceServersRef.current = res.iceServers;
        dbg("admitted | ICE servers:", res.iceServers, "| signal URL:", signalUrl);
        peerId.current = res.peer_id;
        signalTicket.current = res.signal_ticket;
        await startMedia();
        setState("lobby");
      } else if (res.status === "rejected") {
        clearInterval(pollRef.current!);
        clearInterval(waitTimerRef.current!);
        setState("rejected");
      } else if (res.status === "replaced") {
        // The same email asked to join again (another tab or device); that request is the live one.
        clearInterval(pollRef.current!);
        clearInterval(waitTimerRef.current!);
        setState("form");
      }
    }, 2000);
    return () => clearInterval(pollRef.current!);
  }, [state, waitingId, roomId]); // eslint-disable-line

  async function startMedia() {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ video: true, audio: true });
      localStream.current = stream;
      setLocalStreamState(stream);
    } catch { /* no cam */ }
  }

  const connectSignal = useCallback(() => {
    dbg("[signal] connecting to", signalUrl, "| room:", roomId, "| peer:", peerId.current);
    const ws = new WebSocket(`${signalUrl}?room=${roomId}&user=${encodeURIComponent(peerId.current)}&ticket=${encodeURIComponent(signalTicket.current)}`);
    wsRef.current = ws;
    ws.onopen = () => {
      dbg("[signal] connected → registering");
      ws.send(JSON.stringify({ type: "register", roomId: roomId, userId: peerId.current }));
    };
    ws.onclose = (e) => dbg("[signal] closed", e.code, e.reason);
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
          ws.send(JSON.stringify({ type: "answer", to: msg.from, sdp: answer, display_name: guestNameRef.current }));
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
            ws.send(JSON.stringify({ type: "offer", to: existingPeer, sdp: offer, display_name: guestNameRef.current }));
          }
        } else if (msg.type === "peer_joined") {
          dbg("[signal] peer_joined:", msg.userId, ", waiting for their offer");
        } else if (msg.type === "peer_left") {
          pcsRef.current.get(msg.userId)?.close();
          pcsRef.current.delete(msg.userId);
          setRemoteStreams((prev) => { const m = new Map(prev); m.delete(msg.userId); return m; });
        } else if (msg.type === "chat") {
          const newMsg: ChatMsg = { from: msg.from, name: msg.display_name || msg.from, text: msg.text, ts: msg.ts, isMe: msg.from === peerId.current };
          setChatMsgs((prev) => [...prev, newMsg]);
          setUnreadChat((n) => showChat ? 0 : n + 1);
        } else if (msg.type === "kick" && msg.target === peerId.current) {
          endCall();
        }
      } catch (err) { dbg("[signal] message handler error", err); }
    };
  }, [roomId, showChat]); // eslint-disable-line

  function createPeerConnection(pid: string): RTCPeerConnection {
    dbg("creating PC for", pid, "| ICE servers:", iceServersRef.current.length, iceServersRef.current);
    const pc = new RTCPeerConnection({ iceServers: iceServersRef.current });
    pcsRef.current.set(pid, pc);
    if (localStream.current) {
      localStream.current.getTracks().forEach((t) => {
        dbg("adding", t.kind, "track for", pid);
        pc.addTrack(t, localStream.current!);
      });
    } else {
      dbg("⚠ no local stream when creating PC for", pid);
    }
    pc.onicecandidate = (e) => {
      if (e.candidate) dbg(`[pc:${pid}] ICE candidate:`, e.candidate.type, e.candidate.address);
      if (e.candidate && wsRef.current)
        wsRef.current.send(JSON.stringify({ type: "ice", to: pid, candidate: e.candidate }));
    };
    pc.oniceconnectionstatechange = () => dbg(`[pc:${pid}] ICE connection:`, pc.iceConnectionState);
    pc.onicegatheringstatechange = () => dbg(`[pc:${pid}] ICE gathering:`, pc.iceGatheringState);
    pc.onconnectionstatechange = () => dbg(`[pc:${pid}] connection:`, pc.connectionState);
    pc.onsignalingstatechange = () => dbg(`[pc:${pid}] signaling:`, pc.signalingState);
    pc.ontrack = (e) => {
      dbg(`[pc:${pid}] track received:`, e.track.kind);
      setRemoteStreams((prev) => new Map(prev).set(pid, e.streams[0]));
    };
    return pc;
  }

  async function startCall() {
    setState("in-call");
    connectSignal();
  }

  function endCall() {
    pcsRef.current.forEach((pc) => pc.close());
    pcsRef.current.clear();
    wsRef.current?.close();
    localStream.current?.getTracks().forEach((t) => t.stop());
    screenStream.current?.getTracks().forEach((t) => t.stop());
    localStream.current = null;
    setLocalStreamState(null);
    setRemoteStreams(new Map());
    setState("ended");
  }

  function toggleMic() {
    localStream.current?.getAudioTracks().forEach((t) => { t.enabled = !micOn; });
    setMicOn((v) => !v);
  }

  function toggleCam() {
    localStream.current?.getVideoTracks().forEach((t) => { t.enabled = !camOn; });
    setCamOn((v) => !v);
  }

  async function toggleScreen() {
    if (screenSharing) {
      screenStream.current?.getTracks().forEach((t) => t.stop());
      screenStream.current = null;
      // Replace screen track with camera track in all peers
      if (localStream.current) {
        const camTrack = localStream.current.getVideoTracks()[0];
        pcsRef.current.forEach((pc) => {
          const sender = pc.getSenders().find((s) => s.track?.kind === "video");
          if (sender && camTrack) sender.replaceTrack(camTrack);
        });
      }
      setScreenSharing(false);
    } else {
      // Screen capture is unsupported on most tablets/phones (e.g. iOS Safari has no
      // getDisplayMedia at all); tell the user instead of failing silently.
      if (!navigator.mediaDevices?.getDisplayMedia) {
        showNotice("Screen sharing isn't supported on this device or browser. Use a desktop browser (Chrome/Edge/Firefox).");
        return;
      }
      try {
        const display = await navigator.mediaDevices.getDisplayMedia({ video: true, audio: false });
        screenStream.current = display;
        const screenTrack = display.getVideoTracks()[0];
        pcsRef.current.forEach((pc) => {
          const sender = pc.getSenders().find((s) => s.track?.kind === "video");
          if (sender) sender.replaceTrack(screenTrack);
        });
        screenTrack.onended = () => toggleScreen();
        setScreenSharing(true);
      } catch {
        showNotice("Screen share was cancelled or blocked.");
      }
    }
  }

  const noticeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  function showNotice(msg: string) {
    setNotice(msg);
    if (noticeTimer.current) clearTimeout(noticeTimer.current);
    noticeTimer.current = setTimeout(() => setNotice(""), 4500);
  }

  function sendChat() {
    const text = chatInput.trim();
    if (!text || !wsRef.current) return;
    const ts = new Date().toISOString();
    wsRef.current.send(JSON.stringify({ type: "chat", from: peerId.current, display_name: guestName, text, ts }));
    setChatMsgs((prev) => [...prev, { from: peerId.current, name: guestName, text, ts, isMe: true }]);
    setChatInput("");
  }

  const remoteEntries = Array.from(remoteStreams.entries());
  const totalTiles = 1 + remoteEntries.length;
  const gridCols = totalTiles <= 1 ? "grid-cols-1" : totalTiles <= 2 ? "grid-cols-1 sm:grid-cols-2" : "grid-cols-2";

  // Screens

  const formatWait = (s: number) => `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;

  return (
    <div className={cn("flex flex-col bg-canvas text-fg", state === "in-call" ? "h-[100dvh] overflow-hidden" : "min-h-[100dvh]")}>
      {/* Brand bar */}
      <header className="flex shrink-0 items-center gap-3 border-b border-line bg-surface px-4 py-3 md:px-5">
        {/* The company's own logo and name (Settings > Branding), as on the other public pages. */}
        <PortalBrand subtitle="" className="shrink-0" />
        {roomName && <span className="truncate text-sm text-fg-muted">· {roomName}</span>}
      </header>

      {/* Transient notice (e.g. screen share unsupported) */}
      {notice && (
        <div role="status" className="fixed left-1/2 top-16 z-50 max-w-[90vw] -translate-x-1/2 rounded-lg border border-line bg-surface-raised px-4 py-2.5 text-center text-sm text-fg shadow-pop">
          {notice}
        </div>
      )}

      {state === "loading" && (
        <div className="flex flex-1 items-center justify-center" role="status" aria-label="Loading">
          <div className="flex gap-2" aria-hidden>
            {[0, 1, 2].map((i) => <div key={i} className="size-2.5 animate-bounce rounded-full bg-brand" style={{ animationDelay: `${i * 150}ms` }} />)}
          </div>
        </div>
      )}

      {(state === "room_not_found" || state === "room_ended") && (
        <StatusScreen icon={AlertCircle} tone="red"
          title={state === "room_ended" ? "This call has ended" : "Room not found"}
          text={state === "room_ended" ? "The host has ended this call." : "The link may be invalid or expired."} />
      )}

      {state === "form" && (
        <div className="flex flex-1 items-center justify-center px-4">
          <div className="w-full max-w-sm rounded-lg border border-line bg-surface p-6 shadow-card">
            <div className="mb-6 text-center">
              <span className="mx-auto mb-3 flex size-12 items-center justify-center rounded-lg bg-brand/[0.07]">
                <Video className="size-6 text-brand-ink" aria-hidden />
              </span>
              <h1 className="text-lg font-semibold text-fg">{roomName}</h1>
              <p className="mt-0.5 text-[13px] text-fg-muted">Enter your details to request to join</p>
            </div>
            <div className="space-y-4">
              <Field label="Your name" required>
                <Input value={guestName} onChange={(e) => setGuestName(e.target.value)} autoComplete="name"
                  onKeyDown={(e) => { if (e.key === "Enter") enterWaiting(); }}
                  placeholder="e.g. John Smith" />
              </Field>
              <Field label="Email" hint="Optional.">
                <Input type="email" value={guestEmail} onChange={(e) => setGuestEmail(e.target.value)} autoComplete="email"
                  onKeyDown={(e) => { if (e.key === "Enter") enterWaiting(); }}
                  placeholder="you@company.com" />
              </Field>
              <Button variant="primary" size="lg" className="w-full" onClick={enterWaiting} disabled={!guestName.trim()}>
                Request to join
              </Button>
            </div>
          </div>
        </div>
      )}

      {state === "waiting" && (
        <StatusScreen icon={Clock} tone="orange" title="Waiting for host"
          text="The host will admit you shortly. Please keep this tab open.">
          <div className="mt-4 text-3xl font-semibold text-fg-muted tabular" aria-label="Time waiting">{formatWait(waitSeconds)}</div>
        </StatusScreen>
      )}

      {state === "rejected" && (
        <StatusScreen icon={XCircle} tone="red" title="Request declined" text="The host did not admit you to this call.">
          <Button className="mt-5" onClick={() => setState("form")}>Try again</Button>
        </StatusScreen>
      )}

      {state === "lobby" && (
        <div className="flex flex-1 flex-col items-center justify-center px-4 py-6">
          <div className="w-full max-w-md">
            <div className="mb-5 text-center">
              <div className="mb-1 flex items-center justify-center gap-2">
                <CheckCircle className="size-5 text-tone-blue" aria-hidden />
                <h1 className="text-base font-semibold text-fg">You&apos;ve been admitted</h1>
              </div>
              <p className="text-[13px] text-fg-muted">Check your camera and mic before joining</p>
            </div>
            <div className="mb-4">
              <VideoTile name={guestName} stream={localStreamState} muted isMe />
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
            <Button variant="primary" size="lg" className="w-full" onClick={startCall}>Join call</Button>
          </div>
        </div>
      )}

      {state === "in-call" && (
        <div className="flex flex-1 overflow-hidden">
          <div className="flex flex-1 flex-col overflow-hidden">
            {/* Top bar */}
            <div className="flex shrink-0 items-center justify-between px-4 py-3 md:px-5">
              <Pill icon={Users}><span className="tabular">{1 + remoteEntries.length}</span> in call</Pill>
              <span className="flex items-center gap-1.5 text-xs font-medium text-fg-muted">
                <span className="size-1.5 animate-pulse rounded-full bg-brand" aria-hidden /> Live
              </span>
            </div>

            {/* Video grid */}
            <div className={cn("grid flex-1 content-start gap-2 overflow-y-auto px-3 py-2 sm:content-stretch sm:gap-3 sm:overflow-hidden sm:px-4", gridCols)}>
              <VideoTile name={guestName} stream={localStreamState} muted isMe />
              {remoteEntries.map(([pid, stream]) => (
                <VideoTile key={pid} name={remotePeerNames.get(pid) ?? pid} stream={stream} />
              ))}
            </div>

            {/* Controls */}
            <div className="flex shrink-0 justify-center px-2 pb-[calc(1rem+env(safe-area-inset-bottom))] sm:pb-6">
              <div className="flex items-center gap-2 rounded-lg border border-line bg-surface-raised px-3 py-2.5 shadow-pop sm:gap-3 sm:px-4" role="toolbar" aria-label="Call controls">
                <button type="button" onClick={toggleMic} title={micOn ? "Mute" : "Unmute"} aria-label={micOn ? "Mute" : "Unmute"}
                  className={ctrl(micOn ? "normal" : "warn")}>
                  {micOn ? <Mic className="size-5" aria-hidden /> : <MicOff className="size-5" aria-hidden />}
                </button>
                <button type="button" onClick={toggleCam} title={camOn ? "Stop camera" : "Start camera"} aria-label={camOn ? "Stop camera" : "Start camera"}
                  className={ctrl(camOn ? "normal" : "warn")}>
                  {camOn ? <Video className="size-5" aria-hidden /> : <VideoOff className="size-5" aria-hidden />}
                </button>
                <button type="button" onClick={toggleScreen} title={screenSharing ? "Stop sharing" : "Share screen"} aria-label={screenSharing ? "Stop sharing" : "Share screen"}
                  className={ctrl(screenSharing ? "on" : "normal")}>
                  <Monitor className="size-5" aria-hidden />
                </button>
                <button type="button" onClick={() => { setShowChat((v) => !v); setUnreadChat(0); }} title="Chat" aria-label="Chat"
                  aria-pressed={showChat} className={ctrl(showChat ? "on" : "normal")}>
                  <MessageSquare className="size-5" aria-hidden />
                  {unreadChat > 0 && (
                    <span className="absolute -right-1 -top-1 flex size-5 items-center justify-center rounded-full bg-brand text-xs font-semibold text-brand-on tabular">{unreadChat}</span>
                  )}
                </button>
                <div className="mx-1 h-8 w-px bg-line" aria-hidden />
                <button type="button" onClick={endCall} title="Leave call" aria-label="Leave call"
                  className="flex size-11 items-center justify-center rounded-lg bg-brand text-brand-on transition-colors hover:bg-brand-hover focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[rgb(var(--focus)/0.7)] md:size-12">
                  <PhoneOff className="size-5" aria-hidden />
                </button>
              </div>
            </div>
          </div>

          {/* Chat: full-screen overlay on mobile, side panel on desktop */}
          {showChat && (
            <aside aria-label="Call chat" className="fixed inset-0 z-30 flex w-full shrink-0 flex-col border-l border-line bg-surface pb-[env(safe-area-inset-bottom)] sm:static sm:inset-auto sm:z-auto sm:w-72 sm:pb-0">
              <div className="flex items-center justify-between border-b border-line px-4 py-2.5">
                <h2 className="text-sm font-semibold text-fg">Chat</h2>
                <IconButton icon={X} label="Close chat" onClick={() => setShowChat(false)} />
              </div>
              <div className="flex-1 space-y-2 overflow-y-auto p-3">
                {chatMsgs.map((m, i) => (
                  <div key={i} className={cn("flex flex-col", m.isMe ? "items-end" : "items-start")}>
                    <span className="mb-0.5 text-xs text-fg-muted">{m.isMe ? "You" : m.name}</span>
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

      {state === "ended" && (
        <StatusScreen icon={PhoneOff} title="Call ended" text="You have left the call." />
      )}
    </div>
  );
}
