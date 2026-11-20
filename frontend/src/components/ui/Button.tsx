"use client";

import { forwardRef } from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import { Loader2 } from "lucide-react";
import { cn } from "@/lib/utils";

export const buttonVariants = cva(
  "inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-lg text-sm font-medium " +
    "transition-colors select-none disabled:pointer-events-none disabled:opacity-50 " +
    "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[rgb(var(--focus)/0.7)] " +
    "[&_svg]:size-4 [&_svg]:shrink-0",
  {
    variants: {
      variant: {
        primary: "bg-brand text-brand-on hover:bg-brand-hover",
        secondary: "border border-line-strong bg-surface text-fg hover:bg-surface-2",
        ghost: "text-fg hover:bg-surface-2",
        // Red is the primary color, so a destructive action is outlined, never a second red fill.
        danger: "border border-tone-red/40 bg-surface text-tone-red hover:bg-tone-red-soft",
      },
      // 44 px on touch screens, denser from md up
      size: {
        sm: "h-11 px-3 text-[13px] md:h-7 md:px-2.5",
        md: "h-11 px-4 md:h-8 md:px-3",
        lg: "h-11 px-5 md:h-10 md:px-4",
      },
    },
    defaultVariants: { variant: "secondary", size: "md" },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  /** Render the child (a Link, an <a>) with button styling. */
  asChild?: boolean;
  /** Shows a spinner and disables the button. */
  loading?: boolean;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { className, variant, size, asChild, loading, disabled, children, type, ...props },
  ref,
) {
  const classes = cn(buttonVariants({ variant, size }), className);
  if (asChild) {
    return <Slot ref={ref} className={classes} {...props}>{children}</Slot>;
  }
  return (
    <button ref={ref} type={type ?? "button"} className={classes} disabled={disabled || loading}
      aria-busy={loading || undefined} {...props}>
      {loading && <Loader2 className="animate-spin" aria-hidden />}
      {children}
    </button>
  );
});

const iconSizes = {
  sm: "size-11 md:size-7",
  md: "size-11 md:size-8",
  lg: "size-11 md:size-10",
};

export interface IconButtonProps extends Omit<ButtonProps, "asChild" | "children"> {
  /** Required: the accessible name (also used as the tooltip title). */
  label: string;
  icon: React.ElementType;
  /** A count shown on the corner (notifications, needs-you). */
  badge?: number;
}

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { label, icon: Icon, badge, className, size = "md", variant = "ghost", ...props },
  ref,
) {
  return (
    <Button ref={ref} variant={variant} size={size} aria-label={label} title={label}
      className={cn("relative px-0", iconSizes[size ?? "md"], className)} {...props}>
      <Icon aria-hidden />
      {!!badge && badge > 0 && (
        <span className="absolute -right-0.5 -top-0.5 flex h-4 min-w-4 items-center justify-center rounded-full bg-brand px-1 text-[10px] font-semibold leading-none text-brand-on tabular">
          {badge > 99 ? "99+" : badge}
        </span>
      )}
    </Button>
  );
});
