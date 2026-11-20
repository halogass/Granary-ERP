"use client";

import { signOut } from "next-auth/react";
import {
  Bell, LogOut, ChevronDown, CheckCheck, ExternalLink, Search,
  Clock, CheckCircle2, User, Monitor, Sun, Moon,
} from "lucide-react";
import { useState, useEffect, useRef, useCallback } from "react";
import { systemApi, hrApi } from "@/lib/api";
import PersonalClock, { type ClockResult } from "@/components/attendance/PersonalClock";
import Link from "next/link";
import { useRouter } from "next/navigation";
import BrandLogo from "@/components/BrandLogo";
import { cn } from "@/lib/utils";
import { useTheme, type ThemePref } from "@/lib/theme";
import { DropdownMenu, MenuItem, MenuRadioGroup, MenuSeparator } from "@/components/ui/Menu";
import { Kbd } from "@/components/ui/Feedback";
import { Dialog } from "@/components/ui/Dialog";
import { openCommandPalette } from "./CommandPalette";

interface TopbarProps {
  user: { name: string; email: string; is_superadmin: boolean; portal_type: string };
}

interface Notification {
  id: string;
  type: string;
  title: string;
  body?: string;
  link?: string;
  is_read: boolean;
  priority: string;
  created_at?: string;
}

interface TodayStatus {
  has_employee: boolean;
  employee_id?: string;
  check_in?: string | null;
  check_out?: string | null;
  status?: string | null;
  work_hours?: number | null;
  date?: string;
}

function timeAgo(iso?: string) {
  if (!iso) return "";
  const diff = Date.now() - new Date(iso).getTime();
  const m = Math.floor(diff / 60000);
  if (m < 1) return "just now";
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  return `${Math.floor(h / 24)}d ago`;
}

function getUserInitials(name: string): string {
  return name.split(" ").filter(Boolean).map((n) => n[0]).join("").slice(0, 2).toUpperCase();
}

function formatWorkHours(wh: number | null | undefined): string {
  if (!wh) return "-";
  const h = Math.floor(wh);
  const m = Math.round((wh - h) * 60);
  if (h === 0) return `${m}m`;
  if (m === 0) return `${h}h`;
  return `${h}h ${m}m`;
}

function useElapsed(checkIn: string | null | undefined): string {
  const [elapsed, setElapsed] = useState("");
  useEffect(() => {
    if (!checkIn) { setElapsed(""); return; }
    const tick = () => {
      const [h, min] = checkIn.split(":").map(Number);
      const now = new Date();
      const ciMs = (h * 60 + min) * 60000;
      const nowMs = (now.getHours() * 60 + now.getMinutes()) * 60000 + now.getSeconds() * 1000;
      let diff = nowMs - ciMs;
      if (diff < 0) diff = 0;
      const totalSec = Math.floor(diff / 1000);
      const hh = Math.floor(totalSec / 3600);
      const mm = Math.floor((totalSec % 3600) / 60);
      const ss = totalSec % 60;
      setElapsed(`${String(hh).padStart(2, "0")}:${String(mm).padStart(2, "0")}:${String(ss).padStart(2, "0")}`);
    };
    tick();
    const id = setInterval(tick, 1000);
    return () => clearInterval(id);
  }, [checkIn]);
  return elapsed;
}

const PRIORITY_DOT: Record<string, string> = {
  low: "bg-fg-subtle", normal: "bg-tone-blue", high: "bg-tone-orange", urgent: "bg-brand",
};

// ─── Clock Modal ──────────────────────────────────────────────────────────────

interface ClockModalProps {
  today: TodayStatus;
  onClose: () => void;
  onSuccess: (updated: TodayStatus) => void;
}

