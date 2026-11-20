"use client";
// Shared saved-notes library: insert/save named note snippets across any document maker.
import { useEffect, useRef, useState } from "react";
import { useSession } from "next-auth/react";
import { noteTemplatesApi } from "@/lib/api";
import { sanitizeHtml } from "@/lib/sanitize";
import { ChevronDown, Save, Trash2, BookText } from "lucide-react";
import { Button, IconButton, Pill } from "@/components/ui";

interface Template {
  id: string; name: string; body: string; doc_type: string | null;
  created_by_id?: string | null;
  /** When the API says so explicitly, it wins over the local rule. */
  can_edit?: boolean;
}

interface SessionUser { id?: string; is_superadmin?: boolean; is_system_admin?: boolean }

/** Only the template's creator, the Owner (superadmin) and system admins may change or delete it. */
export function canManageTemplate(t: Template, user: SessionUser | null | undefined): boolean {
  if (typeof t.can_edit === "boolean") return t.can_edit;
  if (!user) return false;
  if (user.is_superadmin || user.is_system_admin) return true;
  return !!user.id && !!t.created_by_id && t.created_by_id === user.id;
}

export default function NoteTemplatePicker({ docType, currentValue, onInsert }: {
  docType: string;            // "quotation" | "invoice" | ...
  currentValue: string;       // current notes HTML (for "save as template")
  onInsert: (html: string) => void;
}) {
  const [templates, setTemplates] = useState<Template[]>([]);
  const [open, setOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const { data: session } = useSession();
  const user = session?.user as SessionUser | undefined;
  const boxRef = useRef<HTMLDivElement>(null);

  const load = () => noteTemplatesApi.list(docType).then((r) => setTemplates(r.data)).catch(() => {});
  useEffect(() => { load(); /* eslint-disable-next-line */ }, [docType]);

  useEffect(() => {
    const h = (e: MouseEvent) => { if (boxRef.current && !boxRef.current.contains(e.target as Node)) setOpen(false); };
    document.addEventListener("mousedown", h);
    return () => document.removeEventListener("mousedown", h);
  }, []);

  async function saveCurrent() {
    if (!currentValue || currentValue === "<br>") return;
    const name = window.prompt("Template name");
    if (!name?.trim()) return;
    setSaving(true);
    try {
      await noteTemplatesApi.create({ name: name.trim(), body: currentValue, doc_type: docType });
      await load();
    } finally { setSaving(false); }
  }

  async function remove(id: string, e: React.MouseEvent) {
    e.stopPropagation();
    if (!window.confirm("Delete this saved note?")) return;
    await noteTemplatesApi.remove(id).catch(() => {});
    load();
  }

  return (
    <div ref={boxRef} className="relative flex items-center gap-2">
      <Button size="sm" onClick={() => setOpen((o) => !o)} aria-expanded={open}>
        <BookText aria-hidden /> Insert saved note <ChevronDown aria-hidden />
      </Button>
      <Button size="sm" onClick={saveCurrent} loading={saving}>
        {!saving && <Save aria-hidden />} Save as template
      </Button>

      {open && (
        <div className="absolute left-0 top-full z-20 mt-1 max-h-72 w-72 overflow-y-auto rounded-lg border border-line bg-surface-raised py-1 shadow-pop">
          {templates.length === 0 ? (
            <p className="px-3 py-3 text-center text-xs text-fg-muted">No saved notes yet</p>
          ) : templates.map((t) => (
            <div key={t.id} className="group flex items-center gap-2 px-1 hover:bg-surface-2">
              <button type="button" onClick={() => { onInsert(sanitizeHtml(t.body)); setOpen(false); }}
                className="min-h-11 flex-1 truncate px-2 text-left text-[13px] font-medium text-fg md:min-h-8">
                {t.name}
              </button>
              {!t.doc_type && <Pill className="h-[18px] px-1.5 text-[10px]">shared</Pill>}
              {canManageTemplate(t, user) && (
                <IconButton icon={Trash2} size="sm" label={`Delete ${t.name}`} onClick={(e) => remove(t.id, e)}
                  className="text-fg-subtle hover:text-tone-red md:opacity-0 md:focus-visible:opacity-100 md:group-hover:opacity-100" />
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
