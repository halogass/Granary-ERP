import Link from "next/link";
import { ChevronLeft } from "lucide-react";
import { cn } from "@/lib/utils";

export interface PageHeaderProps {
  title: React.ReactNode;
  /** One line under or beside the title: counts, totals ("14 open, USD 182,000"). */
  summary?: React.ReactNode;
  /** A status pill or similar, right after the title (detail pages). */
  status?: React.ReactNode;
  /** Buttons on the right; the primary action last. */
  actions?: React.ReactNode;
  /** "Back to Deals" link above the title on detail pages. */
  back?: { href: string; label: string };
  /** Sits flush on top of SavedViewTabs / FilterChips / a DataTable (list pages). */
  flush?: boolean;
  className?: string;
  children?: React.ReactNode;
}

/**
 * The top of every page. List pages: <PageHeader flush> then SavedViewTabs, FilterChips, DataTable,
 * all inside one <ListFrame>. Detail and form pages: a normal PageHeader then Cards.
 */
export function PageHeader({ title, summary, status, actions, back, flush, className, children }: PageHeaderProps) {
  return (
    <header className={cn(flush ? "border-b border-line bg-surface px-4 py-3" : "mb-4", className)}>
      {back && (
        <Link href={back.href} className="mb-1 inline-flex items-center gap-1 text-xs text-fg-muted hover:text-fg">
          <ChevronLeft className="size-3.5" aria-hidden /> {back.label}
        </Link>
      )}
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        {/* 21 Nov 2026: never cut the title (a document number was cut to "QUO-2026-11-0000..." by the buttons):
            the title block keeps its smallest width and the buttons wrap to the next line instead */}
        <div data-slot="page-title" className="flex min-w-0 flex-1 basis-full flex-wrap items-baseline gap-x-3 gap-y-0.5 sm:min-w-min sm:basis-0">
          <h1 className="text-lg font-semibold leading-tight text-fg">{title}</h1>
          {status && <span className="self-center">{status}</span>}
          {summary && <p className="text-[13px] text-fg-muted tabular">{summary}</p>}
        </div>
        {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
      </div>
      {children}
    </header>
  );
}

/**
 * The one surface a list page lives in: header, views, chips and table share its border so
 * they read as a single tool. Bleeds to the edges on phones.
 */
export function ListFrame({ className, children }: { className?: string; children: React.ReactNode }) {
  return (
    <div className={cn("-mx-4 overflow-hidden border-y border-line bg-surface md:mx-0 md:rounded-lg md:border", className)}>
      {children}
    </div>
  );
}

/**
 * A page that fills the space between the Topbar and the phone BottomNav exactly (inboxes, chats,
 * calls): it cancels the shell's padding and takes the height left by the shell variables in
 * globals.css, so the page itself never scrolls; its panes scroll inside.
 */
export function FullHeightPage({ className, children }: { className?: string; children: React.ReactNode }) {
  return (
    <div className={cn("-m-4 -mb-24 flex h-[calc(100dvh-var(--shell-top)-var(--shell-bottom))] overflow-hidden md:-m-6",
      className)}>
      {children}
    </div>
  );
}

/** A row of controls (search, filters, actions) above content that is not in a ListFrame. */
export function Toolbar({ className, children }: { className?: string; children: React.ReactNode }) {
  return <div className={cn("flex flex-wrap items-center gap-2", className)}>{children}</div>;
}

/** Pushes the following toolbar items to the right. */
export function ToolbarSpacer() {
  return <span className="ml-auto" aria-hidden />;
}
