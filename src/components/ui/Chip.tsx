import React from "react";
import { X } from "lucide-react";
import { cn } from "@/lib/utils/cn";

interface ChipBase {
  children: React.ReactNode;
  disabled?: boolean;
  className?: string;
}

interface ToggleChip extends ChipBase {
  mode: "toggle";
  pressed: boolean;
  onToggle: () => void;
}

interface RemovableChip extends ChipBase {
  mode: "removable";
  onRemove: () => void;
  removeLabel: string;
}

type ChipProps = ToggleChip | RemovableChip;

const CHIP_BASE =
  "inline-flex items-center gap-2 min-h-8 px-3 rounded-full text-sm transition-colors";

/**
 * A borderless pill. `toggle` is a two-state button (aria-pressed) that lifts to
 * the selected tone when on; `removable` is a label with a trailing dismiss.
 * Commit 6 adopts it for the context categories and word chips.
 */
export const Chip: React.FC<ChipProps> = (props) => {
  if (props.mode === "toggle") {
    const { pressed, onToggle, disabled, className, children } = props;
    return (
      <button
        type="button"
        aria-pressed={pressed}
        disabled={disabled}
        onClick={onToggle}
        className={cn(
          CHIP_BASE,
          "focus-ring active:opacity-90",
          pressed
            ? "bg-surface-selected text-on-selected"
            : "bg-surface-well text-ink-muted hover:text-ink",
          disabled && "opacity-60 cursor-not-allowed",
          !disabled && "cursor-pointer",
          className,
        )}
      >
        {children}
      </button>
    );
  }

  const { onRemove, removeLabel, disabled, className, children } = props;
  return (
    <span
      className={cn(
        CHIP_BASE,
        "bg-surface-well text-ink",
        disabled && "opacity-60",
        className,
      )}
    >
      <span>{children}</span>
      <button
        type="button"
        aria-label={removeLabel}
        title={removeLabel}
        disabled={disabled}
        onClick={onRemove}
        className="inline-grid place-items-center rounded-full focus-ring text-ink-muted hover:opacity-70 active:opacity-90 disabled:cursor-not-allowed"
      >
        <X className="size-3" aria-hidden="true" />
      </button>
    </span>
  );
};
