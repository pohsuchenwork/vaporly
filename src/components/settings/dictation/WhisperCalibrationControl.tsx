import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type WhisperCalibration } from "@/bindings";
import { Button } from "../../ui/Button";
import { Dialog } from "../../ui/Dialog";
import { SettingContainer } from "../../ui/SettingContainer";
import { useSettings } from "../../../hooks/useSettings";

type Phase =
  "idle" | "ambient" | "normal" | "whisper" | "saving" | "done" | "error";

const CAPTURE_PHASES = ["ambient", "normal", "whisper"] as const;
type CapturePhase = (typeof CAPTURE_PHASES)[number];

/** The advance button unlocks this many seconds into each phase. */
const UNLOCK_SECONDS = 5;
/** Safety cap: a phase auto-advances after this long (bounded memory). */
const CAP_SECONDS = 30;

/**
 * Optional per-mic whisper calibration (wizard v2). Each phase records
 * OPEN-ENDEDLY: silence starts, its advance button unlocks after 5 seconds,
 * and the phase keeps recording until pressed (normal voice and whisper the
 * same; whisper ends with Finish). The user sets the pace; a 30 second cap
 * auto-advances if they walk away. Momentary by design: nothing runs until
 * started, nothing keeps running after.
 */
export const WhisperCalibrationControl: React.FC<{ disabled?: boolean }> = ({
  disabled,
}) => {
  const { t } = useTranslation();
  const { getSetting, refreshSettings } = useSettings();
  const calibration = (getSetting("whisper_calibration") ??
    null) as WhisperCalibration | null;
  const [open, setOpen] = useState(false);
  const [phase, setPhase] = useState<Phase>("idle");
  const [elapsed, setElapsed] = useState(0);
  const [result, setResult] = useState<WhisperCalibration | null>(null);
  const [error, setError] = useState("");
  const levels = useRef<{ ambient: number; normal: number; whisper: number }>({
    ambient: 0,
    normal: 0,
    whisper: 0,
  });
  const advancing = useRef(false);

  const capturing = (CAPTURE_PHASES as readonly string[]).includes(phase);
  const running = capturing || phase === "saving";
  const unlocked = elapsed >= UNLOCK_SECONDS;

  // One ticker per capture phase; drives the unlock countdown and the cap.
  useEffect(() => {
    if (!capturing) return;
    setElapsed(0);
    const id = setInterval(() => setElapsed((s) => s + 1), 1000);
    return () => clearInterval(id);
  }, [phase, capturing]);

  useEffect(() => {
    if (capturing && elapsed >= CAP_SECONDS) {
      void advance();
    }
  }, [elapsed, capturing]);

  const startPhase = async (next: CapturePhase) => {
    const res = await commands.whisperCalibrationPhaseStart();
    if (res.status === "error") throw new Error(res.error);
    setPhase(next);
  };

  const run = async () => {
    try {
      setError("");
      advancing.current = false;
      await startPhase("ambient");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setPhase("error");
    }
  };

  /** End the current phase, keep its level, move to the next step. */
  const advance = async () => {
    if (advancing.current || !capturing) return;
    advancing.current = true;
    const current = phase as CapturePhase;
    try {
      const res = await commands.whisperCalibrationPhaseStop();
      if (res.status === "error") throw new Error(res.error);
      // Ambient is uniform silence, so the mean is right; the voice phases
      // use the 75th percentile so trailing gaps before the press cannot
      // drag the measured level down.
      levels.current[current] =
        current === "ambient" ? res.data.mean_rms : res.data.p75_rms;

      if (current === "ambient") {
        await startPhase("normal");
      } else if (current === "normal") {
        await startPhase("whisper");
      } else {
        setPhase("saving");
        const fin = await commands.whisperCalibrationFinish(
          levels.current.ambient,
          levels.current.normal,
          levels.current.whisper,
        );
        if (fin.status === "error") throw new Error(fin.error);
        setResult(fin.data);
        setPhase("done");
        await refreshSettings();
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setPhase("error");
    } finally {
      advancing.current = false;
    }
  };

  /** Abandon the run: stop any live capture, discard, back to idle. */
  const cancel = async () => {
    if (capturing) {
      try {
        await commands.whisperCalibrationPhaseStop();
      } catch {
        // discarding anyway
      }
    }
    advancing.current = false;
    setPhase("idle");
    setError("");
  };

  const remove = async () => {
    await commands.whisperCalibrationClear();
    setResult(null);
    setPhase("idle");
    await refreshSettings();
  };

  const handleOpenChange = (next: boolean) => {
    if (!next) {
      void cancel();
    }
    setOpen(next);
  };

  const fmt = (v: number) => v.toFixed(4);
  const doneKey = (sep: string) =>
    sep === "good" || sep === "workable" || sep === "poor" ? sep : "workable";
  const advanceLabel =
    phase === "whisper"
      ? t("settings.dictation.whisper.calibration.finishBtn")
      : t("settings.dictation.whisper.calibration.continueBtn");

  return (
    <>
      <SettingContainer
        title={t("settings.dictation.whisper.calibration.title")}
        description={t("settings.dictation.whisper.calibration.description")}
        grouped={true}
        disabled={disabled}
      >
        <Button
          variant="secondary"
          size="sm"
          disabled={disabled}
          onClick={() => setOpen(true)}
        >
          {calibration
            ? t("settings.dictation.whisper.calibration.recalibrate")
            : t("settings.dictation.whisper.calibration.calibrate")}
        </Button>
      </SettingContainer>

      <Dialog
        open={open}
        onOpenChange={handleOpenChange}
        title={t("settings.dictation.whisper.calibration.dialogTitle")}
        closeLabel={t("settings.dictation.whisper.calibration.close")}
        dismissible={!running}
        closeOnBackdrop={false}
        footer={
          <div className="flex items-center justify-end gap-2">
            {phase === "idle" && calibration && (
              <Button variant="ghost" size="sm" onClick={remove}>
                {t("settings.dictation.whisper.calibration.remove")}
              </Button>
            )}
            {(phase === "idle" || phase === "error") && (
              <Button variant="primary" size="sm" onClick={run}>
                {phase === "error"
                  ? t("settings.dictation.whisper.calibration.tryAgain")
                  : t("settings.dictation.whisper.calibration.start")}
              </Button>
            )}
            {capturing && (
              <>
                <Button variant="ghost" size="sm" onClick={cancel}>
                  {t("settings.dictation.whisper.calibration.cancel")}
                </Button>
                <Button
                  variant="primary"
                  size="sm"
                  disabled={!unlocked}
                  onClick={advance}
                >
                  {advanceLabel}
                </Button>
              </>
            )}
            {phase === "done" && (
              <Button
                variant="secondary"
                size="sm"
                onClick={() => handleOpenChange(false)}
              >
                {t("settings.dictation.whisper.calibration.close")}
              </Button>
            )}
          </div>
        }
      >
        <div className="space-y-3 text-sm">
          {phase === "idle" && (
            <>
              <p className="text-ink-muted">
                {t("settings.dictation.whisper.calibration.intro")}
              </p>
              {calibration && (
                <p className="text-ink-muted">
                  {t("settings.dictation.whisper.calibration.calibratedFor", {
                    device: calibration.device_name,
                    separation: calibration.separation,
                  })}
                </p>
              )}
            </>
          )}
          {capturing && (
            <>
              <p className="font-medium text-ink">
                {t(`settings.dictation.whisper.calibration.phase.${phase}`)}
              </p>
              <p className="text-ink-muted tabular-nums">
                {unlocked
                  ? t("settings.dictation.whisper.calibration.pressWhenDone")
                  : t("settings.dictation.whisper.calibration.unlocksIn", {
                      s: UNLOCK_SECONDS - elapsed,
                    })}
              </p>
            </>
          )}
          {phase === "saving" && (
            <p className="font-medium text-ink">
              {t("settings.dictation.whisper.calibration.phase.saving")}
            </p>
          )}
          {phase === "done" && result && (
            <>
              <p className="font-medium text-ink">
                {t(
                  `settings.dictation.whisper.calibration.done.${doneKey(result.separation)}`,
                )}
              </p>
              <p className="text-ink-muted tabular-nums">
                {t("settings.dictation.whisper.calibration.levels", {
                  ambient: fmt(result.ambient_rms),
                  normal: fmt(result.normal_rms),
                  whisper: fmt(result.whisper_rms),
                })}
              </p>
              <p className="text-ink-muted">
                {t("settings.dictation.whisper.calibration.done.tip")}
              </p>
            </>
          )}
          {phase === "error" && (
            <p className="text-danger">
              {t("settings.dictation.whisper.calibration.error", {
                message: error,
              })}
            </p>
          )}
        </div>
      </Dialog>
    </>
  );
};
