/* eslint-disable i18next/no-literal-string */
// The Lab: a hidden, dev-only gallery of every signature component, token and
// motion beat, shown in BOTH themes side by side. It is the standing regression
// surface and the demo reel in one page.
//
// NEVER SHIPPED. App.tsx imports this module through a lazy() that lives in a
// dead `import.meta.env.DEV ? ... : null` branch, so Rollup drops the whole
// module (and the RecordingOverlay.css it pulls in) from production bundles.
// Because it is a developer tool and not user-facing product, its literal
// strings are intentional, hence the file-level i18next-disable above.
//
// Open it in dev by typing the chord "g" then "l", or by putting "#lab" in the
// address bar (see src/hooks/useLabGate.ts).

import {
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { toast, Toaster } from "sonner";
import {
  AlertTriangle,
  Check,
  Cog,
  HelpCircle,
  Play,
  Plus,
  RefreshCw,
  Sparkles,
  Trash2,
  X,
} from "lucide-react";
import {
  Button,
  Card,
  Chip,
  Dialog,
  IconButton,
  Input,
  Menu,
  Notice,
  SegmentedControl,
  SettingsGroup,
  Slider,
  Textarea,
  ToggleSwitch,
  Tooltip,
} from "@/components/ui";
import { ProgressBar } from "@/components/shared";
import VaporlyWordmark from "../icons/VaporlyWordmark";
import { repaintAccent } from "@/styles/applyAccent";
import {
  logContrastCheckBothThemes,
  runContrastCheck,
  type ContrastRow,
} from "@/lib/utils/contrastCheck";
// The overlay's own stylesheet gives us the authentic .scard / .stext-cap /
// .committed / .tentative / .scaret classes and the --s-* tokens the harness
// below reuses. It rides in this lazy chunk, so it never reaches production.
import "../../overlay/RecordingOverlay.css";

// ----------------------------------------------------------------------------
// Both-theme display.
//
// The app's dark tokens are scoped to :root[data-theme="dark"] and the OS media
// query, so a nested <div data-theme="dark"> would NOT pick them up. And even if
// it did, applyAccent paints the accent vars inline on document.documentElement,
// which would shadow a nested wrapper. So each themed panel below declares the
// FULL token set inline: every semantic --color-*, the legacy compat aliases,
// and the overlay --s-* vars, at their concrete light/dark values. Descendants
// (Tailwind utilities, primitives, the overlay card) then resolve against the
// panel, never against the OS setting or the live document. Primitive refs
// (var(--neutral-*), var(--sakura-*), ...) stay valid because those are declared
// once on :root and never themed. The color-mix() --s-* expressions are
// redeclared here so they recompute against THIS panel's --color-* values.
// ----------------------------------------------------------------------------

const LIGHT_VARS = {
  "--color-surface-page": "var(--neutral-100)",
  "--color-surface-panel": "var(--neutral-50)",
  "--color-surface-raised": "#ffffff",
  "--color-surface-well": "var(--neutral-200)",
  "--color-surface-selected": "var(--sakura-300)",
  "--color-surface-scrim": "rgba(20, 10, 16, 0.42)",
  "--color-ink": "var(--neutral-975)",
  "--color-ink-muted": "var(--neutral-600)",
  "--color-ink-subtle": "var(--neutral-500)",
  "--color-on-accent": "#ffffff",
  "--color-on-selected": "var(--neutral-975)",
  "--color-accent": "var(--sakura-400)",
  "--color-accent-ink": "var(--sakura-700)",
  "--color-accent-tint": "var(--sakura-100)",
  "--color-accent-solid": "var(--sakura-600)",
  "--color-accent-solid-hover": "var(--sakura-700)",
  "--color-accent-solid-active": "var(--sakura-800)",
  "--color-focus-ring": "var(--sakura-600)",
  "--color-control-secondary": "var(--neutral-200)",
  "--color-control-secondary-hover": "var(--neutral-300)",
  "--color-control-ghost-hover": "var(--neutral-200)",
  "--color-danger": "var(--red-500)",
  "--color-danger-solid": "var(--red-500)",
  "--color-danger-tint": "var(--red-tint-l)",
  "--color-success": "var(--green-600)",
  "--color-success-tint": "var(--green-tint-l)",
  "--color-warning": "var(--amber-600)",
  "--color-warning-tint": "var(--amber-tint-l)",
  "--color-info": "var(--blue-600)",
  "--color-info-tint": "var(--blue-tint-l)",
  "--shadow-1":
    "0 1px 2px rgba(40, 20, 32, 0.05), 0 2px 6px rgba(40, 20, 32, 0.06)",
  "--shadow-2":
    "0 4px 10px rgba(40, 20, 32, 0.08), 0 8px 24px rgba(40, 20, 32, 0.08)",
  "--shadow-3":
    "0 12px 28px rgba(40, 20, 32, 0.12), 0 24px 56px rgba(40, 20, 32, 0.14)",
  // Legacy compat aliases.
  "--color-background": "var(--neutral-100)",
  "--color-background-ui": "var(--sakura-600)",
  "--color-logo-primary": "var(--sakura-400)",
  "--color-text": "var(--neutral-975)",
  "--color-logo-stroke": "#b34a6a",
  "--color-mid-gray": "#808080",
  "--color-text-stroke": "#f6f6f6",
  // Overlay --s-* tokens (recompute against the --color-* above).
  "--s-font": "var(--font-body)",
  "--s-surface": "color-mix(in srgb, var(--color-background) 98%, transparent)",
  "--s-accent": "var(--color-logo-primary)",
  "--s-accent-soft": "color-mix(in srgb, var(--color-accent) 55%, transparent)",
  "--s-muted": "#6e6e6e",
  "--s-faint": "#9a9a9a",
  "--s-border": "color-mix(in srgb, var(--color-mid-gray) 22%, transparent)",
  "--s-hair": "color-mix(in srgb, var(--color-mid-gray) 12%, transparent)",
  "--s-error": "#c14953",
} as CSSProperties;

const DARK_VARS = {
  "--color-surface-page": "var(--neutral-850)",
  "--color-surface-panel": "var(--neutral-800)",
  "--color-surface-raised": "var(--neutral-750)",
  "--color-surface-well": "var(--neutral-900)",
  "--color-surface-selected": "var(--sakura-sel-dark)",
  "--color-surface-scrim": "rgba(0, 0, 0, 0.55)",
  "--color-ink": "var(--neutral-25)",
  "--color-ink-muted": "var(--neutral-400)",
  "--color-ink-subtle": "var(--neutral-500)",
  "--color-on-accent": "#ffffff",
  "--color-on-selected": "var(--neutral-25)",
  "--color-accent": "var(--sakura-300)",
  "--color-accent-ink": "var(--sakura-300)",
  "--color-accent-tint": "var(--sakura-tint-dark)",
  "--color-accent-solid": "var(--sakura-600)",
  "--color-accent-solid-hover": "var(--sakura-700)",
  "--color-accent-solid-active": "var(--sakura-800)",
  "--color-focus-ring": "var(--sakura-300)",
  "--color-control-secondary": "var(--neutral-750)",
  "--color-control-secondary-hover": "var(--neutral-700)",
  "--color-control-ghost-hover": "var(--neutral-800)",
  "--color-danger": "var(--red-400)",
  "--color-danger-solid": "var(--red-500)",
  "--color-danger-tint": "var(--red-tint-d)",
  "--color-success": "var(--green-400)",
  "--color-success-tint": "var(--green-tint-d)",
  "--color-warning": "var(--amber-400)",
  "--color-warning-tint": "var(--amber-tint-d)",
  "--color-info": "var(--blue-400)",
  "--color-info-tint": "var(--blue-tint-d)",
  "--shadow-1": "0 1px 2px rgba(0, 0, 0, 0.3), 0 2px 6px rgba(0, 0, 0, 0.28)",
  "--shadow-2": "0 4px 12px rgba(0, 0, 0, 0.4), 0 8px 24px rgba(0, 0, 0, 0.36)",
  "--shadow-3":
    "0 12px 32px rgba(0, 0, 0, 0.5), 0 24px 60px rgba(0, 0, 0, 0.5)",
  // Legacy compat aliases.
  "--color-background": "var(--neutral-850)",
  "--color-background-ui": "var(--sakura-600)",
  "--color-logo-primary": "var(--sakura-300)",
  "--color-text": "var(--neutral-25)",
  "--color-logo-stroke": "#ffd6e2",
  "--color-mid-gray": "#808080",
  "--color-text-stroke": "#f6f6f6",
  // Overlay --s-* tokens (recompute against the --color-* above).
  "--s-font": "var(--font-body)",
  "--s-surface": "color-mix(in srgb, var(--color-background) 98%, transparent)",
  "--s-accent": "var(--color-logo-primary)",
  "--s-accent-soft": "color-mix(in srgb, var(--color-accent) 55%, transparent)",
  "--s-muted": "#a3a09a",
  "--s-faint": "#6f6c66",
  "--s-border": "color-mix(in srgb, var(--color-text) 12%, transparent)",
  "--s-hair": "color-mix(in srgb, var(--color-text) 8%, transparent)",
  "--s-error": "#e0666f",
} as CSSProperties;

// ----------------------------------------------------------------------------
// Small layout helpers.
// ----------------------------------------------------------------------------

function Section({
  title,
  blurb,
  children,
}: {
  title: string;
  blurb?: string;
  children: ReactNode;
}) {
  return (
    <section className="space-y-4">
      <div className="space-y-1">
        <h2
          className="text-ink"
          style={{
            fontFamily: "var(--font-display)",
            fontSize: "var(--display-sm)",
            fontVariationSettings: '"opsz" 40, "wght" 460',
            lineHeight: 1.1,
          }}
        >
          {title}
        </h2>
        {blurb && <p className="text-sm text-ink-muted max-w-2xl">{blurb}</p>}
      </div>
      {children}
    </section>
  );
}

// Renders its children twice: once in a forced-light panel, once in a
// forced-dark one, so both themes read side by side regardless of the OS.
function TwoThemes({ children }: { children: ReactNode }) {
  return (
    <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
      <ThemedPanel theme="light">{children}</ThemedPanel>
      <ThemedPanel theme="dark">{children}</ThemedPanel>
    </div>
  );
}

function ThemedPanel({
  theme,
  children,
}: {
  theme: "light" | "dark";
  children: ReactNode;
}) {
  return (
    <div
      data-theme={theme}
      style={theme === "dark" ? DARK_VARS : LIGHT_VARS}
      className="bg-surface-page text-ink p-6 rounded-card overflow-x-auto"
    >
      <div className="text-[11px] uppercase tracking-wide text-ink-subtle mb-4">
        {theme}
      </div>
      {children}
    </div>
  );
}

// A labeled color chip. The inset ring keeps a page-tone swatch visible on a
// page-tone panel without borrowing the design's forbidden border.
function Swatch({ token, label }: { token: string; label: string }) {
  return (
    <div className="space-y-1">
      <div
        className="h-12 rounded-well"
        style={{
          background: `var(${token})`,
          boxShadow: "inset 0 0 0 1px rgba(128,128,128,0.28)",
        }}
      />
      <div className="text-[11px] leading-tight text-ink-muted break-all">
        {label}
      </div>
    </div>
  );
}

// A stateful "replay" wrapper: bumping the key remounts the child so any
// entrance animation on it plays again.
function Replay({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: (playKey: number) => ReactNode;
}) {
  const [playKey, setPlayKey] = useState(0);
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
        <Button
          size="sm"
          variant="secondary"
          onClick={() => setPlayKey((k) => k + 1)}
        >
          <RefreshCw className="size-3.5" aria-hidden="true" />
          Replay
        </Button>
        <span className="text-xs text-ink-muted">{label}</span>
      </div>
      {hint && <div className="text-[11px] text-ink-subtle">{hint}</div>}
      <div>{children(playKey)}</div>
    </div>
  );
}

