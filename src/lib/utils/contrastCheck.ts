/**
 * Dev-only WCAG contrast check for the semantic color tokens.
 *
 * This is a developer tool, NOT wired into CI. It reads the live computed values
 * of the `--color-*` tokens off the running document (so it verifies exactly what
 * is painted, var() chains and color-mix included) and asserts every key
 * foreground/background pair clears its WCAG target.
 *
 * How to run:
 *   - From the Lab (once it imports this module) the helpers are attached to
 *     window in dev: `window.vaporlyContrastCheck()` for the current theme, or
 *     `window.vaporlyContrastCheckBothThemes()` to flip data-theme and check both.
 *   - Or paste the self-contained snippet from QA.md ("Design system contrast
 *     check") into the devtools console of either window (main + overlay).
 *
 * Pass criteria (WCAG 2.1):
 *   - body/UI text pairs (ink, ink-muted, on-accent, on-selected, accent-ink): >= 4.5:1
 *   - the focus ring against every surface it can land on: >= 3:1
 */

type Rgb = { r: number; g: number; b: number };

/** Parse a browser-computed color string (rgb/rgba, or color(srgb ...)). */
function parseColorString(value: string): Rgb | null {
  const v = value.trim();
  // color(srgb r g b [/ a]) with 0..1 channels (wide-gamut serialization).
  const srgb = v.match(/^color\(srgb\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)/i);
  if (srgb) {
    return {
      r: Math.round(parseFloat(srgb[1]) * 255),
      g: Math.round(parseFloat(srgb[2]) * 255),
      b: Math.round(parseFloat(srgb[3]) * 255),
    };
  }
  // rgb()/rgba(), comma- or space-separated, with 0..255 channels.
  const nums = v.match(/[\d.]+/g);
  if (nums && nums.length >= 3) {
    return {
      r: parseFloat(nums[0]),
      g: parseFloat(nums[1]),
      b: parseFloat(nums[2]),
    };
  }
  return null;
}

/**
 * Resolve any CSS color expression (including var() and color-mix) to concrete
 * sRGB channels by letting the browser paint it on a throwaway probe element.
 */
export function resolveColor(expr: string): Rgb | null {
  const probe = document.createElement("span");
  probe.style.color = expr;
  probe.style.position = "absolute";
  probe.style.pointerEvents = "none";
  probe.style.opacity = "0";
  document.body.appendChild(probe);
  const computed = getComputedStyle(probe).color;
  probe.remove();
  return parseColorString(computed);
}

function channelLuminance(c: number): number {
  const s = c / 255;
  return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
}

/** WCAG relative luminance of an sRGB color. */
export function relativeLuminance(c: Rgb): number {
  return (
    0.2126 * channelLuminance(c.r) +
    0.7152 * channelLuminance(c.g) +
    0.0722 * channelLuminance(c.b)
  );
}

/** WCAG contrast ratio (1..21) between two sRGB colors. */
export function contrastRatio(a: Rgb, b: Rgb): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  const lighter = Math.max(la, lb);
  const darker = Math.min(la, lb);
  return (lighter + 0.05) / (darker + 0.05);
}

export interface ContrastPair {
  /** Human-readable "<fg> on <bg>" label. */
  name: string;
  /** Foreground color expression (a token var). */
  fg: string;
  /** Background color expression (a token var). */
  bg: string;
  /** WCAG minimum this pair must clear. */
  min: number;
}

const SURFACES: Array<{ label: string; token: string }> = [
  { label: "surface-page", token: "var(--color-surface-page)" },
  { label: "surface-panel", token: "var(--color-surface-panel)" },
  { label: "surface-raised", token: "var(--color-surface-raised)" },
  { label: "surface-well", token: "var(--color-surface-well)" },
];

