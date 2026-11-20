"use client";

import { WifiOff, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui";

export default function OfflinePage() {
  return (
    <div className="flex min-h-screen flex-col items-center justify-center bg-canvas px-4 text-fg">
      <div className="max-w-xs text-center">
        <span className="mx-auto mb-6 flex size-16 items-center justify-center rounded-lg bg-tone-neutral-soft text-tone-neutral">
          <WifiOff className="size-8" aria-hidden />
        </span>
        <h1 className="mb-2 text-xl font-semibold text-fg">You are offline</h1>
        <p className="mb-8 text-sm leading-relaxed text-fg-muted">
          Check your internet connection and try again.
        </p>
        <Button variant="primary" size="lg" onClick={() => location.reload()}>
          <RefreshCw aria-hidden />
          Try again
        </Button>
        <p className="mt-10 text-xs font-medium text-fg-subtle">
          <span className="text-brand-ink">Granary</span> ERP
        </p>
      </div>
    </div>
  );
}
