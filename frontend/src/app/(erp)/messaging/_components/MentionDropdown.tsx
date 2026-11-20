"use client";

import { useEffect, useRef, useState } from "react";
import { messagingApi } from "@/lib/api";
import { User, Briefcase, FileText, Package, TrendingUp } from "lucide-react";

export interface MentionItem {
  type: "user" | "project" | "deal" | "sop" | "product";
  id: string;
  name: string;
  sub?: string;
  avatar_url?: string;
}

const ENTITY_TYPES = [
  { key: "project", label: "Projects", icon: Briefcase, color: "text-tone-blue" },
  { key: "deal", label: "Deals", icon: TrendingUp, color: "text-tone-violet" },
  { key: "sop", label: "SOPs / KB", icon: FileText, color: "text-tone-orange" },
  { key: "product", label: "Products", icon: Package, color: "text-tone-neutral" },
] as const;

interface Props {
  trigger: "@" | "#" | null;
  query: string;
  onSelect: (item: MentionItem) => void;
  onClose: () => void;
}

export default function MentionDropdown({ trigger, query, onSelect, onClose }: Props) {
  const [items, setItems] = useState<MentionItem[]>([]);
  const [entityType, setEntityType] = useState<string | null>(null);
  const [activeIndex, setActiveIndex] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setActiveIndex(0);
    if (!trigger) return;

    if (trigger === "@") {
      messagingApi.mentionUsers(query).then((r) => {
        setItems((r.data as { id: string; name: string; avatar_url?: string; position?: string }[]).map((u) => ({
          type: "user" as const,
          id: u.id,
          name: u.name,
          sub: u.position,
          avatar_url: u.avatar_url,
        })));
      }).catch(() => setItems([]));
    } else if (trigger === "#" && entityType) {
      messagingApi.mentionEntities(query, entityType).then((r) => {
        setItems((r.data as MentionItem[]).map((e) => ({ ...e })));
      }).catch(() => setItems([]));
    }
  }, [trigger, query, entityType]);

  // Handle keyboard navigation
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (!trigger) return;
      if (e.key === "ArrowDown") { e.preventDefault(); setActiveIndex((i) => Math.min(i + 1, items.length - 1)); }
      if (e.key === "ArrowUp") { e.preventDefault(); setActiveIndex((i) => Math.max(i - 1, 0)); }
      if (e.key === "Enter" && items[activeIndex]) { e.preventDefault(); onSelect(items[activeIndex]); }
      if (e.key === "Escape") { onClose(); }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [trigger, items, activeIndex, onSelect, onClose]);

  if (!trigger) return null;

  if (trigger === "#" && !entityType) {
    return (
      <div role="listbox" aria-label="Mention entity type" className="absolute bottom-full left-0 z-50 mb-2 w-56 overflow-hidden rounded-lg border border-line bg-surface-raised p-1 shadow-pop">
        <p className="px-2.5 pb-1 pt-1.5 text-xs font-medium text-fg-muted">Mention entity type</p>
        {ENTITY_TYPES.map(({ key, label, icon: Icon, color }) => (
          <button
            key={key}
            type="button"
            onClick={() => setEntityType(key)}
            className="flex min-h-11 w-full items-center gap-3 rounded-md px-2.5 text-left transition-colors hover:bg-surface-2 md:min-h-9"
          >
            <Icon className={`size-4 ${color}`} aria-hidden />
            <span className="text-sm text-fg">{label}</span>
          </button>
        ))}
      </div>
    );
  }

  if (items.length === 0) return null;

  return (
    <div
      ref={listRef}
      role="listbox"
      aria-label={trigger === "@" ? "Mention user" : `Mention ${entityType}`}
      className="absolute bottom-full left-0 z-50 mb-2 max-h-56 w-64 overflow-hidden overflow-y-auto rounded-lg border border-line bg-surface-raised p-1 shadow-pop"
    >
      <p className="px-2.5 pb-1 pt-1.5 text-xs font-medium text-fg-muted">
        {trigger === "@" ? "Mention user" : `Mention ${entityType}`}
      </p>
      {items.map((item, i) => {
        const isActive = i === activeIndex;
        const TypeIcon = trigger === "@" ? User : ENTITY_TYPES.find((t) => t.key === entityType)?.icon ?? FileText;
        const color = trigger === "@" ? "text-fg-muted" : ENTITY_TYPES.find((t) => t.key === entityType)?.color ?? "text-fg-muted";
        return (
          <button
            key={item.id}
            type="button"
            role="option"
            aria-selected={isActive}
            onClick={() => onSelect(item)}
            className={`flex min-h-11 w-full items-center gap-3 rounded-md px-2.5 py-1.5 text-left transition-colors md:min-h-9 ${isActive ? "bg-surface-2" : "hover:bg-surface-2"}`}
          >
            <TypeIcon className={`size-4 shrink-0 ${color}`} aria-hidden />
            <div className="min-w-0">
              <p className="truncate text-sm font-medium text-fg">{item.name}</p>
              {item.sub && <p className="truncate text-xs text-fg-muted">{item.sub}</p>}
            </div>
          </button>
        );
      })}
    </div>
  );
}
