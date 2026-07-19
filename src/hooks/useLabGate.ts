import { useEffect, useState } from "react";

// The dev-only summon for the Lab (src/components/dev/Lab.tsx). Two ways in,
// both DEV-only:
//   1. Type the chord "g" then "l" within ~600ms (skipped while a field is
//      focused, so it never eats real typing).
//   2. Put "#lab" in the address bar (handy for reload-in-place).
// The chord toggles location.hash between "#lab" and empty; a hashchange
// listener keeps the returned flag in sync with the URL either way.
//
// In production this hook is a hard no-op: import.meta.env.DEV is statically
// false, so the early return elides the whole body (and App.tsx never renders
// the Lab). The value is constant for a given build, so the hook-call order is
// stable and React never sees a changing hook shape.

const CHORD_WINDOW_MS = 600;
const LAB_HASH = "lab";

function hashIsLab(): boolean {
  return typeof location !== "undefined" && location.hash === "#" + LAB_HASH;
}

export function useLabGate(): boolean {
  if (!import.meta.env.DEV) return false;

  const [open, setOpen] = useState<boolean>(() => hashIsLab());

  useEffect(() => {
    const syncFromHash = () => setOpen(hashIsLab());
    window.addEventListener("hashchange", syncFromHash);

    // "g" then "l" within the window toggles the lab. Any other key (or a long
    // pause) breaks the pending chord.
    let lastG = 0;
    const onKeyDown = (event: KeyboardEvent) => {
      // Never intercept the chord while the user is typing into a field.
      const el = document.activeElement as HTMLElement | null;
      const tag = el?.tagName;
      if (
        tag === "INPUT" ||
        tag === "TEXTAREA" ||
        tag === "SELECT" ||
        el?.isContentEditable
      ) {
        return;
      }
      if (event.ctrlKey || event.metaKey || event.altKey) return;

      const key = event.key.toLowerCase();
      if (key === "g") {
        lastG = Date.now();
        return;
      }
      if (key === "l" && Date.now() - lastG <= CHORD_WINDOW_MS) {
        lastG = 0;
        // Toggle the hash; syncFromHash updates state on the resulting event.
        location.hash = hashIsLab() ? "" : LAB_HASH;
        return;
      }
      lastG = 0;
    };
    window.addEventListener("keydown", onKeyDown);

    return () => {
      window.removeEventListener("hashchange", syncFromHash);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);

  return open;
}
