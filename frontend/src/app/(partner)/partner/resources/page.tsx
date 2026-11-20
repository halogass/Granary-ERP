"use client";

import { useEffect, useState, useCallback } from "react";
import { portalPartnerApi } from "@/lib/api";
import { BookOpen, Download, FileText, Image, Video, Link2 } from "lucide-react";
import { Button, Card, EmptyState, FilterChips, ListFrame, PageHeader, Skeleton } from "@/components/ui";

interface Resource {
  id: string;
  title: string;
  description?: string;
  resource_type: string;
  file_url?: string;
  external_url?: string;
  tier_required?: string;
  created_at: string;
}

const TYPE_ICON: Record<string, React.ElementType> = {
  brochure: FileText, presentation: FileText, logo: Image, video: Video, guide: BookOpen, link: Link2, other: FileText,
};

export default function PartnerResourcesPage() {
  const [resources, setResources] = useState<Resource[]>([]);
  const [loading, setLoading] = useState(true);
  const [filterType, setFilterType] = useState("all");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await portalPartnerApi.listResources();
      setResources(res.data.items ?? res.data ?? []);
    } catch { setResources([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const filtered = filterType === "all" ? resources : resources.filter((r) => r.resource_type === filterType);
  const types = [...new Set(resources.map((r) => r.resource_type))];

  return (
    <div className="space-y-4">
      <PageHeader title="Partner resources" summary="Sales kits, brand assets, guides and presentations." />

      {types.length > 0 && (
        <ListFrame>
          <FilterChips className="border-b-0"
            chips={types.map((t) => ({ id: t, label: t.charAt(0).toUpperCase() + t.slice(1), active: filterType === t }))}
            onToggle={(id) => setFilterType((cur) => (cur === id ? "all" : id))}
            onClear={() => setFilterType("all")} />
        </ListFrame>
      )}

      {loading ? (
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2" aria-busy="true">
          {[0, 1, 2, 3].map((i) => <Skeleton key={i} className="h-24" />)}
        </div>
      ) : filtered.length === 0 ? (
        <Card><EmptyState icon={BookOpen} title="No resources available" description="We add sales kits and guides here as they are published." /></Card>
      ) : (
        <ul className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          {filtered.map((resource) => {
            const Icon = TYPE_ICON[resource.resource_type] ?? FileText;
            const href = resource.file_url ?? resource.external_url;
            return (
              <li key={resource.id}>
                <Card className="flex h-full items-start gap-3 p-4">
                  <span className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-surface-2 text-fg-muted">
                    <Icon className="size-5" aria-hidden />
                  </span>
                  <div className="min-w-0 flex-1">
                    <p className="text-sm font-semibold text-fg">{resource.title}</p>
                    {resource.description && <p className="mt-1 line-clamp-2 text-[13px] text-fg-muted">{resource.description}</p>}
                    <p className="mt-2 text-xs capitalize text-fg-muted">
                      {resource.resource_type}
                      {resource.tier_required && <> · {resource.tier_required}+</>}
                    </p>
                  </div>
                  {href && (
                    <Button size="sm" asChild>
                      <a href={href} target="_blank" rel="noopener noreferrer" aria-label={`Open ${resource.title}`}>
                        <Download aria-hidden /> Open
                      </a>
                    </Button>
                  )}
                </Card>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
