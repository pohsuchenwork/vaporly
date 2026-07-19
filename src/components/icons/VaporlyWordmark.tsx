import React from "react";
import VaporlyDroplet from "./VaporlyDroplet";

/**
 * Vaporly brand lockup: the sakura droplet mark, then the "Vaporly" wordmark set
 * in Sora (a geometric display face, distinct from the Manrope UI). Kept as the
 * shared component so every call site (sidebar, onboarding) renders the brand.
 * Width drives the type size; the droplet scales with it. `height` is accepted
 * for API-compat and ignored.
 */
const VaporlyWordmark = ({
  width = 160,
  className,
}: {
  width?: number;
  height?: number;
  className?: string;
}) => {
  const fontSize = Math.max(18, Math.round(width / 6.1));
  // Fit the droplet's VISIBLE teardrop to the cap height of "V". The teardrop
  // fills ~0.808 of its 24-unit viewBox vertically and leaves ~0.10 of the box
  // empty below it, so size the square SVG up from the target cap height, then
  // nudge it down by that bottom pad to drop the visible tip onto the baseline.
  // Net: visible top meets the cap-top of "V", visible bottom meets the
  // baseline. CAP_RATIO / BOTTOM_PAD are eyeballed for Manrope 600 - tune here.
  const CAP_RATIO = 0.7; // cap height / font size
  const TEARDROP_FILL = 0.808; // visible teardrop height / viewBox height
  const BOTTOM_PAD = 0.1; // empty space below teardrop / viewBox height
  const dropSize = Math.round((fontSize * CAP_RATIO) / TEARDROP_FILL);
  const dropNudge = dropSize * BOTTOM_PAD;
  const wordStyle: React.CSSProperties = {
    fontFamily: "var(--font-wordmark)",
    fontSize,
    fontWeight: 600,
    letterSpacing: "-0.02em",
    lineHeight: 1,
    color: "var(--color-text)",
  };
  return (
    <span
      className={className}
      role="img"
      aria-label="Vaporly"
      style={{
        display: "inline-flex",
        alignItems: "baseline",
        gap: Math.round(fontSize * 0.32),
        userSelect: "none",
      }}
    >
      <VaporlyDroplet
        width={dropSize}
        height={dropSize}
        className="fill-accent-ink shrink-0"
        style={{ transform: `translateY(${dropNudge}px)` }}
      />
      {/* eslint-disable-next-line i18next/no-literal-string -- brand name */}
      <span style={wordStyle}>Vaporly</span>
    </span>
  );
};

export default VaporlyWordmark;