// ----------------------------------------------------------------------------
// Section: Type specimen.
// ----------------------------------------------------------------------------

function TypeSpecimen() {
  const body: Array<[string, string, string]> = [
    ["overline", "text-xs uppercase tracking-wide text-ink-subtle", "~11px"],
    ["caption", "text-xs text-ink-muted", "~11px"],
    ["sm / UI", "text-sm text-ink", "~13px"],
    ["body", "text-base text-ink", "15px"],
    ["heading", "text-lg font-semibold text-ink", "~17px"],
  ];
  const display: Array<[string, string]> = [
    ["display-sm", "var(--display-sm)"],
    ["display-md", "var(--display-md)"],
    ["display-lg", "var(--display-lg)"],
  ];
  return (
    <div className="space-y-6">
      <div className="space-y-2">
        <div className="text-xs uppercase tracking-wide text-ink-subtle">
          Body / UI: Manrope
        </div>
        {body.map(([role, cls, px]) => (
          <div key={role} className="flex items-baseline gap-3">
            <span className="w-24 shrink-0 text-[11px] text-ink-subtle tabular-nums">
              {role} {px}
            </span>
            <span className={cls}>The spoken word becomes vapor.</span>
          </div>
        ))}
      </div>

      <div className="space-y-3">
        <div className="text-xs uppercase tracking-wide text-ink-subtle">
          Display: Manrope Light (sparse, large)
        </div>
        {display.map(([role, size]) => (
          <div key={role} className="flex items-baseline gap-3">
            <span className="w-24 shrink-0 text-[11px] text-ink-subtle">
              {role}
            </span>
            <span
              className="text-ink"
              style={{
                fontFamily: "var(--font-display)",
                fontSize: size,
                fontWeight: 340,
                lineHeight: 1.05,
              }}
            >
              Vapor.
            </span>
          </div>
        ))}
      </div>

      <div className="space-y-2">
        <div className="text-xs uppercase tracking-wide text-ink-subtle">
          Tabular figures (columns align)
        </div>
        <div className="text-sm text-ink tabular-nums">
          {["1,240 words", "48.02 MB/s", "9:07 elapsed", "100,001 total"].map(
            (n) => (
              <div key={n} className="flex justify-between max-w-[220px]">
                <span className="text-ink-subtle">row</span>
                <span>{n}</span>
              </div>
            ),
          )}
        </div>
      </div>

      <div className="space-y-2">
        <div className="text-xs uppercase tracking-wide text-ink-subtle">
          Wordmark (Manrope Light + accent-ink)
        </div>
        <VaporlyWordmark width={220} />
      </div>
    </div>
  );
}