function ClockModal({ today, onClose, onSuccess }: ClockModalProps) {
  const elapsed = useElapsed(today.check_in && !today.check_out ? today.check_in : null);

  const action = !today.check_in ? "clock_in" : !today.check_out ? "clock_out" : "done";

  // Recognised or sent to HR review: show the result briefly, then refresh the topbar status.
  async function finished(result: ClockResult) {
    if (result.status !== "success") return;
    const fresh = await hrApi.myToday();
    setTimeout(() => onSuccess(fresh.data), 1500);
  }

  const now = new Date();
  const dateLabel = now.toLocaleDateString("en-GB", { weekday: "long", day: "numeric", month: "long", year: "numeric" });

  return (
    <Dialog open onOpenChange={(o) => { if (!o) onClose(); }} size="sm" title="Attendance" description={dateLabel}>
        <div className="space-y-4">
          {/* Status row */}
          <div className="flex items-center justify-between text-sm">
            <div className="flex items-center gap-3">
              {today.check_in ? (
                <>
                  <div className="text-center">
                    <div className="text-xs text-fg-muted">In</div>
                    <div className="font-semibold text-fg tabular">{today.check_in}</div>
                  </div>
                  <div className="text-fg-subtle" aria-hidden>to</div>
                  <div className="text-center">
                    <div className="text-xs text-fg-muted">Out</div>
                    <div className="font-semibold text-fg tabular">{today.check_out ?? "-"}</div>
                  </div>
                </>
              ) : (
                <span className="text-fg-muted text-xs">Not clocked in yet</span>
              )}
            </div>
            {action === "clock_out" && elapsed && (
              <div className="text-right">
                <div className="text-xs text-fg-muted">Elapsed</div>
                <div className="font-semibold text-brand-ink text-sm tabular">{elapsed}</div>
              </div>
            )}
            {action === "done" && today.work_hours != null && (
              <div className="flex items-center gap-1.5 text-tone-blue font-semibold text-sm">
                <CheckCircle2 className="w-4 h-4" />
                {formatWorkHours(today.work_hours)}
              </div>
            )}
          </div>

          {/* Camera / result / done */}
          {action === "done" ? (
            <div className="flex flex-col items-center gap-3 py-4">
              <CheckCircle2 className="w-12 h-12 text-tone-blue" />
              <p className="text-sm font-medium text-fg">All done for today</p>
              <p className="text-xs text-fg-muted tabular">
                {today.check_in} to {today.check_out}, {formatWorkHours(today.work_hours)}
              </p>
            </div>
          ) : (
            <PersonalClock eventType={action} onDone={finished} />
          )}
        </div>
    </Dialog>
  );
}


// ─── Topbar ───────────────────────────────────────────────────────────────────

const THEME_OPTIONS = [
  { value: "system", label: "Same as computer", icon: Monitor },
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
];

