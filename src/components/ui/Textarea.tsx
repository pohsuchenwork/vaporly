import React from "react";
import { cn } from "@/lib/utils/cn";

interface TextareaProps extends React.TextareaHTMLAttributes<HTMLTextAreaElement> {
  variant?: "default" | "compact";
}

/**
 * A recessed well textarea: one tone, no border, no hover / focus fill swap.
 * Focus is the shared ring only.
 */
export const Textarea: React.FC<TextareaProps> = ({
  className,
  variant = "default",
  ...props
}) => {
  const sizeClasses = {
    default: "min-h-24",
    compact: "min-h-20",
  } as const;

  return (
    <textarea
      className={cn(
        "w-full px-3 py-2 text-sm bg-surface-well text-ink rounded-control transition-colors focus-ring placeholder:text-ink-subtle resize-y disabled:opacity-60 disabled:cursor-not-allowed",
        sizeClasses[variant],
        className,
      )}
      {...props}
    />
  );
};