// ----------------------------------------------------------------------------
// Section: Color + tokens.
// ----------------------------------------------------------------------------

function ColorTokens() {
  const surfaces: Array<[string, string]> = [
    ["--color-surface-page", "surface-page"],
    ["--color-surface-panel", "surface-panel"],
    ["--color-surface-raised", "surface-raised"],
    ["--color-surface-well", "surface-well"],
    ["--color-surface-selected", "surface-selected"],
  ];
  const ink: Array<[string, string]> = [
    ["--color-ink", "ink"],
    ["--color-ink-muted", "ink-muted"],
    ["--color-ink-subtle", "ink-subtle"],
  ];
  const accent: Array<[string, string]> = [
    ["--color-accent", "accent"],
    ["--color-accent-ink", "accent-ink"],
    ["--color-accent-solid", "accent-solid"],
    ["--color-accent-tint", "accent-tint"],
  ];
  const status: Array<[string, string]> = [
    ["--color-danger", "danger"],
    ["--color-danger-tint", "danger-tint"],
    ["--color-success", "success"],
    ["--color-success-tint", "success-tint"],
    ["--color-warning", "warning"],
    ["--color-warning-tint", "warning-tint"],
    ["--color-info", "info"],
    ["--color-info-tint", "info-tint"],
  ];
  const grid = (rows: Array<[string, string]>) => (
    <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
      {rows.map(([token, label]) => (
        <Swatch key={token} token={token} label={label} />
      ))}
    </div>
  );
  return (
    <div className="space-y-5">
      <div className="space-y-2">
        <div className="text-xs uppercase tracking-wide text-ink-subtle">
          Surfaces (tone climbs with elevation)
        </div>
        {grid(surfaces)}
      </div>
      <div className="space-y-2">
        <div className="text-xs uppercase tracking-wide text-ink-subtle">
          Ink
        </div>
        {grid(ink)}
      </div>
      <div className="space-y-2">
        <div className="text-xs uppercase tracking-wide text-ink-subtle">
          Accent (fill vs text vs solid)
        </div>
        {grid(accent)}
      </div>
      <div className="space-y-2">
        <div className="text-xs uppercase tracking-wide text-ink-subtle">
          Status
        </div>
        {grid(status)}
      </div>

      {/* The legibility case: accent-ink AS TEXT on the page tone. This is the
          fixed light-mode trap (was ~1.3:1 before the accent-ink role). */}
      <div className="space-y-2">
        <div className="text-xs uppercase tracking-wide text-ink-subtle">
          Legibility: accent-ink on surface-page
        </div>
        <div className="rounded-well p-3 bg-surface-page">
          <span
            className="text-lg font-semibold"
            style={{ color: "var(--color-accent-ink)" }}
          >
            Vapor reads clearly here.
          </span>
        </div>
      </div>
    </div>
  );
}