export default function Topbar({ user }: TopbarProps) {
  const [notifOpen, setNotifOpen] = useState(false);
  const [notifications, setNotifications] = useState<Notification[]>([]);
  const [unreadCount, setUnreadCount] = useState(0);
  const [clockOpen, setClockOpen] = useState(false);
  const [todayStatus, setTodayStatus] = useState<TodayStatus | null>(null);
  const notifRef = useRef<HTMLDivElement>(null);
  const { theme, setTheme } = useTheme();
  const router = useRouter();

  const fetchNotifications = useCallback(async () => {
    try {
      const [listRes, countRes] = await Promise.all([
        systemApi.listNotifications({ limit: 15 }),
        systemApi.getUnreadCount(),
      ]);
      setNotifications(listRes.data ?? []);
      setUnreadCount(countRes.data?.count ?? 0);
    } catch { /* silent */ }
  }, []);

  const fetchTodayStatus = useCallback(async () => {
    try {
      const res = await hrApi.myToday();
      setTodayStatus(res.data);
    } catch { /* silent: user may not be HR */ }
  }, []);

  useEffect(() => {
    fetchNotifications();
    const interval = setInterval(fetchNotifications, 30000);
    return () => clearInterval(interval);
  }, [fetchNotifications]);

  useEffect(() => {
    fetchTodayStatus();
  }, [fetchTodayStatus]);

  useEffect(() => {
    function handler(e: MouseEvent) {
      if (notifRef.current && !notifRef.current.contains(e.target as Node)) {
        setNotifOpen(false);
      }
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setNotifOpen(false);
    }
    document.addEventListener("mousedown", handler);
    document.addEventListener("keydown", onKey);
    return () => { document.removeEventListener("mousedown", handler); document.removeEventListener("keydown", onKey); };
  }, []);

  async function markRead(id: string) {
    await systemApi.markNotificationRead(id);
    setNotifications((ns) => ns.map((n) => n.id === id ? { ...n, is_read: true } : n));
    setUnreadCount((c) => Math.max(0, c - 1));
  }

  async function markAllRead() {
    await systemApi.markAllNotificationsRead();
    setNotifications((ns) => ns.map((n) => ({ ...n, is_read: true })));
    setUnreadCount(0);
  }

  const initials = getUserInitials(user.name);

  // Clock button state
  const hasEmployee = todayStatus?.has_employee;
  const checkedIn = !!todayStatus?.check_in;
  const checkedOut = !!todayStatus?.check_out;
  const elapsed = useElapsed(checkedIn && !checkedOut ? todayStatus?.check_in : null);

  const iconBtn = "relative flex size-11 md:size-9 items-center justify-center rounded-lg text-fg-muted hover:bg-surface-2 hover:text-fg transition-colors";

  return (
    <>
      <header className="flex h-[var(--shell-top)] shrink-0 items-center gap-2 border-b border-line bg-surface px-3 md:px-4">
        {/* Mobile-only logo */}
        <div className="flex min-w-0 shrink items-center overflow-hidden md:hidden [&_img]:max-w-full">
          <BrandLogo variant="sidebar" />
        </div>

        {/* Search: opens the command palette */}
        <button type="button" onClick={openCommandPalette}
          className="hidden h-8 w-full max-w-xs items-center gap-2 rounded-lg border border-line bg-canvas px-2.5 text-left text-[13px] text-fg-subtle transition-colors hover:border-line-strong hover:text-fg-muted md:flex">
          <Search className="size-3.5 shrink-0" aria-hidden />
          <span className="flex-1 truncate">Go to a page, record or setting</span>
          <Kbd>Ctrl K</Kbd>
        </button>

        <div className="ml-auto flex shrink-0 items-center gap-1">
          <button type="button" onClick={openCommandPalette} aria-label="Search" className={cn(iconBtn, "md:hidden")}>
            <Search className="size-[18px]" aria-hidden />
          </button>

          {/* Clock In / Out: only shown if an employee record exists */}
          {hasEmployee && (
            <button
              type="button"
              onClick={() => setClockOpen(true)}
              className={cn(
                "flex h-11 items-center gap-1.5 rounded-lg px-2.5 text-xs font-semibold transition-colors md:h-8",
                checkedOut
                  ? "bg-tone-blue-soft text-tone-blue"
                  : checkedIn
                  ? "bg-tone-orange-soft text-tone-orange"
                  : "bg-brand/[0.08] text-brand-ink hover:bg-brand/[0.14]"
              )}
              title={checkedOut ? "View today's attendance" : checkedIn ? "Clock Out" : "Clock In"}
              aria-label={checkedOut ? "View today's attendance" : checkedIn ? "Clock out" : "Clock in"}
            >
              <Clock className="w-3.5 h-3.5 shrink-0" aria-hidden />
              {checkedOut ? (
                <span className="flex items-center gap-1 tabular">
                  <CheckCircle2 className="w-3 h-3" aria-hidden />
                  {todayStatus?.work_hours ? formatWorkHours(todayStatus.work_hours) : "Done"}
                </span>
              ) : checkedIn ? (
                <span className="tabular">{elapsed || todayStatus?.check_in}</span>
              ) : (
                "Clock In"
              )}
            </button>
          )}

          {/* Notifications */}
          <div className="relative" ref={notifRef}>
            <button
              type="button"
              aria-label={unreadCount > 0 ? `Notifications, ${unreadCount} unread` : "Notifications"}
              aria-expanded={notifOpen}
              onClick={() => { setNotifOpen(!notifOpen); if (!notifOpen) fetchNotifications(); }}
              className={iconBtn}
            >
              <Bell className="size-[18px]" aria-hidden />
              {unreadCount > 0 && (
                <span className="absolute right-1 top-1 flex h-4 min-w-4 items-center justify-center rounded-full bg-brand px-1 text-[10px] font-semibold leading-none text-brand-on tabular">
                  {unreadCount > 9 ? "9+" : unreadCount}
                </span>
              )}
            </button>

            {notifOpen && (
              <div className={cn("z-50 overflow-hidden rounded-lg border border-line bg-surface-raised shadow-pop",
                // phones: a panel across the screen under the Topbar; desktop: a dropdown under the bell
                "fixed inset-x-2 top-[calc(var(--shell-top)+0.25rem)] md:absolute md:inset-x-auto md:right-0 md:top-full md:mt-2 md:w-[22rem]")}>
                <div className="flex items-center justify-between border-b border-line px-4 py-2.5">
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-semibold text-fg">Notifications</span>
                    {unreadCount > 0 && <span className="text-xs text-brand-ink tabular">{unreadCount} unread</span>}
                  </div>
                  {unreadCount > 0 && (
                    <button
                      type="button"
                      onClick={markAllRead}
                      className="flex min-h-11 items-center gap-1 rounded-md px-1.5 py-1 text-xs font-medium text-brand-ink hover:bg-surface-2 md:min-h-0"
                    >
                      <CheckCheck className="w-3.5 h-3.5" aria-hidden />
                      Mark all read
                    </button>
                  )}
                </div>

                <div className="max-h-[calc(100dvh-var(--shell-top)-var(--shell-bottom)-5rem)] overflow-y-auto overscroll-contain md:max-h-[360px]">
                  {notifications.length === 0 ? (
                    <div className="flex flex-col items-center justify-center gap-2 py-10">
                      <Bell className="size-5 text-fg-subtle" aria-hidden />
                      <p className="text-sm text-fg-muted">No notifications</p>
                    </div>
                  ) : (
                    notifications.map((n) => (
                      <div
                        key={n.id}
                        className={cn(
                          "border-b border-line-soft px-4 py-3 transition-colors hover:bg-surface-2",
                          !n.is_read && "bg-tone-blue-soft/40"
                        )}
                      >
                        <div className="flex items-start gap-3">
                          <span
                            className={cn("mt-1.5 size-2 shrink-0 rounded-full", n.is_read ? "bg-line-strong" : PRIORITY_DOT[n.priority] ?? "bg-tone-blue")}
                            aria-hidden
                          />
                          <div className="min-w-0 flex-1">
                            <p className={cn("text-[13px] leading-snug", n.is_read ? "text-fg-muted" : "font-semibold text-fg")}>
                              {n.title}
                            </p>
                            {n.body && <p className="mt-0.5 line-clamp-2 text-xs text-fg-muted">{n.body}</p>}
                            <p className="mt-1 text-xs text-fg-subtle">{timeAgo(n.created_at)}</p>
                          </div>
                          <div className="flex shrink-0 items-center gap-0.5">
                            {n.link && (
                              <Link
                                href={n.link}
                                aria-label="Open"
                                onClick={() => { markRead(n.id); setNotifOpen(false); }}
                                className="flex size-11 items-center justify-center rounded-md text-fg-muted hover:bg-surface hover:text-fg md:size-auto md:p-1.5"
                              >
                                <ExternalLink className="w-3.5 h-3.5" aria-hidden />
                              </Link>
                            )}
                            {!n.is_read && (
                              <button
                                type="button"
                                aria-label="Mark read"
                                onClick={() => markRead(n.id)}
                                className="flex size-11 items-center justify-center rounded-md text-fg-muted hover:bg-surface hover:text-brand-ink md:size-auto md:p-1.5"
                              >
                                <CheckCheck className="w-3.5 h-3.5" aria-hidden />
                              </button>
                            )}
                          </div>
                        </div>
                      </div>
                    ))
                  )}
                </div>
              </div>
            )}
          </div>

          {/* User menu */}
          <DropdownMenu
            trigger={
              <button type="button" aria-label="Account menu"
                className="flex h-11 min-w-11 items-center justify-center gap-2 rounded-lg px-1.5 transition-colors hover:bg-surface-2 md:h-9 md:min-w-0">
                <span className="flex size-7 shrink-0 items-center justify-center rounded-full bg-brand/[0.1] text-[11px] font-semibold text-brand-ink">
                  {initials}
                </span>
                <span className="hidden text-left sm:block">
                  <span className="block text-[13px] font-medium leading-tight text-fg">{user.name}</span>
                </span>
                <ChevronDown className="hidden size-3.5 text-fg-subtle sm:block" aria-hidden />
              </button>
            }
            className="w-60"
          >
            <div className="px-2.5 py-2">
              <div className="truncate text-sm font-semibold text-fg">{user.name}</div>
              <div className="truncate text-xs text-fg-muted">{user.email}</div>
              <div className="text-xs text-fg-muted">{user.is_superadmin ? "Superadmin" : "Staff"}</div>
            </div>
            <MenuSeparator />
            {hasEmployee && (
              <MenuItem icon={Clock} onSelect={() => setClockOpen(true)}>
                {checkedOut ? "View attendance" : checkedIn ? "Clock out" : "Clock in"}
              </MenuItem>
            )}
            <MenuItem icon={User} onSelect={() => router.push("/profile")}>My profile</MenuItem>
            <MenuSeparator />
            <MenuRadioGroup label="Theme" value={theme} onValueChange={(v) => setTheme(v as ThemePref)} options={THEME_OPTIONS} />
            <MenuSeparator />
            <MenuItem icon={LogOut} onSelect={() => signOut({ callbackUrl: "/login" })}>Sign out</MenuItem>
          </DropdownMenu>
        </div>
      </header>

      {/* Clock Modal */}
      {clockOpen && todayStatus && (
        <ClockModal
          today={todayStatus}
          onClose={() => setClockOpen(false)}
          onSuccess={(updated) => {
            setTodayStatus(updated);
            setClockOpen(false);
          }}
        />
      )}
    </>
  );
}
