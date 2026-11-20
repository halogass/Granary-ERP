import { cn } from "@/lib/utils";

const AVATAR_TONES = [
  "bg-tone-blue-soft text-tone-blue",
  "bg-tone-violet-soft text-tone-violet",
  "bg-tone-orange-soft text-tone-orange",
  "bg-tone-neutral-soft text-tone-neutral",
];

const SIZES = {
  xs: "size-6 text-[10px]",
  sm: "size-7 text-xs",
  md: "size-9 text-sm",
  lg: "size-12 text-base",
  xl: "size-20 text-2xl",
};

/** Initials on a soft tone picked from the name; an optional online dot. */
export default function Avatar({ name, online, size = "md", className }: {
  name: string; online?: boolean; size?: keyof typeof SIZES; className?: string;
}) {
  const initials = (name || "?").split(" ").map((w) => w[0]).join("").slice(0, 2).toUpperCase();
  const tone = AVATAR_TONES[((name || "?").charCodeAt(0) || 0) % AVATAR_TONES.length];
  return (
    <div className={cn("relative shrink-0", className)}>
      <div className={cn(SIZES[size], tone, "flex items-center justify-center rounded-full font-semibold")} aria-hidden>
        {initials}
      </div>
      {online !== undefined && (
        <span className={cn("absolute -bottom-0.5 -right-0.5 size-2.5 rounded-full border-2 border-surface",
          online ? "bg-tone-blue" : "bg-line-strong")} aria-label={online ? "Online" : "Offline"} />
      )}
    </div>
  );
}