// A per-theme WCAG readout. Flips document.documentElement between light and
// dark (repainting the accent to match) to measure each theme with the shared
// contrastCheck util, then restores. The flips are synchronous within one task,
// so the running window never repaints an intermediate frame.
function ContrastReadout() {
  const [rows, setRows] = useState<{
    light: ContrastRow[];
    dark: ContrastRow[];
  }>({ light: [], dark: [] });

  const measure = () => {
    const root = document.documentElement;
    const original = root.dataset.theme;
    const collect = (theme: "light" | "dark") => {
      root.dataset.theme = theme;
      repaintAccent();
      return runContrastCheck().rows;
    };
    const light = collect("light");
    const dark = collect("dark");
    if (original === undefined) {
      delete root.dataset.theme;
    } else {
      root.dataset.theme = original;
    }
    repaintAccent();
    setRows({ light, dark });
  };

  // Measure once on mount; the Recheck button re-runs on demand.
  useEffect(() => {
    measure();
  }, []);

  const table = (label: string, data: ContrastRow[]) => (
    <div className="flex-1 min-w-[280px] space-y-1">
      <div className="text-xs uppercase tracking-wide text-ink-subtle">
        {label}
      </div>
      <div className="rounded-card overflow-hidden bg-surface-panel shadow-1 text-xs">
        {data.map((r) => (
          <div
            key={r.name}
            className="flex items-center justify-between gap-3 px-3 py-1.5"
          >
            <span className="text-ink-muted truncate">{r.name}</span>
            <span className="flex items-center gap-2 tabular-nums shrink-0">
              <span className="text-ink">{r.ratio.toFixed(2)}</span>
              <span
                style={{
                  color: r.pass
                    ? "var(--color-success)"
                    : "var(--color-danger)",
                }}
              >
                {r.pass ? "PASS" : "FAIL"}
              </span>
            </span>
          </div>
        ))}
      </div>
    </div>
  );

  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2">
        <Button size="sm" variant="secondary" onClick={measure}>
          Recheck
        </Button>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => logContrastCheckBothThemes()}
        >
          Log both themes to console
        </Button>
      </div>
      <div className="flex flex-wrap gap-4">
        {table("light", rows.light)}
        {table("dark", rows.dark)}
      </div>
    </div>
  );
}

// ----------------------------------------------------------------------------
// Section: Motion.
// ----------------------------------------------------------------------------

