import React from "react";
import { Loader2 } from "lucide-react";
import { cn } from "@/lib/utils/cn";

interface IconButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  /** Required: icon-only controls have no text, so they must name themselves. */
  "aria-label": string;
  variant?: "default" | "accent" | "danger";
  /** danger only: a second, armed state (text + tint) for confirm affordances. */
  armed?: boolean;
  loading?: boolean;
}

/**
 * The one canonical icon button: a square 32px hit box around a single 16px
 * glyph. No borders. One hover rule (ink dims up a rung / picks up a ghost
 * fill), one focus recipe (the shared ring), an active press, and the sanctioned
 * disabled + loading states. `danger` + `armed` renders the confirm tone; pair
 * it with useArmedConfirm at the call site.
 */
export const IconButton: React.FC<IconButtonProps> = ({
  variant = "default",
  armed = false,
  loading = false,
  disabled,
  className,
  children,
  type = "button",
  ...props
}) => {
  const variantClass = {
    default: "text-ink-muted hover:text-ink hover:bg-control-ghost-hover",
    accent: "text-accent hover:opacity-70",
    danger: armed
      ? "text-danger bg-danger-tint"
      : "text-ink-muted hover:text-danger",
  }[variant];

  return (
    <button
      type={type}
      disabled={disabled}
      aria-busy={loading || undefined}
      className={cn(
        "min-h-8 min-w-8 inline-grid place-items-center rounded-control cursor-pointer transition-colors focus-ring active:opacity-90",
        variantClass,
        loading && "pointer-events-none",
        disabled && "opacity-60 cursor-not-allowed",
        className,
      )}
      {...props}
    >
      {loading ? (
        <Loader2 className="size-4 animate-spin" aria-hidden="true" />
      ) : (
        children
      )}
    </button>
  );
};
