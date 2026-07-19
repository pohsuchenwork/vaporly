import type { CSSProperties } from "react";

// Vaporly droplet, the General-section nav glyph. Component name/file kept to
// avoid churn at the call site (Sidebar SECTIONS_CONFIG.general.icon).
const VaporlyDroplet = ({
  width,
  height,
  className,
  style,
}: {
  width?: number | string;
  height?: number | string;
  className?: string;
  style?: CSSProperties;
}) => (
  <svg
    width={width || 20}
    height={height || 20}
    viewBox="0 0 24 24"
    className={className || "fill-ink"}
    style={style}
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <path d="M12 2.2c3 4.9 7 8.4 7 12.4a7 7 0 1 1-14 0c0-4 4-7.5 7-12.4Z" />
  </svg>
);

export default VaporlyDroplet;
