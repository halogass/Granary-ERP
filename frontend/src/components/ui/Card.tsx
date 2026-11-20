import { cn } from "@/lib/utils";

type DivProps = React.HTMLAttributes<HTMLDivElement>;

/** A bordered surface for one section of a page (detail sections, forms, dashboard blocks). */
export function Card({ className, ...props }: DivProps) {
  return <section className={cn("rounded-lg border border-line bg-surface shadow-card", className)} {...props} />;
}

/** Title row of a Card: title on the left, actions on the right. */
export function CardHeader({ title, description, actions, className, children }: {
  title?: React.ReactNode; description?: React.ReactNode; actions?: React.ReactNode; className?: string; children?: React.ReactNode;
}) {
  return (
    <header className={cn("flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-line px-4 py-2.5", className)}>
      <div className="min-w-0 flex-1">
        {title && <h2 className="text-sm font-semibold text-fg">{title}</h2>}
        {description && <p className="text-xs text-fg-muted">{description}</p>}
        {children}
      </div>
      {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
    </header>
  );
}

export function CardBody({ className, ...props }: DivProps) {
  return <div className={cn("p-4", className)} {...props} />;
}

export function CardFooter({ className, ...props }: DivProps) {
  return <footer className={cn("flex items-center justify-end gap-2 border-t border-line px-4 py-2.5", className)} {...props} />;
}

/** Label/value pairs in two columns (record details, an expanded row). */
export function KeyValues({ items, className, cols = 2 }: {
  items: { label: React.ReactNode; value: React.ReactNode; numeric?: boolean }[]; className?: string; cols?: 1 | 2 | 3;
}) {
  const grid = { 1: "sm:grid-cols-1", 2: "sm:grid-cols-2", 3: "sm:grid-cols-3" }[cols];
  return (
    <dl className={cn("grid grid-cols-1 gap-x-6 gap-y-2 text-[13px]", grid, className)}>
      {items.map((it, i) => (
        <div key={i} className="flex min-w-0 justify-between gap-3 sm:block">
          <dt className="text-fg-muted">{it.label}</dt>
          <dd className={cn("min-w-0 text-fg sm:mt-0.5", it.numeric && "tabular")}>{it.value ?? "-"}</dd>
        </div>
      ))}
    </dl>
  );
}