function MotionGallery() {
  const [houseKey, setHouseKey] = useState(0);
  const [vaporKey, setVaporKey] = useState(0);

  const chip = (label: string, tokens: string) => (
    <div className="text-[11px] text-ink-subtle mt-1">
      {label}
      <span className="mx-1">:</span>
      {tokens}
    </div>
  );

  const demoBox = (className: string, text: string) => (
    <div
      className={`rounded-well bg-surface-well px-4 py-3 text-sm text-ink ${className}`}
    >
      {text}
    </div>
  );

  return (
    <div className="space-y-6">
      {/* House entrance utilities. */}
      <div className="space-y-3">
        <div className="flex items-center gap-2">
          <Button
            size="sm"
            variant="secondary"
            onClick={() => setHouseKey((k) => k + 1)}
          >
            <RefreshCw className="size-3.5" aria-hidden="true" />
            Replay entrances
          </Button>
          <span className="text-xs text-ink-muted">
            house utilities (rise / fade / pop / stagger)
          </span>
        </div>
        <div key={houseKey} className="grid grid-cols-2 sm:grid-cols-4 gap-3">
          <div>
            {demoBox("motion-rise", "motion-rise")}
            {chip("rise 8..16px + fade", "dur-3 / ease-standard")}
          </div>
          <div>
            {demoBox("motion-fade", "motion-fade")}
            {chip("fade only", "dur-3 / ease-standard")}
          </div>
          <div>
            {demoBox("motion-pop", "motion-pop")}
            {chip("scale 0.92..1", "dur-2 / ease-emphasized")}
          </div>
          <div>
            <div className="motion-stagger space-y-1">
              <div className="rounded-well bg-surface-well px-3 py-1.5 text-xs text-ink">
                one
              </div>
              <div className="rounded-well bg-surface-well px-3 py-1.5 text-xs text-ink">
                two
              </div>
              <div className="rounded-well bg-surface-well px-3 py-1.5 text-xs text-ink">
                three
              </div>
            </div>
            {chip("stagger (bounded groups)", "48ms step")}
          </div>
        </div>
      </div>

      {/* The vapor vocabulary in isolation (the overlay's signature beats). */}
      <div className="space-y-3">
        <div className="flex items-center gap-2">
          <Button
            size="sm"
            variant="secondary"
            onClick={() => setVaporKey((k) => k + 1)}
          >
            <RefreshCw className="size-3.5" aria-hidden="true" />
            Replay vapor beats
          </Button>
          <span className="text-xs text-ink-muted">
            vapor-condense / vapor-drift / caret / draw-check / vapor-bloom
          </span>
        </div>
        <div
          key={vaporKey}
          className="grid grid-cols-2 sm:grid-cols-3 gap-4 items-start"
        >
          {/* vapor-condense + vapor-drift: a fresh commit condensing from vapor
              with the sakura mist blooming under it. Uses the authentic classes;
              the cap mask is neutralized so the single line is not top-faded. */}
          <div>
            <div
              className="stext-cap"
              style={{
                maxHeight: "none",
                WebkitMaskImage: "none",
                maskImage: "none",
                paddingTop: 0,
              }}
            >
              <p>
                <span className="committed">Speech becomes </span>
                <span className="committed condensing">vapor.</span>
              </p>
            </div>
            {chip("vapor-condense + vapor-drift", "dur-2/3 / ease-standard")}
          </div>

          {/* caret-breathe and caret-catch. */}
          <div>
            <div className="text-sm text-ink flex items-center gap-4">
              <span className="inline-flex items-center">
                breathe
                <span className="scaret" />
              </span>
              <span className="inline-flex items-center">
                catch
                <span className="scaret catch" />
              </span>
            </div>
            {chip("caret", "1.1s + catch 300ms / ease-spring")}
          </div>

          {/* draw-check + vapor-bloom (the success pair). */}
          <div>
            <span className="sresult-icon" style={{ color: "var(--s-accent)" }}>
              <span className="sresult-bloom" />
              <svg className="scheck" viewBox="0 0 16 16" aria-hidden="true">
                <path
                  d="M3.5 8.5 L6.5 11.5 L12.5 5"
                  stroke="currentColor"
                  strokeWidth="1.8"
                  fill="none"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
              </svg>
            </span>
            {chip("draw-check + vapor-bloom", "dur-3 / hero-2")}
          </div>
        </div>
      </div>
    </div>
  );
}

// ----------------------------------------------------------------------------
// Section: Primitives gallery.
// ----------------------------------------------------------------------------

function TooltipDemo() {
  const ref = useRef<HTMLDivElement>(null);
  const [show, setShow] = useState(false);
  return (
    <div
      ref={ref}
      className="inline-flex"
      onMouseEnter={() => setShow(true)}
      onMouseLeave={() => setShow(false)}
    >
      <Button size="sm" variant="secondary">
        Hover for tooltip
      </Button>
      {show && (
        <Tooltip targetRef={ref} position="top">
          <p className="text-sm text-center text-ink">
            A raised surface, one shadow, no border.
          </p>
        </Tooltip>
      )}
    </div>
  );
}

