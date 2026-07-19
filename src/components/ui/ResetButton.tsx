import React from "react";
import { useTranslation } from "react-i18next";
import ResetIcon from "../icons/ResetIcon";
import { IconButton } from "./IconButton";

interface ResetButtonProps {
  onClick: () => void;
  disabled?: boolean;
  className?: string;
  ariaLabel?: string;
  children?: React.ReactNode;
}

/**
 * Reset-to-default affordance: the canonical IconButton wearing the reset glyph.
 * API unchanged so its call sites compile.
 */
export const ResetButton: React.FC<ResetButtonProps> = React.memo(
  ({ onClick, disabled = false, className = "", ariaLabel, children }) => {
    const { t } = useTranslation();
    return (
      <IconButton
        onClick={onClick}
        disabled={disabled}
        aria-label={ariaLabel ?? t("common.reset")}
        className={className}
      >
        {children ?? <ResetIcon />}
      </IconButton>
    );
  },
);
