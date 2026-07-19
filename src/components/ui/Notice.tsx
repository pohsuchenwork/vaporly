import React from "react";
import { X } from "lucide-react";
import { cn } from "@/lib/utils/cn";
import { IconButton } from "./IconButton";

type NoticeTone = "warning" | "danger" | "info" | "success";

interface NoticeProps {
  tone?: NoticeTone;
  icon?: React.ReactNode;
  children: React.ReactNode;
  onDismiss?: () => void;
  dismissLabel?: string;
  className?: string;
}

const TONE_SURFACE: Record<NoticeTone, string> = {
  warning: "bg-warning-tint",
  danger: "bg-danger-tint",
  info: "bg-info-tint",
  success: "bg-success-tint",
};

const TONE_ICON: Record<NoticeTone, string> = {
  warning: "text-warning",
  danger: "text-danger",
  info: "text-info",
  success: "text-success",
};

/**
 * An inline, border-free status banner: a tinted surface holding a tone-colored
 * icon and ink body text, with an optional dismiss. Replaces the ad hoc
 * "border border-amber-500/40 bg-amber-500/10" banners with one shared look.
 */
export const Notice: React.FC<NoticeProps> = ({
  tone = "warning",
  icon,
  children,
  onDismiss,
  dismissLabel,
  className,
}) => (
  <div
    className={cn(
      "rounded-card px-4 py-3 flex items-start gap-2 text-sm text-ink",
      TONE_SURFACE[tone],
      className,
    )}
  >
    {icon && (
      <span
        className={cn("mt-0.5 shrink-0", TONE_ICON[tone])}
        aria-hidden="true"
      >
        {icon}
      </span>
    )}
    <div className="flex-1 min-w-0">{children}</div>
    {onDismiss && dismissLabel && (
      <IconButton
        className="shrink-0 -mr-2 -mt-1"
        aria-label={dismissLabel}
        title={dismissLabel}
        onClick={onDismiss}
      >
        <X className="w-4 h-4" />
      </IconButton>
    )}
  </div>
);
