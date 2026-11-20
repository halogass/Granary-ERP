"use client";

import { Toaster as SonnerToaster, toast } from "sonner";
import { useTheme } from "@/lib/theme";

/**
 * Toasts: `toast.success("Invoice sent")`, `toast.error(extractApiError(e))`.
 * Name the outcome with the same verb as the button ("Send" -> "Sent").
 * For an error on a list row with no inline slot, ErrorToast is still available.
 */
export { toast };

/** Mounted once in app/providers.tsx; follows the app theme, not only the OS. */
export function Toaster() {
  const { resolved } = useTheme();
  return (
    <SonnerToaster theme={resolved} richColors position="top-right"
      toastOptions={{ style: { fontFamily: "var(--font-sans)", borderRadius: "8px" } }} />
  );
}
