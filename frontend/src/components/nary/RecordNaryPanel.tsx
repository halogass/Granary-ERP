"use client";

import { useCallback, useEffect, useState } from "react";
import { Bot } from "lucide-react";
import { naryApi } from "@/lib/api";
import { usePermissions } from "@/hooks/usePermissions";
import CardView from "./CardView";
import type { NaryCard } from "./types";

/** Nary's open cards about one record, on that record's page. Hidden when there are none. */
export default function RecordNaryPanel({ entityType, entityId }: { entityType: string; entityId: string }) {
  const { can, loaded } = usePermissions();
  const [cards, setCards] = useState<NaryCard[]>([]);
  const load = useCallback(async () => {
    try { setCards((await naryApi.cards({ status: "pending", entity_type: entityType, entity_id: entityId })).data); }
    catch { setCards([]); }
  }, [entityType, entityId]);
  useEffect(() => { if (loaded && can("nary:view")) load(); }, [load, loaded, can]);
  if (!cards.length) return null;
  return (
    <section className="space-y-2">
      <h2 className="inline-flex items-center gap-1.5 text-sm font-semibold text-fg">
        <Bot className="size-4 text-tone-violet" aria-hidden /> Nary is asking
      </h2>
      {cards.map((c) => <CardView key={c.id} card={c} onDone={() => load()} />)}
    </section>
  );
}
