import React from "react";

interface SettingsGroupProps {
  title?: string;
  description?: string;
  children: React.ReactNode;
}

/**
 * A titled section of setting rows. The card is a flat, raised-by-tone panel
 * (no shadow, no border): rows inside separate by an inset hairline divider
 * (divide-y) plus their own min-h-12 rhythm. The header is a quiet uppercase
 * label, flush with the panel's left edge.
 */
export const SettingsGroup: React.FC<SettingsGroupProps> = ({
  title,
  description,
  children,
}) => {
  return (
    <div className="space-y-3">
      {title && (
        <div>
          <h2 className="text-xs font-medium text-ink-muted uppercase tracking-wide">
            {title}
          </h2>
          {description && (
            <p className="text-xs text-ink-muted mt-2">{description}</p>
          )}
        </div>
      )}
      <div className="bg-surface-panel rounded-card overflow-hidden divide-y divide-hairline">
        {children}
      </div>
    </div>
  );
};
