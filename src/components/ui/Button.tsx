import React from "react";
import { Loader2 } from "lucide-react";
import { cn } from "@/lib/utils/cn";

interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?:
    | "primary"
    | "primary-soft"
    | "secondary"
    | "danger"
    | "danger-ghost"
    | "ghost";
  size?: "sm" | "md" | "lg";
  loading?: boolean;
}

/**
 * The primary action control. Four looks (primary / secondary / ghost / danger);
 * the two legacy names are kept as aliases so existing call sites compile, but
 * render with the new styles. No borders, no shadow. One hover rule per variant
 * (solid fills step a rung, ghost picks up a ghost fill), an active press, the
 * shared focus ring, and disabled + loading states. Heights: sm and md 36px, lg 48px.
 */
export const Button: React.FC<ButtonProps> = ({
  children,
  className,
  variant = "primary",
  size = "md",
  loading = false,
  disabled,
  ...props
}) => {
  const base =
    "inline-flex items-center justify-center font-medium rounded-control cursor-pointer transition-colors focus-ring disabled:opacity-60 disabled:cursor-not-allowed";

  const variantClasses: Record<NonNullable<ButtonProps["variant"]>, string> = {
    primary:
      "bg-accent-solid text-on-accent hover:bg-accent-solid-hover active:bg-accent-solid-active",
    // Legacy alias: renders as primary.
    "primary-soft":
      "bg-accent-solid text-on-accent hover:bg-accent-solid-hover active:bg-accent-solid-active",
    secondary:
      "bg-control-secondary text-ink hover:bg-control-secondary-hover active:bg-control-secondary-hover",
    ghost: "bg-transparent text-ink hover:bg-control-ghost-hover",
    // Legacy alias: renders as ghost.
    "danger-ghost": "bg-transparent text-ink hover:bg-control-ghost-hover",
    danger: "bg-danger-solid text-on-accent hover:opacity-90 active:opacity-90",
  };

  const sizeClasses: Record<NonNullable<ButtonProps["size"]>, string> = {
    sm: "h-9 px-3 text-sm gap-2",
    md: "h-9 px-4 text-sm gap-2",
    lg: "h-12 px-5 text-base gap-2",
  };

  return (
    <button
      disabled={disabled}
      aria-busy={loading || undefined}
      className={cn(
        base,
        variantClasses[variant],
        sizeClasses[size],
        loading && "pointer-events-none",
        className,
      )}
      {...props}
    >
      {loading && (
        <Loader2 className="size-4 animate-spin" aria-hidden="true" />
      )}
      {children}
    </button>
  );
};
