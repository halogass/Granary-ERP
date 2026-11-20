import { cn } from "@/lib/utils";

/** A small radio group drawn as buttons (approve/reject, a response, a mode). */
export default function ChoiceButtons<T extends string>({ value, onChange, options, label, className }: {
  value: T;
  onChange: (v: T) => void;
  options: { value: T; label: string }[];
  label: string;
  className?: string;
}) {
  return (
    <div role="radiogroup" aria-label={label} className={cn("flex flex-wrap gap-2", className)}>
      {options.map((o) => (
        <button key={o.value} type="button" role="radio" aria-checked={value === o.value} onClick={() => onChange(o.value)}
          className={cn("h-11 flex-1 rounded-lg border px-3.5 text-sm font-medium transition-colors md:h-9",
            value === o.value ? "border-brand bg-brand text-brand-on" : "border-line-strong bg-surface text-fg hover:bg-surface-2")}>
          {o.label}
        </button>
      ))}
    </div>
  );
}
