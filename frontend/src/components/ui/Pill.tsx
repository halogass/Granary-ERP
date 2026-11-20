import { cn } from "@/lib/utils";
import { TONE_CLASSES, statusLabel, statusTone, type StatusFamily, type Tone } from "./status";

export interface PillProps extends React.HTMLAttributes<HTMLSpanElement> {
  tone?: Tone;
  /** A small leading icon (lucide). */
  icon?: React.ElementType;
}

/** A soft colored pill: tinted background, colored text, fully rounded. */
export function Pill({ tone = "neutral", icon: Icon, className, children, ...props }: PillProps) {
  return (
    <span className={cn("inline-flex h-[22px] max-w-full items-center gap-1 whitespace-nowrap rounded-full px-2.5 text-xs font-medium",
      TONE_CLASSES[tone], className)} {...props}>
      {Icon && <Icon aria-hidden className="size-3 shrink-0" />}
      <span className="truncate">{children}</span>
    </span>
  );
}

export interface StatusPillProps extends Omit<PillProps, "tone" | "children"> {
  family?: StatusFamily;
  status: string | null | undefined;
  /** The text to show; defaults to the status, humanized. */
  label?: React.ReactNode;
  /** Override the mapped tone (rare: a page-specific rule like "due today"). */
  tone?: Tone;
}

/** A status or stage, colored by its family's shared mapping (components/ui/status.ts). */
export function StatusPill({ family = "generic", status, label, tone, ...props }: StatusPillProps) {
  return <Pill tone={tone ?? statusTone(family, status)} data-status={status ?? ""} {...props}>{label ?? statusLabel(status)}</Pill>;
}

/** A red count for "needs you" (nav items, tabs). Renders nothing at zero. */
export function CountBadge({ count, className, tone = "brand" }: { count?: number | null; className?: string; tone?: "brand" | "muted" }) {
  if (!count) return null;
  return (
    <span className={cn("inline-flex h-[18px] min-w-[18px] items-center justify-center rounded-full px-1.5 text-[11px] font-semibold leading-none tabular",
      tone === "brand" ? "bg-brand text-brand-on" : "bg-surface-2 text-fg-muted", className)}>
      {count > 99 ? "99+" : count}
    </span>
  );
}
