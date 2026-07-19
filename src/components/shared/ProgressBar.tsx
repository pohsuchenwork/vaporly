import React from "react";
import { useTranslation } from "react-i18next";

export interface ProgressData {
  id: string;
  percentage: number;
  speed?: number;
  label?: string;
}

interface ProgressBarProps {
  progress: ProgressData[];
  className?: string;
  size?: "small" | "medium" | "large";
  showSpeed?: boolean;
  showLabel?: boolean;
}

const ProgressBar: React.FC<ProgressBarProps> = ({
  progress,
  className = "",
  size = "medium",
  showSpeed = false,
  showLabel = false,
}) => {
  const { t } = useTranslation();
  const sizeClasses = {
    small: "w-16 h-1",
    medium: "w-20 h-1.5",
    large: "w-24 h-2",
  };

  const progressClasses = sizeClasses[size];

  if (progress.length === 0) {
    return null;
  }

  if (progress.length === 1) {
    // Single progress bar
    const item = progress[0];
    const percentage = Math.max(0, Math.min(100, item.percentage));

    return (
      <div className={`flex items-center gap-3 ${className}`}>
        {/* div-based bar: native <progress> cannot animate its value in
            WebKit, which is exactly the stutter users see. */}
        <div
          className={`${progressClasses} rounded-full bg-surface-well overflow-hidden`}
          role="progressbar"
          aria-valuenow={Math.round(percentage)}
          aria-valuemin={0}
          aria-valuemax={100}
        >
          <div
            className="h-full rounded-full bg-accent"
            style={{
              transition: "width 220ms ease-out",
              width: `${percentage.toFixed(1)}%`,
            }}
          />
        </div>
        {(showSpeed || showLabel) && (
          <div className="text-xs text-ink/60 tabular-nums min-w-fit">
            {showLabel && item.label && (
              <span className="me-2">{item.label}</span>
            )}
            {showSpeed && item.speed !== undefined && item.speed > 0 ? (
              // eslint-disable-next-line i18next/no-literal-string
              <span>{item.speed.toFixed(1)}MB/s</span>
            ) : showSpeed ? (
              <span>{t("common.downloading")}</span>
            ) : null}
          </div>
        )}
      </div>
    );
  }

  // Multiple progress bars
  return (
    <div className={`flex items-center gap-2 ${className}`}>
      <div className="flex gap-1">
        {progress.map((item) => {
          const percentage = Math.max(0, Math.min(100, item.percentage));
          return (
            <progress
              key={item.id}
              value={percentage}
              max={100}
              title={item.label || `${percentage}%`}
              className="w-3 h-1.5 [&::-webkit-progress-bar]:rounded-full [&::-webkit-progress-bar]:bg-surface-well [&::-webkit-progress-value]:rounded-full [&::-webkit-progress-value]:bg-accent"
            />
          );
        })}
      </div>
      <div className="text-xs text-ink/60 min-w-fit">
        {t("common.downloadingCount", { count: progress.length })}
      </div>
    </div>
  );
};

export default ProgressBar;
