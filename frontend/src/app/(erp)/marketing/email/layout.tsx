"use client";

import { useEffect, useState } from "react";
import { Loader2, Lock } from "lucide-react";
import { emailMarketingApi } from "@/lib/api";
import { Card } from "@/components/ui";

/** Email marketing is limited to the marketing team (Marketing role, Marketing Team Lead) and the Owner. */
export default function EmailMarketingLayout({ children }: { children: React.ReactNode }) {
  const [allowed, setAllowed] = useState<boolean | null>(null);

  useEffect(() => {
    emailMarketingApi.access().then((r) => setAllowed(!!r.data.allowed)).catch(() => setAllowed(false));
  }, []);

  if (allowed === null) {
    return <div className="flex justify-center py-20"><Loader2 className="size-6 animate-spin text-fg-muted" aria-label="Loading" /></div>;
  }
  if (!allowed) {
    return (
      <Card className="mx-auto mt-16 max-w-md px-6 py-10 text-center">
        <span className="mx-auto mb-3 flex size-10 items-center justify-center rounded-lg bg-surface-2 text-fg-muted">
          <Lock className="size-5" aria-hidden />
        </span>
        <h1 className="text-base font-semibold text-fg">Email marketing is restricted</h1>
        <p className="mt-2 text-[13px] text-fg-muted">
          Only the marketing team and the Owner can use email marketing. Ask an admin for the
          &ldquo;Email marketing&rdquo; permission if you need access.
        </p>
      </Card>
    );
  }
  return <>{children}</>;
}
