import React, { useRef } from "react";
import { cn } from "@/lib/utils/cn";
import { SettingContainer } from "./SettingContainer";

interface SliderProps {
  value: number;
  onChange: (value: number) => void;
  min: number;
  max: number;
  step?: number;
  disabled?: boolean;
  label: string;
  description: string;
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
  showValue?: boolean;
  formatValue?: (value: number) => string;
}

const clamp = (v: number, lo: number, hi: number) =>
  Math.min(hi, Math.max(lo, v));

/**
 * A div-based slider (no raw range input, no gradient hack, no native thumb).
 * The track is a recessed well, the fill is the bright accent, the knob is a
 * raised chip. Focus is the shared ring; pointer drag and Arrow / Home / End
 * keys drive the value. Keeps the prop contract stable.
 */
export const Slider: React.FC<SliderProps> = ({
  value,
  onChange,
  min,
  max,
  step = 0.01,
  disabled = false,
  label,
  description,
  descriptionMode = "tooltip",
  grouped = false,
  showValue = true,
  formatValue = (v) => v.toFixed(2),
}) => {
  const trackRef = useRef<HTMLDivElement>(null);
  const decimals = (String(step).split(".")[1] || "").length;
  const percent = max > min ? (clamp(value, min, max) - min) / (max - min) : 0;

  const snap = (raw: number) => {
    const stepped = Math.round((raw - min) / step) * step + min;
    return Number(clamp(stepped, min, max).toFixed(decimals));
  };

  const setFromClientX = (clientX: number) => {
    const el = trackRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    // A degenerate zero-width track must never write a value: the old fallback
    // computed ratio 0, which is exactly how a collapsed layout once persisted
    // volume 0.0 on every click.
    if (rect.width <= 0) return;
    const ratio = (clientX - rect.left) / rect.width;
    onChange(snap(min + clamp(ratio, 0, 1) * (max - min)));
  };

  const handlePointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (disabled) return;
    // Drive the drag from window-level listeners. WKWebView's pointer capture
    // is unreliable, so an element-scoped pointermove gated on
    // hasPointerCapture never fired and the slider could only click-to-set.
    e.preventDefault();
    e.currentTarget.focus();
    setFromClientX(e.clientX);
    const move = (ev: PointerEvent) => setFromClientX(ev.clientX);
    const end = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", end);
      window.removeEventListener("blur", end);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", end);
    window.addEventListener("blur", end);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (disabled) return;
    let next: number | null = null;
    switch (e.key) {
      case "ArrowLeft":
      case "ArrowDown":
        next = value - step;
        break;
      case "ArrowRight":
      case "ArrowUp":
        next = value + step;
        break;
      case "Home":
        next = min;
        break;
      case "End":
        next = max;
        break;
      default:
        return;
    }
    e.preventDefault();
    onChange(snap(next));
  };

  return (
    <SettingContainer
      title={label}
      description={description}
      descriptionMode={descriptionMode}
      grouped={grouped}
      layout="horizontal"
      disabled={disabled}
    >
      <div className="w-56 flex items-center gap-2 h-6">
        <div
          ref={trackRef}
          role="slider"
          aria-label={label}
          aria-valuemin={min}
          aria-valuemax={max}
          aria-valuenow={value}
          aria-orientation="horizontal"
          aria-disabled={disabled || undefined}
          tabIndex={disabled ? -1 : 0}
          onPointerDown={handlePointerDown}
          onKeyDown={handleKeyDown}
          className={cn(
            "relative flex-grow h-6 flex items-center rounded-full focus-ring select-none touch-none",
            disabled ? "opacity-60 cursor-not-allowed" : "cursor-pointer",
          )}
        >
          <div className="w-full h-1.5 rounded-full bg-surface-well overflow-hidden">
            <div
              className="h-full rounded-full bg-accent"
              style={{ width: `${percent * 100}%` }}
            />
          </div>
          <div
            className="absolute top-1/2 size-4 rounded-full bg-surface-raised border border-hairline -translate-x-1/2 -translate-y-1/2"
            style={{ left: `${percent * 100}%` }}
          />
        </div>
        {showValue && (
          <span className="text-sm font-medium text-ink w-12 text-end tabular-nums">
            {formatValue(value)}
          </span>
        )}
      </div>
    </SettingContainer>
  );
};
