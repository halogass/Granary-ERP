"use client";

import { useEffect, useState, useCallback, useMemo } from "react";
import { localSource } from "@/lib/entitySources";
import { kbApi } from "@/lib/api";
import { Tag, Plus, BookOpen, Hash } from "lucide-react";
import {
  Button, Card, Combobox, Dialog, EmptyState, Field, Input, PageHeader, Skeleton, Stat, StatGroup, StatusPill, Textarea,
} from "@/components/ui";

interface Category {
  id: string;
  name: string;
  slug: string;
  description?: string;
  parent_id?: string;
  article_count?: number;
  /** The API has no active flag for categories (deleted ones are not listed): absent means active. */
  is_active?: boolean;
  sort_order?: number;
  created_at: string;
}

export default function KBCategoriesPage() {
  const [categories, setCategories] = useState<Category[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [saving, setSaving] = useState(false);
  const [form, setForm] = useState({ name: "", slug: "", description: "", parent_id: "", sort_order: "0" });

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await kbApi.listCategories();
      setCategories(res.data.items ?? res.data ?? []);
    } catch { setCategories([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  function toSlug(name: string) {
    return name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/(^-|-$)/g, "");
  }

  function handleNameChange(name: string) {
    setForm({ ...form, name, slug: toSlug(name) });
  }

  async function create() {
    if (!form.name.trim()) return;
    setSaving(true);
    try {
      await kbApi.createCategory({
        name: form.name.trim(),
        slug: form.slug.trim() || toSlug(form.name),
        description: form.description.trim() || undefined,
        parent_id: form.parent_id || undefined,
        sort_order: parseInt(form.sort_order) || 0,
      });
      setShowModal(false);
      setForm({ name: "", slug: "", description: "", parent_id: "", sort_order: "0" });
      await load();
    } finally { setSaving(false); }
  }

  const topLevel = categories.filter((c) => !c.parent_id);
  const parentOptions = useMemo(
    () => localSource(topLevel.map((c) => ({ id: c.id, label: c.name }))),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [categories],
  );
  const byParent: Record<string, Category[]> = {};
  categories.forEach((c) => {
    if (c.parent_id) {
      if (!byParent[c.parent_id]) byParent[c.parent_id] = [];
      byParent[c.parent_id].push(c);
    }
  });

  const subCount = categories.filter((c) => !!c.parent_id).length;

  return (
    <div className="max-w-5xl">
      <PageHeader title="KB categories" summary={`${categories.length} categories, ${topLevel.length} top level, ${subCount} sub-categories`}
        actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> New category</Button>} />

      <StatGroup className="mb-4">
        <Stat label="Total categories" value={categories.length} />
        <Stat label="Top level" value={topLevel.length} />
        <Stat label="Sub-categories" value={subCount} />
      </StatGroup>

      <Card className="overflow-hidden">
        {loading ? (
          <div className="space-y-2 p-4">{[0, 1, 2].map((i) => <Skeleton key={i} className="h-10 w-full" />)}</div>
        ) : categories.length === 0 ? (
          <EmptyState icon={Tag} title="No categories yet" description="Create categories to organise your knowledge base."
            action={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> New category</Button>} />
        ) : (
          <ul>
            {topLevel
              .sort((a, b) => (a.sort_order ?? 0) - (b.sort_order ?? 0))
              .map((cat) => {
                const children = byParent[cat.id] ?? [];
                return (
                  <li key={cat.id} className="border-b border-line-soft last:border-0">
                    <div className="flex items-center gap-3 px-4 py-2.5 hover:bg-surface-2">
                      <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-surface-2">
                        <BookOpen className="size-4 text-brand-ink" aria-hidden />
                      </span>
                      <div className="min-w-0 flex-1">
                        <div className="flex flex-wrap items-center gap-2">
                          <p className="text-sm font-medium text-fg">{cat.name}</p>
                          <span className="font-mono text-xs text-fg-subtle">/{cat.slug}</span>
                          {cat.is_active === false && <StatusPill status="inactive" />}
                        </div>
                        {cat.description && <p className="mt-0.5 truncate text-xs text-fg-muted">{cat.description}</p>}
                      </div>
                      <div className="flex shrink-0 items-center gap-3 text-xs text-fg-muted tabular">
                        {children.length > 0 && <span>{children.length} sub</span>}
                        {cat.article_count !== undefined && <span className="font-medium text-fg">{cat.article_count} articles</span>}
                      </div>
                    </div>
                    {children.length > 0 && (
                      <ul>
                        {[...children].sort((a, b) => (a.sort_order ?? 0) - (b.sort_order ?? 0)).map((child) => (
                          <li key={child.id} className="flex items-center gap-3 border-t border-line-soft bg-surface-2/60 py-2 pl-12 pr-4 hover:bg-surface-2">
                            <Hash className="size-3.5 shrink-0 text-fg-subtle" aria-hidden />
                            <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
                              <p className="text-sm text-fg">{child.name}</p>
                              <span className="font-mono text-xs text-fg-subtle">/{child.slug}</span>
                            </div>
                            {child.article_count !== undefined && (
                              <span className="text-xs text-fg-muted tabular">{child.article_count} articles</span>
                            )}
                          </li>
                        ))}
                      </ul>
                    )}
                  </li>
                );
              })}
          </ul>
        )}
      </Card>

      <Dialog open={showModal} onOpenChange={setShowModal} title="New category"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} disabled={!form.name.trim()} loading={saving}>Create category</Button>
        </>}>
        <div className="space-y-4">
          <Field label="Name" required>
            <Input value={form.name} onChange={(e) => handleNameChange(e.target.value)} placeholder="e.g. Product Guides" autoComplete="off" />
          </Field>
          <Field label="Slug">
            <Input value={form.slug} onChange={(e) => setForm({ ...form, slug: e.target.value })} placeholder="auto-generated" className="font-mono" autoComplete="off" />
          </Field>
          <Field label="Parent category">
            <Combobox value={form.parent_id} source={parentOptions}
              placeholder="Top level" emptyLabel="No categories"
              onChange={(id) => setForm({ ...form, parent_id: id })} />
          </Field>
          <Field label="Description">
            <Textarea value={form.description} onChange={(e) => setForm({ ...form, description: e.target.value })} rows={2} />
          </Field>
          <Field label="Sort order">
            <Input type="number" numeric value={form.sort_order} onChange={(e) => setForm({ ...form, sort_order: e.target.value })} />
          </Field>
        </div>
      </Dialog>
    </div>
  );
}
