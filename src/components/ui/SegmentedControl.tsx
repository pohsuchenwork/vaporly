import { cn } from "@/lib/utils/cn";

export interface SegmentedOption<T extends string> {
  value: T;
  label: string;
  disabled?: boolean;
}

interface SegmentedControlProps<T extends string> {
  options: SegmentedOption<T>[];
  value: T;
  onChange: (value: T) => void;
  disabled?: boolean;
  "aria-label"?: string;
  className?: string;
}

/**
 * A borderless segmented control: a recessed well track holding radio segments,
 * the active one lifted onto the selected tone with one shadow. One focus recipe
 * (the shared ring), a roving tab stop, and arrow-key navigation. Generalized
 * from the Dictation stage picker; commit 6 adopts it at the call sites.
 */
export function SegmentedControl<T extends string>({
  options,
  value,
  onChange,
  disabled = false,
  "aria-label": ariaLabel,
  className,
}: SegmentedControlProps<T>) {
  const move = (delta: number) => {
    const current = options.findIndex((o) => o.value === value);
    for (let i = 1; i <= options.length; i++) {
      const next =
        options[(current + delta * i + options.length) % options.length];
      if (next && !next.disabled) {
        onChange(next.value);
        return;
      }
    }
  };

  // Home jumps to the first enabled segment, End to the last.
  const jumpTo = (toEnd: boolean) => {
    const ordered = toEnd ? [...options].reverse() : options;
    const target = ordered.find((o) => !o.disabled);
    if (target) onChange(target.value);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (disabled) return;
    if (e.key === "ArrowRight" || e.key === "ArrowDown") {
      e.preventDefault();
      move(1);
    } else if (e.key === "ArrowLeft" || e.key === "ArrowUp") {
      e.preventDefault();
      move(-1);
    } else if (e.key === "Home") {
      e.preventDefault();
      jumpTo(false);
    } else if (e.key === "End") {
      e.preventDefault();
      jumpTo(true);
    }
  };

  return (
    <div
      role="radiogroup"
      aria-label={ariaLabel}
      onKeyDown={handleKeyDown}
      className={cn(
        "inline-flex bg-surface-well rounded-control p-0.5 gap-0.5",
        disabled && "opacity-60",
        className,
      )}
    >
      {options.map((option) => {
        const active = option.value === value;
        const segDisabled = disabled || option.disabled;
        return (
          <button
            key={option.value}
            type="button"
            role="radio"
            aria-checked={active}
            disabled={segDisabled}
            tabIndex={active ? 0 : -1}
            onClick={() => onChange(option.value)}
            className={cn(
              "min-h-8 px-3 text-xs font-medium rounded-well transition-colors focus-ring",
              segDisabled ? "cursor-not-allowed" : "cursor-pointer",
              active
                ? "bg-surface-selected text-on-selected"
                : "text-ink-muted hover:text-ink",
            )}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}