/** The pairs asserted in both themes. */
export const TOKEN_PAIRS: ContrastPair[] = [
  // Primary + secondary ink must clear 4.5:1 on every surface it sits on.
  ...SURFACES.map((s) => ({
    name: `ink on ${s.label}`,
    fg: "var(--color-ink)",
    bg: s.token,
    min: 4.5,
  })),
  ...SURFACES.map((s) => ({
    name: `ink-muted on ${s.label}`,
    fg: "var(--color-ink-muted)",
    bg: s.token,
    min: 4.5,
  })),
  // Text painted onto the accent + selected tones.
  {
    name: "on-accent on accent-solid",
    fg: "var(--color-on-accent)",
    bg: "var(--color-accent-solid)",
    min: 4.5,
  },
  {
    name: "on-selected on surface-selected",
    fg: "var(--color-on-selected)",
    bg: "var(--color-surface-selected)",
    min: 4.5,
  },
  // Accent-colored TEXT (wordmark, links, the success beat) on the page. This is the pair
  // that regressed to ~1.3:1 before the accent-ink role existed.
  {
    name: "accent-ink on surface-page",
    fg: "var(--color-accent-ink)",
    bg: "var(--color-surface-page)",
    min: 4.5,
  },
  // The focus ring is a non-text UI indicator: it needs 3:1 against every
  // surface it can be drawn over (including the selected tone).
  ...SURFACES.map((s) => ({
    name: `focus-ring on ${s.label}`,
    fg: "var(--color-focus-ring)",
    bg: s.token,
    min: 3,
  })),
  {
    name: "focus-ring on surface-selected",
    fg: "var(--color-focus-ring)",
    bg: "var(--color-surface-selected)",
    min: 3,
  },
];

export interface ContrastRow extends ContrastPair {
  ratio: number;
  pass: boolean;
}

/** Compute every pair against the CURRENT theme. */
export function runContrastCheck(pairs: ContrastPair[] = TOKEN_PAIRS): {
  pass: boolean;
  rows: ContrastRow[];
} {
  const rows: ContrastRow[] = pairs.map((pair) => {
    const fg = resolveColor(pair.fg);
    const bg = resolveColor(pair.bg);
    const ratio = fg && bg ? contrastRatio(fg, bg) : 0;
    return {
      ...pair,
      ratio: Math.round(ratio * 100) / 100,
      pass: ratio >= pair.min,
    };
  });
  return { pass: rows.every((r) => r.pass), rows };
}

/** Run against the current theme and print a readable table + a pass/fail line. */
export function logContrastCheck(): boolean {
  const theme = document.documentElement.dataset.theme ?? "(OS default)";
  const { pass, rows } = runContrastCheck();
  console.log(`Vaporly contrast check - theme: ${theme}`);
  console.table(
    rows.map((r) => ({
      pair: r.name,
      ratio: r.ratio,
      min: r.min,
      result: r.pass ? "PASS" : "FAIL",
    })),
  );
  const failures = rows.filter((r) => !r.pass);
  if (failures.length > 0) {
    console.error(
      `Contrast check FAILED: ${failures.length} pair(s) below target.`,
      failures,
    );
  } else {
    console.log("Contrast check passed: every pair clears its target.");
  }
  return pass;
}

/**
 * Force light, then dark (via data-theme), checking each, then restore the
 * original attribute. Use this to verify both themes from one console call.
 */
export function logContrastCheckBothThemes(): boolean {
  const root = document.documentElement;
  const original = root.dataset.theme;
  let allPass = true;
  for (const theme of ["light", "dark"] as const) {
    root.dataset.theme = theme;
    allPass = logContrastCheck() && allPass;
  }
  if (original === undefined) {
    delete root.dataset.theme;
  } else {
    root.dataset.theme = original;
  }
  return allPass;
}

declare global {
  interface Window {
    vaporlyContrastCheck?: typeof logContrastCheck;
    vaporlyContrastCheckBothThemes?: typeof logContrastCheckBothThemes;
  }
}

// In dev, expose the helpers on window so a supervisor can call them straight
// from the console once any module (e.g. the Lab) has imported this file.
if (import.meta.env.DEV && typeof window !== "undefined") {
  window.vaporlyContrastCheck = logContrastCheck;
  window.vaporlyContrastCheckBothThemes = logContrastCheckBothThemes;
}
