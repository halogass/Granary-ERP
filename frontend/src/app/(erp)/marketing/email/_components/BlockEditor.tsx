"use client";

import { useState } from "react";
import {
  Plus, Trash2, GripVertical, ChevronUp, ChevronDown,
  Image, Type, AlignLeft, MousePointer, Minus, Space, Code,
} from "lucide-react";
import TokenInput from "./TokenInput";
import { Button, Field, IconButton, Input, Select, Textarea, controlClass } from "@/components/ui";

export type Block =
  | { type: "header_image"; url: string; alt: string }
  | { type: "heading"; level: 1 | 2; text: string }
  | { type: "paragraph"; text: string }
  | { type: "cta_button"; label: string; button_url: string; button_color: string }
  | { type: "divider" }
  | { type: "spacer"; height: number }
  | { type: "image"; url: string; alt: string }
  | { type: "code"; text: string };

const BLOCK_TYPES: { type: Block["type"]; label: string; icon: React.ElementType }[] = [
  { type: "header_image", label: "Header Image", icon: Image },
  { type: "heading",      label: "Heading",      icon: Type },
  { type: "paragraph",   label: "Paragraph",    icon: AlignLeft },
  { type: "cta_button",  label: "CTA Button",   icon: MousePointer },
  { type: "image",       label: "Image",        icon: Image },
  { type: "divider",     label: "Divider",      icon: Minus },
  { type: "spacer",      label: "Spacer",       icon: Space },
  { type: "code",        label: "Code",         icon: Code },
];

function defaultBlock(type: Block["type"]): Block {
  switch (type) {
    case "header_image": return { type, url: "", alt: "" };
    case "heading":      return { type, level: 1, text: "Your Heading" };
    case "paragraph":    return { type, text: "Hi {{first_name}},\n\nYour content here." };
    // the email button's own color (content of the email, the brand colour by default)
    case "cta_button":   return { type, label: "Click Here", button_url: "https://", button_color: "#1f4e79" };
    case "image":        return { type, url: "", alt: "" };
    case "divider":      return { type };
    case "spacer":       return { type, height: 20 };
    case "code":         return { type, text: "" };
  }
}

interface BlockEditorProps {
  blocks: Block[];
  onChange: (blocks: Block[]) => void;
}

export default function BlockEditor({ blocks, onChange }: BlockEditorProps) {
  const [showAdd, setShowAdd] = useState(false);

  function addBlock(type: Block["type"]) {
    onChange([...blocks, defaultBlock(type)]);
    setShowAdd(false);
  }

  function remove(i: number) {
    onChange(blocks.filter((_, idx) => idx !== i));
  }

  function move(i: number, dir: -1 | 1) {
    const next = [...blocks];
    [next[i], next[i + dir]] = [next[i + dir], next[i]];
    onChange(next);
  }

  function update(i: number, patch: Partial<Block>) {
    const next = [...blocks];
    next[i] = { ...next[i], ...patch } as Block;
    onChange(next);
  }

  return (
    <div className="space-y-3">
      {blocks.map((block, i) => {
        const label = BLOCK_TYPES.find((b) => b.type === block.type)?.label ?? block.type;
        return (
          <div key={i} className="overflow-hidden rounded-lg border border-line bg-surface">
            <div className="flex items-center gap-1 border-b border-line bg-surface-2 px-3 py-1">
              <GripVertical className="size-4 shrink-0 text-fg-subtle" aria-hidden />
              <span className="min-w-0 flex-1 truncate text-xs font-medium text-fg-muted">{label}</span>
              <IconButton size="sm" icon={ChevronUp} label={`Move ${label} up`} onClick={() => move(i, -1)} disabled={i === 0} />
              <IconButton size="sm" icon={ChevronDown} label={`Move ${label} down`} onClick={() => move(i, 1)} disabled={i === blocks.length - 1} />
              <IconButton size="sm" icon={Trash2} label={`Remove ${label}`} onClick={() => remove(i)} className="text-tone-red" />
            </div>
            <div className="space-y-3 p-4">
              <BlockFields block={block} onChange={(patch) => update(i, patch)} />
            </div>
          </div>
        );
      })}

      {showAdd ? (
        <div className="rounded-lg border border-dashed border-line-strong p-4">
          <p className="mb-3 text-xs font-medium text-fg-muted">Choose a block type</p>
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
            {BLOCK_TYPES.map(({ type, label, icon: Icon }) => (
              <button key={type} type="button" onClick={() => addBlock(type)}
                className="flex flex-col items-center gap-1.5 rounded-lg border border-line p-3 text-center transition-colors hover:border-brand hover:bg-brand/[0.04]">
                <Icon className="size-5 text-fg-muted" aria-hidden />
                <span className="text-[11px] leading-tight text-fg">{label}</span>
              </button>
            ))}
          </div>
          <Button size="sm" variant="ghost" className="mt-3" onClick={() => setShowAdd(false)}>Cancel</Button>
        </div>
      ) : (
        <button type="button" onClick={() => setShowAdd(true)}
          className="flex min-h-11 w-full items-center justify-center gap-2 rounded-lg border border-dashed border-line-strong text-sm text-fg-muted transition-colors hover:border-brand hover:text-brand-ink">
          <Plus className="size-4" aria-hidden /> Add block
        </button>
      )}
    </div>
  );
}