function PrimitivesGallery() {
  const [menuVal, setMenuVal] = useState<string | null>("turbo");
  const [toggles, setToggles] = useState({ on: true, off: false });
  const [slider, setSlider] = useState(0.4);
  const [segment, setSegment] = useState("comfortable");
  const [chipOn, setChipOn] = useState(true);
  const [chips, setChips] = useState(["email", "meetings", "code"]);
  const [dialogOpen, setDialogOpen] = useState(false);

  const label = (t: string) => (
    <div className="text-xs uppercase tracking-wide text-ink-subtle">{t}</div>
  );

  return (
    <div className="space-y-6">
      {/* Buttons. */}
      <div className="space-y-2">
        {label("Button")}
        <div className="flex flex-wrap gap-2 items-center">
          <Button variant="primary">
            <Sparkles className="size-4" aria-hidden="true" />
            Primary
          </Button>
          <Button variant="secondary">Secondary</Button>
          <Button variant="ghost">Ghost</Button>
          <Button variant="danger">Danger</Button>
          <Button variant="primary" disabled>
            Disabled
          </Button>
          <Button variant="primary" loading>
            Loading
          </Button>
          <Button variant="secondary" size="sm">
            Small
          </Button>
          <Button variant="secondary" size="lg">
            Large
          </Button>
        </div>
      </div>

      {/* IconButton. */}
      <div className="space-y-2">
        {label("IconButton (default / accent / danger / armed)")}
        <div className="flex flex-wrap gap-2 items-center">
          <IconButton aria-label="Settings">
            <Cog className="size-4" aria-hidden="true" />
          </IconButton>
          <IconButton aria-label="Sparkle" variant="accent">
            <Sparkles className="size-4" aria-hidden="true" />
          </IconButton>
          <IconButton aria-label="Delete" variant="danger">
            <Trash2 className="size-4" aria-hidden="true" />
          </IconButton>
          <IconButton aria-label="Confirm delete" variant="danger" armed>
            <Trash2 className="size-4" aria-hidden="true" />
          </IconButton>
          <IconButton aria-label="Disabled" disabled>
            <Cog className="size-4" aria-hidden="true" />
          </IconButton>
          <IconButton aria-label="Loading" loading>
            <Cog className="size-4" aria-hidden="true" />
          </IconButton>
        </div>
      </div>

      {/* Menu. */}
      <div className="space-y-2">
        {label("Menu (portaled list opens in the live theme)")}
        <div className="max-w-xs">
          <Menu
            selectedValue={menuVal}
            onSelect={setMenuVal}
            options={[
              { value: "small", label: "Whisper Small" },
              { value: "medium", label: "Whisper Medium" },
              { value: "turbo", label: "Whisper Turbo" },
              { value: "large", label: "Whisper Large", disabled: true },
            ]}
          />
        </div>
      </div>

      {/* Toggle + Slider rows (they render as SettingContainer rows). */}
      <div className="space-y-2">
        {label("ToggleSwitch + Slider (rows)")}
        <SettingsGroup>
          <ToggleSwitch
            label="On"
            description="A switch in the on state."
            descriptionMode="inline"
            checked={toggles.on}
            onChange={(v) => setToggles((s) => ({ ...s, on: v }))}
          />
          <ToggleSwitch
            label="Off"
            description="A switch in the off state."
            descriptionMode="inline"
            checked={toggles.off}
            onChange={(v) => setToggles((s) => ({ ...s, off: v }))}
          />
          <ToggleSwitch
            label="Disabled"
            description="A disabled switch."
            descriptionMode="inline"
            checked
            disabled
            onChange={() => {}}
          />
          <ToggleSwitch
            label="Loading"
            description="A switch mid-update."
            descriptionMode="inline"
            checked
            isUpdating
            onChange={() => {}}
          />
          <Slider
            label="Slider"
            description="A div-based slider."
            descriptionMode="inline"
            min={0}
            max={1}
            step={0.01}
            value={slider}
            onChange={setSlider}
          />
        </SettingsGroup>
      </div>

      {/* Input + Textarea. */}
      <div className="space-y-2">
        {label("Input + Textarea (click to see the focus ring)")}
        <div className="grid sm:grid-cols-2 gap-3">
          <Input placeholder="A recessed well input" />
          <Textarea placeholder="A recessed well textarea" />
        </div>
      </div>

      {/* SegmentedControl. */}
      <div className="space-y-2">
        {label("SegmentedControl")}
        <SegmentedControl
          aria-label="Density"
          value={segment}
          onChange={setSegment}
          options={[
            { value: "cozy", label: "Cozy" },
            { value: "comfortable", label: "Comfortable" },
            { value: "compact", label: "Compact" },
          ]}
        />
      </div>

      {/* Chips. */}
      <div className="space-y-2">
        {label("Chip (toggle + removable)")}
        <div className="flex flex-wrap gap-2 items-center">
          <Chip
            mode="toggle"
            pressed={chipOn}
            onToggle={() => setChipOn((v) => !v)}
          >
            Toggle me
          </Chip>
          {chips.map((c) => (
            <Chip
              key={c}
              mode="removable"
              removeLabel={`Remove ${c}`}
              onRemove={() => setChips((list) => list.filter((x) => x !== c))}
            >
              {c}
            </Chip>
          ))}
          <IconButton
            aria-label="Reset chips"
            onClick={() => setChips(["email", "meetings", "code"])}
          >
            <Plus className="size-4" aria-hidden="true" />
          </IconButton>
        </div>
      </div>

      {/* Card. */}
      <div className="space-y-2">
        {label("Card (panel + raised)")}
        <div className="grid sm:grid-cols-2 gap-3">
          <Card className="p-4 text-sm text-ink">Panel card, shadow-1.</Card>
          <Card raised className="p-4 text-sm text-ink">
            Raised card, shadow-2.
          </Card>
        </div>
      </div>

      {/* Dialog + Tooltip. */}
      <div className="space-y-2">
        {label("Dialog + Tooltip")}
        <div className="flex flex-wrap gap-3 items-center">
          <Button variant="secondary" onClick={() => setDialogOpen(true)}>
            Open dialog
          </Button>
          <TooltipDemo />
        </div>
        <Dialog
          open={dialogOpen}
          onOpenChange={setDialogOpen}
          title="A tokenized dialog"
          description="Raised surface, shadow-3, scrim behind."
          closeLabel="Close"
          footer={
            <>
              <Button variant="ghost" onClick={() => setDialogOpen(false)}>
                Cancel
              </Button>
              <Button variant="primary" onClick={() => setDialogOpen(false)}>
                Confirm
              </Button>
            </>
          }
        >
          <p className="text-sm text-ink">
            The dialog traps focus, restores it on close, and dismisses on
            Escape or a backdrop press.
          </p>
        </Dialog>
      </div>

      {/* ProgressBar. */}
      <div className="space-y-2">
        {label("ProgressBar")}
        <ProgressBar
          progress={[
            { id: "m", percentage: 62, speed: 4.2, label: "model.gguf" },
          ]}
          size="large"
          showSpeed
          showLabel
        />
      </div>

      {/* Notice. */}
      <div className="space-y-2">
        {label("Notice (warning / danger / info / success)")}
        <div className="space-y-2">
          <Notice tone="warning" icon={<AlertTriangle className="size-4" />}>
            A warning notice on a tinted surface.
          </Notice>
          <Notice tone="danger" icon={<AlertTriangle className="size-4" />}>
            A danger notice.
          </Notice>
          <Notice tone="info" icon={<HelpCircle className="size-4" />}>
            An info notice.
          </Notice>
          <Notice tone="success" icon={<Check className="size-4" />}>
            A success notice.
          </Notice>
        </div>
      </div>
    </div>
  );
}

