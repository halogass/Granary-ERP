"use client";

import { createContext, forwardRef, useContext, useId } from "react";
import * as LabelPrimitive from "@radix-ui/react-label";
import { cn } from "@/lib/utils";

interface FieldCtx { id: string; hintId?: string; errorId?: string; invalid: boolean; required?: boolean }
const FieldContext = createContext<FieldCtx | null>(null);

/**
 * The props a control inside a <Field> needs (id, aria-describedby, aria-invalid).
 * Input, Textarea, Select and Checkbox call it, so they wire themselves up.
 */
export function useFieldControl(props: { id?: string; "aria-describedby"?: string; "aria-invalid"?: React.AriaAttributes["aria-invalid"]; required?: boolean }) {
  const ctx = useContext(FieldContext);
  if (!ctx) return props;
  const describedBy = [props["aria-describedby"], ctx.errorId, ctx.hintId].filter(Boolean).join(" ") || undefined;
  return {
    id: props.id ?? ctx.id,
    "aria-describedby": describedBy,
    "aria-invalid": props["aria-invalid"] ?? (ctx.invalid || undefined),
    required: props.required ?? ctx.required,
  };
}

export const Label = forwardRef<HTMLLabelElement, React.ComponentPropsWithoutRef<typeof LabelPrimitive.Root>>(
  function Label({ className, ...props }, ref) {
    return <LabelPrimitive.Root ref={ref} className={cn("text-[13px] font-medium text-fg", className)} {...props} />;
  },
);

export interface FieldProps {
  label: React.ReactNode;
  /** One line under the control, in plain language. */
  hint?: React.ReactNode;
  /** Shown in red instead of the hint; marks the control invalid. */
  error?: React.ReactNode;
  required?: boolean;
  /** Something on the label row's right (a "changed" mark, a link). */
  aside?: React.ReactNode;
  id?: string;
  className?: string;
  children: React.ReactNode;
}

/** Label + control + hint/error, stacked. Put any Input/Textarea/Select/Combobox inside. */
export function Field({ label, hint, error, required, aside, id, className, children }: FieldProps) {
  const auto = useId();
  const fieldId = id ?? `f${auto.replace(/:/g, "")}`;
  const hintId = hint ? `${fieldId}-hint` : undefined;
  const errorId = error ? `${fieldId}-error` : undefined;
  return (
    <FieldContext.Provider value={{ id: fieldId, hintId, errorId, invalid: !!error, required }}>
      <div className={cn("flex min-w-0 flex-col gap-1.5", className)}>
        <div className="flex items-baseline justify-between gap-2">
          <Label htmlFor={fieldId}>
            {label}
            {required && <span className="ml-0.5 text-brand-ink" aria-hidden>*</span>}
          </Label>
          {aside}
        </div>
        {children}
        {error ? (
          <p id={errorId} className="text-xs text-tone-red">{error}</p>
        ) : hint ? (
          <p id={hintId} className="text-xs text-fg-muted">{hint}</p>
        ) : null}
      </div>
    </FieldContext.Provider>
  );
}

/** A responsive grid for forms: one column on phones, `cols` from md up. */
export function FieldGrid({ cols = 2, className, children }: { cols?: 1 | 2 | 3 | 4; className?: string; children: React.ReactNode }) {
  const map = { 1: "md:grid-cols-1", 2: "md:grid-cols-2", 3: "md:grid-cols-3", 4: "md:grid-cols-4" };
  return <div className={cn("grid grid-cols-1 gap-x-4 gap-y-4", map[cols], className)}>{children}</div>;
}
