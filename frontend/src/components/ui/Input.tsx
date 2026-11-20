"use client";

import { forwardRef } from "react";
import { Check, ChevronDown } from "lucide-react";
import * as SwitchPrimitive from "@radix-ui/react-switch";
import { cn } from "@/lib/utils";
import { useFieldControl } from "./Field";

/** Shared look of every text-like control. */
export const controlClass =
  // 16 px on phones so iOS does not zoom into a focused field
  "w-full rounded-lg border border-line-strong bg-surface px-3 text-base text-fg md:text-sm " +
  "placeholder:text-fg-subtle transition-colors " +
  "hover:border-fg-subtle/60 focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/20 " +
  "disabled:cursor-not-allowed disabled:bg-surface-2 disabled:text-fg-muted " +
  "aria-[invalid=true]:border-tone-red aria-[invalid=true]:focus:ring-tone-red/20";

export interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  /** Right-aligned tabular figures, for amounts and quantities. */
  numeric?: boolean;
  /** An icon inside on the left (search). */
  leading?: React.ElementType;
  /** Text or a node inside on the right (a unit like "%" or "USD"). */
  trailing?: React.ReactNode;
}

export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { className, numeric, leading: Leading, trailing, type = "text", ...props },
  ref,
) {
  const ctl = useFieldControl(props);
  const input = (
    <input ref={ref} type={type} {...props} {...ctl}
      className={cn(controlClass, "h-11 md:h-9", numeric && "text-right tabular no-spinner",
        Leading && "pl-9", trailing != null && "pr-12", className)} />
  );
  if (!Leading && trailing == null) return input;
  return (
    <div className="relative">
      {Leading && <Leading aria-hidden className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-fg-subtle" />}
      {input}
      {trailing != null && (
        <span className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-xs text-fg-muted">{trailing}</span>
      )}
    </div>
  );
});

export const Textarea = forwardRef<HTMLTextAreaElement, React.TextareaHTMLAttributes<HTMLTextAreaElement>>(
  function Textarea({ className, rows = 3, ...props }, ref) {
    const ctl = useFieldControl(props);
    return <textarea ref={ref} rows={rows} {...props} {...ctl} className={cn(controlClass, "min-h-[72px] py-2 leading-relaxed", className)} />;
  },
);

export interface SelectProps extends React.SelectHTMLAttributes<HTMLSelectElement> {
  /** Shorthand for simple lists; children still work for groups. */
  options?: { value: string; label: string; disabled?: boolean }[];
  placeholder?: string;
}

/** A native select (keyboard, mobile pickers and forms work as the browser does), styled. */
export const Select = forwardRef<HTMLSelectElement, SelectProps>(function Select(
  { className, options, placeholder, children, ...props },
  ref,
) {
  const ctl = useFieldControl(props);
  return (
    <div className="relative">
      <select ref={ref} {...props} {...ctl} className={cn(controlClass, "h-11 cursor-pointer appearance-none pr-9 md:h-9", className)}>
        {placeholder != null && <option value="">{placeholder}</option>}
        {options?.map((o) => <option key={o.value} value={o.value} disabled={o.disabled}>{o.label}</option>)}
        {children}
      </select>
      <ChevronDown aria-hidden className="pointer-events-none absolute right-3 top-1/2 size-4 -translate-y-1/2 text-fg-subtle" />
    </div>
  );
});

export interface CheckboxProps extends Omit<React.InputHTMLAttributes<HTMLInputElement>, "type"> {
  label?: React.ReactNode;
  hint?: React.ReactNode;
}

/** A native checkbox with an optional label to its right; the whole row is the hit area. */
export const Checkbox = forwardRef<HTMLInputElement, CheckboxProps>(function Checkbox(
  { label, hint, className, ...props },
  ref,
) {
  const ctl = useFieldControl(props);
  const box = (
    // Without a label, className (margins, opacity) goes on this 16px holder, never on the box itself: on the
    // input, margins squeezed it into a thin bar (the inbox row checkboxes, 24 Oct).
    <span className={cn("relative inline-flex size-4 shrink-0", !label && className)}>
      <input ref={ref} type="checkbox" {...props} {...ctl}
        className={"peer size-4 shrink-0 cursor-pointer appearance-none rounded border border-line-strong bg-surface transition-colors " +
          "checked:border-brand checked:bg-brand focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 " +
          "focus-visible:outline-[rgb(var(--focus)/0.7)] disabled:cursor-not-allowed disabled:opacity-50 " +
          "indeterminate:border-brand indeterminate:bg-brand"} />
      <Check aria-hidden strokeWidth={3} className="pointer-events-none absolute inset-0.5 size-3 text-brand-on opacity-0 peer-checked:opacity-100" />
    </span>
  );
  if (!label) return box;
  return (
    <label className={cn("inline-flex min-h-11 cursor-pointer items-start gap-2.5 py-1 text-sm text-fg md:min-h-0", className)}>
      <span className="mt-0.5">{box}</span>
      <span className="flex flex-col gap-0.5">
        <span>{label}</span>
        {hint && <span className="text-xs text-fg-muted">{hint}</span>}
      </span>
    </label>
  );
});

export interface SwitchProps extends React.ComponentPropsWithoutRef<typeof SwitchPrimitive.Root> {
  label?: React.ReactNode;
}

/** On/off that applies at once (Radix switch, role="switch"). */
export const Switch = forwardRef<HTMLButtonElement, SwitchProps>(function Switch({ label, className, ...props }, ref) {
  const ctl = useFieldControl(props as { id?: string });
  const control = (
    <SwitchPrimitive.Root ref={ref} {...props} {...ctl}
      className={cn("peer inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full border-2 border-transparent transition-colors " +
        "bg-line-strong data-[state=checked]:bg-brand disabled:cursor-not-allowed disabled:opacity-50 " +
        "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[rgb(var(--focus)/0.7)]",
        !label && className)}>
      <SwitchPrimitive.Thumb className="pointer-events-none block size-4 rounded-full bg-white shadow-sm transition-transform data-[state=checked]:translate-x-4 data-[state=unchecked]:translate-x-0" />
    </SwitchPrimitive.Root>
  );
  if (!label) return control;
  return (
    <label className={cn("inline-flex min-h-11 cursor-pointer items-center gap-2.5 text-sm text-fg md:min-h-0", className)}>
      {control}
      <span>{label}</span>
    </label>
  );
});
