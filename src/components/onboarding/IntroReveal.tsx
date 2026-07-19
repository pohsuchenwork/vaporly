import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import VaporlyWordmark from "../icons/VaporlyWordmark";
import { useReducedMotion } from "../../hooks/useReducedMotion";
import { cn } from "@/lib/utils/cn";

interface IntroRevealProps {
  /** Fired once when the reveal has played out or the user skipped it. */
  onDone: () => void;
}

/**
 * First-run reveal: a single, warm, skippable moment before onboarding begins.
 * The Manrope wordmark rises as its tracking settles, a sakura underline draws
 * in left to right, and the tagline fades in; then the plate
 * lifts away to hand off to the first step. Any key, click, or Esc skips
 * straight to onDone. Reduced motion holds a static wordmark + tagline briefly,
 * with no draws or rises.
 *
 * App.tsx renders this only on the fresh-onboarding path, once per session
 * (sessionStorage "vaporly.introShown"); it is not part of the step machine.
 */
export const IntroReveal: React.FC<IntroRevealProps> = ({ onDone }) => {
  const { t } = useTranslation();
  const reduced = useReducedMotion();
  const [leaving, setLeaving] = useState(false);
  const doneRef = useRef(false);

  const finish = useCallback(() => {
    if (doneRef.current) return;
    doneRef.current = true;
    onDone();
  }, [onDone]);

  // Timed sequence. Reduced motion just holds the static plate briefly; full
  // motion plays the reveal, lifts the plate away, then hands off.
  useEffect(() => {
    if (reduced) {
      const done = window.setTimeout(finish, 500);
      return () => window.clearTimeout(done);
    }
    const lift = window.setTimeout(() => setLeaving(true), 1050);
    const done = window.setTimeout(finish, 1300);
    return () => {
      window.clearTimeout(lift);
      window.clearTimeout(done);
    };
  }, [reduced, finish]);

  // Any key / click / Esc dismisses immediately.
  useEffect(() => {
    const skip = () => finish();
    window.addEventListener("keydown", skip);
    window.addEventListener("pointerdown", skip);
    return () => {
      window.removeEventListener("keydown", skip);
      window.removeEventListener("pointerdown", skip);
    };
  }, [finish]);

  return (
    <div className="h-screen w-screen flex items-center justify-center bg-surface-page cursor-default select-none">
      <div
        className={cn(
          "flex flex-col items-center gap-4",
          !reduced && leaving && "intro-leaving",
        )}
      >
        <VaporlyWordmark
          width={260}
          className={!reduced ? "intro-wordmark-anim" : undefined}
        />
        <span
          aria-hidden="true"
          className={cn(
            "h-[3px] w-48 rounded-full bg-accent",
            !reduced && "intro-underline",
          )}
        />
        <p
          className={cn("text-ink", !reduced && "intro-tagline")}
          style={{
            fontFamily: "var(--font-display)",
            fontSize: "var(--display-sm)",
          }}
        >
          {t("onboarding.tagline")}
        </p>
      </div>
    </div>
  );
};

export default IntroReveal;
