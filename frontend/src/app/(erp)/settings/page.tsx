"use client";

import { Suspense } from "react";
import SettingsView from "@/components/settings/SettingsView";

/**
 * Settings (S2): ten areas, each one long page of sections with a save bar. The registry is
 * src/lib/settings-registry.ts; old ?tab= and #anchor URLs redirect to their area and section.
 */
export default function SettingsPage() {
  return (
    <Suspense fallback={null}>
      <SettingsView />
    </Suspense>
  );
}
