import React, { useEffect, useRef, useState } from "react";
import { HelpCircle } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils/cn";
import { IconButton } from "./IconButton";
import { Tooltip } from "./Tooltip";

interface SettingContainerProps {
  title: string;
  description: string;
  children: React.ReactNode;
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
  layout?: "horizontal" | "stacked";
  disabled?: boolean;
  tooltipPosition?: "top" | "bottom";
}

/**
 * One setting row. Horizontal by default (label on the left, control on the
 * right); a few rows opt into a stacked layout. Rows live inside a SettingsGroup
 * card and separate by a hairline divider (divide-y on the panel) plus their
 * min-h-12 rhythm. The row uses mx-4 (margin, not padding) so the divider insets
 * from the card's side edges instead of spanning edge to edge.
 */
export const SettingContainer: React.FC<SettingContainerProps> = ({
  title,
  description,
  children,
  descriptionMode = "tooltip",
  layout = "horizontal",
  disabled = false,
  tooltipPosition = "top",
}) => {
  const { t } = useTranslation();
  const [showTooltip, setShowTooltip] = useState(false);
  const tooltipRef = useRef<HTMLDivElement>(null);

  // Close the click-opened tooltip on an outside press.
  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (
        tooltipRef.current &&
        !tooltipRef.current.contains(event.target as Node)
      ) {
        setShowTooltip(false);
      }
    };

    if (showTooltip) {
      document.addEventListener("mousedown", handleClickOutside);
      return () =>
        document.removeEventListener("mousedown", handleClickOutside);
    }
  }, [showTooltip]);

  const toggleTooltip = () => setShowTooltip((v) => !v);

  const titleClass = cn(
    "text-sm font-medium text-ink",
    disabled && "opacity-60",
  );

  const infoAffordance = (
    <div
      ref={tooltipRef}
      className="relative flex items-center"
      onMouseEnter={() => setShowTooltip(true)}
      onMouseLeave={() => setShowTooltip(false)}
    >
      <IconButton
        aria-label={t("common.moreInfo")}
        onClick={toggleTooltip}
        className="text-ink-subtle hover:text-ink-muted"
      >
        <HelpCircle className="size-4" aria-hidden="true" />
      </IconButton>
      {showTooltip && (
        <Tooltip targetRef={tooltipRef} position={tooltipPosition}>
          <p className="text-sm text-start leading-relaxed text-ink whitespace-pre-line">
            {description}
          </p>
        </Tooltip>
      )}
    </div>
  );

  if (layout === "stacked") {
    return (
      <div className="mx-4 py-3">
        {descriptionMode === "tooltip" ? (
          <div className="flex items-center gap-1 mb-2">
            <h3 className={titleClass}>{title}</h3>
            {infoAffordance}
          </div>
        ) : (
          <div className="mb-2">
            <h3 className={titleClass}>{title}</h3>
            <p
              className={cn("text-sm text-ink-muted", disabled && "opacity-60")}
            >
              {description}
            </p>
          </div>
        )}
        <div className="w-full">{children}</div>
      </div>
    );
  }

  // Horizontal layout (default): label left, control right.
  return (
    <div className="flex items-center justify-between gap-4 min-h-12 mx-4 py-3">
      <div className="max-w-[66%]">
        {descriptionMode === "tooltip" ? (
          <div className="flex items-center gap-1">
            <h3 className={titleClass}>{title}</h3>
            {infoAffordance}
          </div>
        ) : (
          <>
            <h3 className={titleClass}>{title}</h3>
            <p
              className={cn("text-sm text-ink-muted", disabled && "opacity-60")}
            >
              {description}
            </p>
          </>
        )}
      </div>
      <div className="relative">{children}</div>
    </div>
  );
};