const tokenCls = `${controlClass} h-11 md:h-9`;
const tokenAreaCls = `${controlClass} min-h-[72px] py-2 leading-relaxed`;

function BlockFields({ block, onChange }: { block: Block; onChange: (patch: Partial<Block>) => void }) {
  if (block.type === "header_image" || block.type === "image") {
    return (
      <>
        <Field label="Image URL">
          <Input value={block.url} placeholder="https://..." onChange={(e) => onChange({ url: e.target.value } as Partial<Block>)} />
        </Field>
        <Field label="Alt text">
          <Input value={block.alt} onChange={(e) => onChange({ alt: e.target.value } as Partial<Block>)} />
        </Field>
      </>
    );
  }

  if (block.type === "heading") {
    return (
      <div className="flex gap-3">
        <div className="min-w-0 flex-1">
          <Field label="Text">
            <TokenInput className={tokenCls} value={block.text} onChange={(text) => onChange({ text } as Partial<Block>)} />
          </Field>
        </div>
        <Field label="Level" className="w-24">
          <Select value={block.level} onChange={(e) => onChange({ level: Number(e.target.value) as 1 | 2 } as Partial<Block>)}
            options={[{ value: "1", label: "H1" }, { value: "2", label: "H2" }]} />
        </Field>
      </div>
    );
  }

  if (block.type === "paragraph") {
    return (
      <Field label="Text">
        <TokenInput multiline rows={4} className={tokenAreaCls} value={block.text} onChange={(text) => onChange({ text } as Partial<Block>)} />
      </Field>
    );
  }

  if (block.type === "cta_button") {
    return (
      <>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <Field label="Button label">
            <TokenInput className={tokenCls} value={block.label} onChange={(label) => onChange({ label } as Partial<Block>)} />
          </Field>
          <Field label="Button color">
            <div className="flex items-center gap-2">
              <input type="color" value={block.button_color} aria-label="Pick button color"
                onChange={(e) => onChange({ button_color: e.target.value } as Partial<Block>)}
                className="h-11 w-12 shrink-0 cursor-pointer rounded-lg md:h-9 border border-line-strong bg-surface p-0.5" />
              <Input className="font-mono" value={block.button_color} onChange={(e) => onChange({ button_color: e.target.value } as Partial<Block>)} />
            </div>
          </Field>
        </div>
        <Field label="Button URL">
          <TokenInput className={tokenCls} value={block.button_url} placeholder="https://..." onChange={(button_url) => onChange({ button_url } as Partial<Block>)} />
        </Field>
      </>
    );
  }

  if (block.type === "spacer") {
    return (
      <Field label="Height (px, 4 to 120)">
        <Input numeric type="number" value={block.height} min={4} max={120} onChange={(e) => onChange({ height: Number(e.target.value) } as Partial<Block>)} />
      </Field>
    );
  }

  if (block.type === "code") {
    return (
      <Field label="Code content">
        <Textarea className="font-mono text-xs" rows={5} value={block.text} onChange={(e) => onChange({ text: e.target.value } as Partial<Block>)} />
      </Field>
    );
  }

  // divider: no fields
  return <p className="text-xs italic text-fg-muted">Horizontal divider line, no settings.</p>;
}
