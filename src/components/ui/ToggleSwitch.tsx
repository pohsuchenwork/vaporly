import React from "react";
import { cn } from "@/lib/utils/cn";
import { SettingContainer } from "./SettingContainer";

interface ToggleSwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  isUpdating?: boolean;
  label: string;
  description: string;
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
  tooltipPosition?: "top" | "bottom";
}

/**
 * A borderless switch built on a real `role="switch"` button (no Flowbite
 * peer / white knob). Track tones step: off is a recessed well, on is the solid
 * accent; the knob is a raised chip that translates on transform. One focus
 * recipe (the shared ring), an active press, and the loading spinner recolored
 * to the accent.
 */
export const ToggleSwitch: React.FC<ToggleSwitchProps> = ({
  checked,
  onChange,
  disabled = false,
  isUpdating = false,
  label,
  description,
  descriptionMode = "tooltip",
  grouped = false,
  tooltipPosition = "top",
}) => {
  const inert = disabled || isUpdating;
  return (
    <SettingContainer
      title={label}
      description={description}
      descriptionMode={descriptionMode}
      grouped={grouped}
      disabled={disabled}
      tooltipPosition={tooltipPosition}
    >
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        aria-label={label}
        disabled={inert}
        onClick={() => onChange(!checked)}
        className={cn(
          "relative inline-flex items-center w-11 h-6 rounded-full transition-colors focus-ring shrink-0",
          checked ? "bg-accent-solid" : "bg-surface-well",
          inert
            ? "opacity-60 cursor-not-allowed"
            : "cursor-pointer active:scale-95",
        )}
      >
        <span
          className={cn(
            "absolute top-0.5 left-0.5 size-5 rounded-full bg-surface-raised border border-hairline transition-transform",
            checked ? "translate-x-5" : "translate-x-0",
          )}
        />
      </button>
      {isUpdating && (
        <div className="absolute inset-0 flex items-center justify-center">
          <div className="size-4 border-2 border-accent border-t-transparent rounded-full animate-spin" />
        </div>
      )}
    </SettingContainer>
  );
};
