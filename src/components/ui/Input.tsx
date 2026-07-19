import React from "react";
import { cn } from "@/lib/utils/cn";

interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  variant?: "default" | "compact";
}

/**
 * A recessed well input: one tone, no border, no hover / focus fill swap. Focus
 * is the shared ring only. Both variants are 32px fields (`compact` is kept as
 * an alias for call-site compatibility).
 */
export const Input: React.FC<InputProps> = ({
  className,
  variant = "default",
  disabled,
  ...props
}) => {
  const sizeClasses = {
    default: "h-8 px-3",
    compact: "h-8 px-3",
  } as const;

  return (
    <input
      disabled={disabled}
      className={cn(
        "w-full text-sm bg-surface-well text-ink rounded-control transition-colors focus-ring placeholder:text-ink-subtle disabled:opacity-60 disabled:cursor-not-allowed",
        sizeClasses[variant],
        className,
      )}
      {...props}
    />
  );
};
