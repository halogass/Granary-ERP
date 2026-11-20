"use client";

export const dynamic = "force-dynamic";

import { Suspense, useEffect, useState } from "react";
import { useSearchParams } from "next/navigation";
import { useSession } from "next-auth/react";
import { Loader2 } from "lucide-react";
import { hrApi } from "@/lib/api";
import PersonalClock from "@/components/attendance/PersonalClock";
import KioskClock from "@/components/attendance/KioskClock";
import { cn } from "@/lib/utils";

interface Today { has_employee: boolean; check_in?: string | null; check_out?: string | null }

/** Clock-in on your own device: sign-in required; the server verifies your face against your account. */
function Personal() {
  const { status } = useSession();
  const [today, setToday] = useState<Today | null>(null);
  const [eventType, setEventType] = useState<"clock_in" | "clock_out">("clock_in");
  const [now, setNow] = useState(new Date());

  useEffect(() => {
    const t = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(t);
  }, []);

  useEffect(() => {
    if (status === "unauthenticated") window.location.href = "/login?callbackUrl=%2Fclock";
    if (status !== "authenticated") return;
    hrApi.myToday().then((r) => {
      setToday(r.data);
      setEventType(r.data.check_in && !r.data.check_out ? "clock_out" : "clock_in");
    }).catch(() => setToday({ has_employee: false }));
  }, [status]);

  if (status !== "authenticated" || !today) {
    return <div className="flex min-h-screen items-center justify-center bg-canvas"><Loader2 className="size-8 animate-spin text-fg-muted" aria-label="Loading" /></div>;
  }

  return (
    <div className="flex min-h-screen flex-col items-center justify-center bg-canvas p-4">
      <div className="w-full max-w-sm overflow-hidden rounded-lg border border-line bg-surface shadow-pop">
        <div className="bg-brand px-6 py-5 text-center text-brand-on">
          <div className="mb-1 text-xs font-semibold uppercase tracking-widest opacity-80">Granary</div>
          <div className="text-4xl font-semibold tracking-wider tabular">
            {now.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit", second: "2-digit" })}
          </div>
          <div className="mt-1 text-xs opacity-80 tabular">
            {today.check_in ? `In ${today.check_in}${today.check_out ? `, out ${today.check_out}` : ""}` : "Not clocked in yet"}
          </div>
        </div>
        <div className="space-y-3 p-4">
          {!today.has_employee ? (
            <p className="py-6 text-center text-sm text-fg-muted">Your account is not linked to an employee record.</p>
          ) : (
            <>
              <div role="tablist" aria-label="Clock in or out" className="flex gap-1 rounded-lg bg-surface-2 p-1">
                {(["clock_in", "clock_out"] as const).map((t) => (
                  <button key={t} type="button" role="tab" aria-selected={eventType === t} onClick={() => setEventType(t)}
                    className={cn("h-11 flex-1 rounded-md text-sm font-semibold md:h-9",
                      eventType === t ? "bg-surface text-fg shadow-card" : "text-fg-muted hover:text-fg")}>
                    {t === "clock_in" ? "Clock in" : "Clock out"}
                  </button>
                ))}
              </div>
              <PersonalClock key={eventType} eventType={eventType}
                onDone={() => hrApi.myToday().then((r) => setToday(r.data)).catch(() => {})} />
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function ClockPageInner() {
  const params = useSearchParams();
  return params.get("kiosk") === "1" ? <KioskClock /> : <Personal />;
}

export default function ClockPage() {
  return (
    <Suspense>
      <ClockPageInner />
    </Suspense>
  );
}
