import React, { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Mic } from "lucide-react";
import VaporlyWordmark from "../icons/VaporlyWordmark";
import { Button } from "../ui/Button";
import { events } from "@/bindings";
import { cn } from "@/lib/utils/cn";
import { useSettings } from "../../hooks/useSettings";
import { useModelStore } from "../../stores/modelStore";
import { useOsType } from "../../hooks/useOsType";
import { useReducedMotion } from "../../hooks/useReducedMotion";

interface TryItStepProps {
  onComplete: () => void;
}

/** Render a stored binding ("option+space") as keycap chips (⌥ Space). */
const useBindingKeys = (binding: string | undefined, isMac: boolean) =>
  useMemo(() => {
    if (!binding) return isMac ? ["Fn"] : ["Ctrl", "Space"];
    const mac: Record<string, string> = {
      cmd: "⌘",
      command: "⌘",
      meta: "⌘",
      option: "⌥",
      alt: isMac ? "⌥" : "Alt",
      shift: "⇧",
      ctrl: isMac ? "⌃" : "Ctrl",
      control: isMac ? "⌃" : "Ctrl",
      space: "Space",
      escape: "Esc",
      fn: "Fn",
      function: "Fn",
    };
    return binding
      .split("+")
      .map((k) => k.trim().toLowerCase())
      .map((k) => mac[k] ?? k.charAt(0).toUpperCase() + k.slice(1));
  }, [binding, isMac]);

/**
 * Final onboarding step: first dictation success inside a friendly demo box.
 * The paste path types straight into the focused textarea, so the user's own
 * words appearing IS the success moment; we confirm via the history "added"
 * event rather than parsing the textarea.
 */
export const TryItStep: React.FC<TryItStepProps> = ({ onComplete }) => {
  const { t } = useTranslation();
  const os = useOsType();
  const reduced = useReducedMotion();
  const { settings } = useSettings();
  const downloadProgress = useModelStore((s) => s.downloadProgress);
  const [succeeded, setSucceeded] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const binding = settings?.bindings?.["transcribe"]?.current_binding;
  const keys = useBindingKeys(binding, os === "macos");
  const downloading = Object.keys(downloadProgress).length > 0;

  useEffect(() => {
    textareaRef.current?.focus();
  }, []);

  useEffect(() => {
    const unlisten = events.historyUpdatePayload.listen((event) => {
      if (event.payload.action === "added") {
        setSucceeded(true);
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  return (
    <div className="motion-stagger h-screen w-screen flex flex-col items-center justify-center p-8 gap-6 text-center">
      <VaporlyWordmark width={160} />

      {/* The mic circle crossfades to the check on success: same accent-tint
          circle, the check pops in with one sakura bloom swelling behind it. */}
      <div className="relative flex items-center justify-center">
        {succeeded ? (
          <>
            {!reduced && (
              <span
                aria-hidden="true"
                className="accent-bloom pointer-events-none absolute left-1/2 top-1/2 h-24 w-24 -translate-x-1/2 -translate-y-1/2 rounded-full"
                style={{
                  background:
                    "radial-gradient(circle, var(--color-accent), transparent 70%)",
                }}
              />
            )}
            <div
              className={cn(
                "relative flex items-center justify-center w-14 h-14 rounded-full bg-accent-tint",
                !reduced && "motion-pop",
              )}
            >
              <Check className="w-7 h-7 text-accent" aria-hidden="true" />
            </div>
          </>
        ) : (
          <div className="flex items-center justify-center w-14 h-14 rounded-full bg-accent-tint">
            <Mic className="w-7 h-7 text-accent" aria-hidden="true" />
          </div>
        )}
      </div>

      {/* Re-keyed so the copy rises in when it swaps to the success message. */}
      <div
        key={succeeded ? "copy-done" : "copy-prompt"}
        className="space-y-1.5 max-w-md"
      >
        {succeeded ? (
          <>
            <h1 className="text-xl font-semibold">
              {t("onboarding.tryIt.successTitle")}
            </h1>
            <p className="text-ink-muted">
              {t("onboarding.tryIt.successBody")}
            </p>
          </>
        ) : (
          <>
            <h1 className="text-xl font-semibold">
              {t("onboarding.tryIt.title")}
            </h1>
            <p className="text-ink-muted">
              {t("onboarding.tryIt.subtitleHold")}{" "}
              <span className="inline-flex gap-1 align-middle mx-0.5">
                {keys.map((k, i) => (
                  <kbd
                    key={i}
                    className="px-2 py-0.5 rounded-control bg-surface-well text-ink text-sm font-medium"
                  >
                    {k}
                  </kbd>
                ))}
              </span>{" "}
              {t("onboarding.tryIt.subtitleSpeak")}
            </p>
          </>
        )}
      </div>

      {/* The user's own just-spoken words appear here. On success a faint accent
          wash recedes to the resting well tone, echoing vapor settling. Inline so it
          wins over the stagger's entrance animation on this same element. */}
      <textarea
        ref={textareaRef}
        rows={4}
        readOnly={succeeded}
        placeholder={t("onboarding.tryIt.placeholder")}
        aria-label={t("onboarding.tryIt.title")}
        className="w-full max-w-md rounded-card bg-surface-well p-4 text-base leading-relaxed resize-none focus-ring placeholder:text-ink-subtle"
        style={
          succeeded && !reduced
            ? {
                animation:
                  "success-wash var(--dur-hero-2) var(--ease-standard) both",
              }
            : undefined
        }
      />

      {downloading && !succeeded && (
        <p className="text-sm text-ink-subtle">
          {t("onboarding.tryIt.downloading")}
        </p>
      )}

      {/* Re-keyed so the primary CTA rises in as it replaces the skip link. */}
      <div
        key={succeeded ? "cta-done" : "cta-prompt"}
        className="flex items-center gap-3"
      >
        {succeeded ? (
          <Button onClick={onComplete} variant="primary" size="lg">
            {t("onboarding.tryIt.continue")}
          </Button>
        ) : (
          <Button onClick={onComplete} variant="ghost" size="md">
            {t("onboarding.tryIt.skip")}
          </Button>
        )}
      </div>
    </div>
  );
};