// ----------------------------------------------------------------------------
// Section: Overlay hero harness.
//
// A contained replica of the Live panel (no ov-stage, so it does not go
// fullscreen). "Play dictation" feeds a scripted, monotonic committed/tentative
// stream on a timer so you WATCH the vapor condense, the sakura mist bloom,
// and the breathing caret without speaking. "Caught" plays the arrival beat
// (drawn check + bloom + card settle). The delta diff mirrors the real overlay:
// only the freshly committed span re-keys and animates.
// ----------------------------------------------------------------------------

type Frame = { committed: string; tentative: string };

const DICTATION_SCRIPT: Frame[] = [
  { committed: "", tentative: "so" },
  { committed: "", tentative: "so i" },
  { committed: "So I", tentative: "was" },
  { committed: "So I", tentative: "was thinking" },
  { committed: "So I was thinking,", tentative: "we should" },
  { committed: "So I was thinking,", tentative: "we should ship" },
  { committed: "So I was thinking, we should ship", tentative: "it" },
  { committed: "So I was thinking, we should ship it.", tentative: "" },
];

const STATIC_BARS = [8, 16, 24, 14, 28, 12, 20, 10, 6];

function OverlayHarness() {
  const [frame, setFrame] = useState<Frame>({ committed: "", tentative: "" });
  const [deltaStart, setDeltaStart] = useState(0);
  const [hasShownText, setHasShownText] = useState(false);
  const [landed, setLanded] = useState(false);
  const [cardKey, setCardKey] = useState(0);
  const committedLenRef = useRef(0);
  const timersRef = useRef<number[]>([]);
  const capRef = useRef<HTMLDivElement>(null);

  const clearTimers = () => {
    timersRef.current.forEach((id) => window.clearTimeout(id));
    timersRef.current = [];
  };

  useEffect(() => clearTimers, []);

  const applyFrame = (f: Frame) => {
    const prevLen = committedLenRef.current;
    if (f.committed.length > prevLen) setDeltaStart(prevLen);
    else if (f.committed.length < prevLen) setDeltaStart(f.committed.length);
    committedLenRef.current = f.committed.length;
    setFrame(f);
    if (f.committed.length > 0 || f.tentative.length > 0) setHasShownText(true);
    // Keep the newest line pinned.
    requestAnimationFrame(() => {
      const el = capRef.current;
      if (el) el.scrollTop = el.scrollHeight;
    });
  };

  const play = () => {
    clearTimers();
    setLanded(false);
    setHasShownText(false);
    setDeltaStart(0);
    committedLenRef.current = 0;
    setFrame({ committed: "", tentative: "" });
    setCardKey((k) => k + 1);
    DICTATION_SCRIPT.forEach((f, i) => {
      const id = window.setTimeout(() => applyFrame(f), 120 + i * 620);
      timersRef.current.push(id);
    });
  };

  const landedBeat = () => {
    setLanded(true);
    setCardKey((k) => k + 1);
    const id = window.setTimeout(() => setLanded(false), 1600);
    timersRef.current.push(id);
  };

  const hasText = frame.committed.length > 0 || frame.tentative.length > 0;

  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2">
        <Button size="sm" variant="primary" onClick={play}>
          <Play className="size-3.5" aria-hidden="true" />
          Play dictation
        </Button>
        <Button size="sm" variant="secondary" onClick={landedBeat}>
          <Check className="size-3.5" aria-hidden="true" />
          Caught beat
        </Button>
      </div>

      {/* Contained stage: centers the card, gives the pop room, never fixed. */}
      <div className="flex justify-center py-4">
        <div key={cardKey} className={`scard open ${landed ? "inserted" : ""}`}>
          <div className="stext">
            <div className="stext-clip">
              <div className="stext-cap" ref={capRef}>
                <p>
                  <span className="committed">
                    {frame.committed.slice(0, deltaStart)}
                  </span>
                  {frame.committed.length > deltaStart && (
                    <span
                      className="committed condensing"
                      key={frame.committed.length}
                    >
                      {frame.committed.slice(deltaStart) + " "}
                    </span>
                  )}
                  <span className="tentative">{frame.tentative}</span>
                  {!landed && (
                    <span
                      className={hasShownText ? "scaret catch" : "scaret"}
                    />
                  )}
                </p>
              </div>
            </div>
          </div>

          {landed ? (
            <div className="sbase">
              <div className="sbase-l">
                <span className="sresult-icon">
                  <span className="sresult-bloom" />
                  <svg
                    className="scheck"
                    viewBox="0 0 16 16"
                    aria-hidden="true"
                  >
                    <path
                      d="M3.5 8.5 L6.5 11.5 L12.5 5"
                      stroke="currentColor"
                      strokeWidth="1.8"
                      fill="none"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    />
                  </svg>
                </span>
              </div>
              <span className="swork-label motion-rise">Caught.</span>
              <div className="sbase-r" />
            </div>
          ) : (
            <div className="sbase">
              <div className="sbase-l">
                <span className="sdot" />
              </div>
              <div className="swave">
                {STATIC_BARS.map((h, i) => (
                  <i key={i} style={{ height: `${h}px` }} />
                ))}
              </div>
              <div className="sbase-r">
                <span className="stimer">0:03</span>
                <button className="sx" aria-label="Cancel" type="button">
                  <svg viewBox="0 0 16 16" aria-hidden="true">
                    <path
                      d="M4 4 L12 12 M12 4 L4 12"
                      stroke="currentColor"
                      strokeWidth="1.6"
                      strokeLinecap="round"
                    />
                  </svg>
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
      <div className="text-[11px] text-ink-subtle">
        tentative is vapor (faint); committed is clear (deep). Only the fresh
        delta condenses.{" "}
        {hasText ? "" : "Press Play dictation to watch it condense."}
      </div>
    </div>
  );
}

// ----------------------------------------------------------------------------
// Section: Toasts.
// ----------------------------------------------------------------------------

function ToastGallery() {
  return (
    <div className="flex flex-wrap gap-2">
      <Button
        size="sm"
        variant="secondary"
        onClick={() => toast.success("Caught. Your words are in.")}
      >
        Success
      </Button>
      <Button
        size="sm"
        variant="secondary"
        onClick={() =>
          toast.error("Those words did not make it.", {
            description: "They are on your clipboard.",
          })
        }
      >
        Error
      </Button>
      <Button
        size="sm"
        variant="secondary"
        onClick={() => toast.warning("One minute left in this session.")}
      >
        Warning
      </Button>
      <Button
        size="sm"
        variant="secondary"
        onClick={() => toast("A plain message toast.")}
      >
        Message
      </Button>
    </div>
  );
}

// ----------------------------------------------------------------------------
// The page.
// ----------------------------------------------------------------------------

export default function Lab() {
  const replayIntro = () => {
    try {
      sessionStorage.removeItem("vaporly.introShown");
      toast.success("Intro reveal cleared.", {
        description: "It plays on the next fresh onboarding run.",
      });
    } catch {
      toast.error("Could not clear the intro flag.");
    }
  };

  const closeLab = () => {
    location.hash = "";
  };

  return (
    <div className="min-h-dvh overflow-y-auto bg-surface-page text-ink cursor-default">
      <Toaster
        theme="system"
        position="top-right"
        style={{ zIndex: "var(--z-toast)" } as CSSProperties}
        toastOptions={{
          unstyled: true,
          classNames: {
            toast:
              "bg-surface-raised rounded-card shadow-3 px-4 py-3 flex items-center gap-3 text-sm",
            title: "text-ink font-medium",
            description: "text-ink-muted",
          },
        }}
      />
      <div className="mx-auto max-w-5xl p-6 space-y-10">
        {/* Header. */}
        <header className="space-y-3">
          <div className="flex items-center justify-between gap-4">
            <div className="flex items-center gap-3">
              <VaporlyWordmark width={150} />
              <span
                className="text-ink"
                style={{
                  fontFamily: "var(--font-display)",
                  fontSize: "var(--display-sm)",
                  fontVariationSettings: '"opsz" 40, "wght" 480',
                }}
              >
                The Lab
              </span>
            </div>
            <div className="flex items-center gap-2">
              <Button size="sm" variant="secondary" onClick={replayIntro}>
                <Sparkles className="size-3.5" aria-hidden="true" />
                Replay intro reveal
              </Button>
              <IconButton aria-label="Close the lab" onClick={closeLab}>
                <X className="size-4" aria-hidden="true" />
              </IconButton>
            </div>
          </div>
          <p className="text-sm text-ink-muted max-w-2xl">
            A dev-only gallery of every signature component, token and motion
            beat, in both themes. Never shipped (tree-shaken out of production).
            Open with the chord g then l, or with #lab in the address bar.
          </p>
        </header>

        <Section
          title="Type"
          blurb="Manrope across the whole UI, with a light weight for the sparse display octave, and tabular figures where numbers align."
        >
          <TwoThemes>
            <TypeSpecimen />
          </TwoThemes>
        </Section>

        <Section
          title="Color + tokens"
          blurb="The semantic surfaces, ink, accent and status tokens. Depth is tone plus one shadow, never a border."
        >
          <TwoThemes>
            <ColorTokens />
          </TwoThemes>
          <ContrastReadout />
        </Section>

        <Section
          title="Motion"
          blurb="House entrance utilities and the overlay's vapor vocabulary, each labeled with its easing and duration tokens."
        >
          <TwoThemes>
            <MotionGallery />
          </TwoThemes>
        </Section>

        <Section
          title="Primitives"
          blurb="Every variant and state of the core controls, imported straight from src/components/ui."
        >
          <TwoThemes>
            <PrimitivesGallery />
          </TwoThemes>
        </Section>

        <Section
          title="Overlay hero harness"
          blurb="A contained replica of the recording overlay's Live panel. Play a scripted dictation and watch words condense from vapor."
        >
          <TwoThemes>
            <OverlayHarness />
          </TwoThemes>
        </Section>

        <Section
          title="Toasts"
          blurb="Fired top-right (collision-proof against the overlay), tokenized to the raised surface."
        >
          <ToastGallery />
        </Section>
      </div>
    </div>
  );
}
