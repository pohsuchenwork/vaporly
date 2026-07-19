import { useEffect, useState } from "react";

const QUERY = "(prefers-reduced-motion: reduce)";

/**
 * Tracks the user's OS "reduce motion" preference and re-renders on change.
 * SSR-safe: guards `window`/`matchMedia` and defaults to false.
 *
 * This is the JS gate for the few motion beats that CSS alone cannot collapse
 * (later commits: overlay ink toggles, onboarding reveal). Purely declarative
 * animations already name a reduced form via the motion.css tokens, so most of
 * the app needs no JS here.
 */
export function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState<boolean>(() => {
    if (typeof window === "undefined" || !window.matchMedia) return false;
    return window.matchMedia(QUERY).matches;
  });

  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return;
    const mql = window.matchMedia(QUERY);
    const onChange = (event: MediaQueryListEvent) => setReduced(event.matches);
    // Re-sync in case the preference flipped between the initial render and this
    // effect (or across a remount), then subscribe to future changes.
    setReduced(mql.matches);
    mql.addEventListener("change", onChange);
    return () => mql.removeEventListener("change", onChange);
  }, []);

  return reduced;
}
