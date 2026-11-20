import Link from "next/link";
import { cn } from "@/lib/utils";

export interface EmptyStateProps {
  icon?: React.ElementType;
  title: React.ReactNode;
  /** What to do next, in one sentence. */
  description?: React.ReactNode;
  /** The action that fills the space (usually one Button). */
  action?: React.ReactNode;
  className?: string;
  compact?: boolean;
}

/** Nothing to show yet: say why and offer the next step. */
export function EmptyState({ icon: Icon, title, description, action, className, compact }: EmptyStateProps) {
  return (
    <div className={cn("flex flex-col items-center justify-center gap-2 px-6 text-center", compact ? "py-6" : "py-12", className)}>
      {Icon && (
        <span className="mb-1 flex size-10 items-center justify-center rounded-lg bg-surface-2 text-fg-muted">
          <Icon className="size-5" aria-hidden />
        </span>
      )}
      <p className="text-sm font-medium text-fg">{title}</p>
      {description && <p className="max-w-sm text-[13px] text-fg-muted">{description}</p>}
      {action && <div className="mt-2">{action}</div>}
    </div>
  );
}

/** A grey placeholder block while data loads. Size it with className (h-4 w-32...). */
export function Skeleton({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div aria-hidden className={cn("animate-pulse rounded-md bg-surface-2", className)} {...props} />;
}

/** A keyboard key: <Kbd>Ctrl</Kbd> <Kbd>K</Kbd>. */
export function Kbd({ className, children }: { className?: string; children: React.ReactNode }) {
  return (
    <kbd className={cn("inline-flex h-5 min-w-5 items-center justify-center rounded border border-line bg-surface-2 px-1 font-sans text-[11px] font-medium text-fg-muted", className)}>
      {children}
    </kbd>
  );
}

export interface StatProps {
  label: React.ReactNode;
  value: React.ReactNode;
  /** A short line under the value ("12 overdue", "+8% vs last month"). */
  note?: React.ReactNode;
  /** Red note and a red mark: this number needs attention. */
  alert?: boolean;
  href?: string;
  className?: string;
}

/** A labelled number for dashboards and summary rows. Group several in <StatGroup>. */
export function Stat({ label, value, note, alert, href, className }: StatProps) {
  const body = (
    <>
      <span className="text-[13px] text-fg-muted">{label}</span>
      <span className="text-xl font-semibold leading-tight text-fg tabular">{value}</span>
      {note && <span className={cn("text-xs tabular", alert ? "font-medium text-tone-red" : "text-fg-muted")}>{note}</span>}
    </>
  );
  const cls = cn("relative flex min-w-0 flex-col gap-1 bg-surface px-4 py-3",
    alert && "before:absolute before:inset-y-3 before:left-0 before:w-0.5 before:rounded-full before:bg-brand", className);
  if (href) {
    return <Link href={href} className={cn(cls, "transition-colors hover:bg-surface-2")}>{body}</Link>;
  }
  return <div className={cls}>{body}</div>;
}

/** Stats in one bordered strip, divided by hairlines; wraps on small screens. */
export function StatGroup({ className, children }: { className?: string; children: React.ReactNode }) {
  return (
    <div className={cn("grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-line bg-line sm:grid-cols-[repeat(auto-fit,minmax(160px,1fr))]", className)}>
      {children}
    </div>
  );
}
