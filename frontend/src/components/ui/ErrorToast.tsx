"use client";

import { AlertTriangle, X } from "lucide-react";

/** Dismissable error shown at the bottom of the screen, for actions on list rows that have no inline error slot. */
export default function ErrorToast({ message, onClose }: { message: string; onClose: () => void }) {
  if (!message) return null;
  return (
    <div role="alert"
      className="fixed bottom-20 left-4 right-4 md:bottom-4 sm:left-auto sm:max-w-md z-50 flex items-start gap-3 bg-surface-raised border border-tone-red/30 shadow-pop rounded-lg px-4 py-3 animate-fade-up">
      <AlertTriangle className="w-4 h-4 text-tone-red shrink-0 mt-0.5" />
      <p className="text-sm text-fg flex-1 break-words">{message}</p>
      <button onClick={onClose} aria-label="Dismiss" className="-m-2 p-2 text-fg-muted hover:text-fg">
        <X className="w-4 h-4" />
      </button>
    </div>
  );
}
