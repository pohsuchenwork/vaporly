import React from "react";
import { cn } from "@/lib/utils/cn";

interface CardProps extends React.HTMLAttributes<HTMLDivElement> {
  /** Lift onto the raised tone with a heavier shadow (menus, popovers). */
  raised?: boolean;
}

/**
 * A borderless, flat surface that separates by tone alone. `raised` steps up a
 * rung. Nothing else: padding and radius of the contents are the caller's.
 */
export const Card: React.FC<CardProps> = ({
  raised = false,
  className,
  ...props
}) => (
  <div
    className={cn(
      "rounded-card",
      raised ? "bg-surface-raised" : "bg-surface-panel",
      className,
    )}
    {...props}
  />
);
