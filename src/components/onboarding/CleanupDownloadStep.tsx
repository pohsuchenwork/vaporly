import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Sparkles } from "lucide-react";
import VaporlyWordmark from "../icons/VaporlyWordmark";
import { Button } from "../ui/Button";
import { useLlmEngineStore } from "../../stores/llmEngineStore";
import { useSettings } from "../../hooks/useSettings";

interface CleanupDownloadStepProps {
  onComplete: () => void;
}

/**
 * Onboarding: fetch the cleanup model the hardware ladder recommends (round-2
 * defaults run mind-change on the Model engine out of the box, so a fresh
 * install needs the model to deliver what the dials promise). Auto-advances
 * when the model is already on disk and SKIPS entirely on the Raw hardware
 * tier, which has no recommended model (those machines paste
 * deterministically). A visible skip keeps slow connections from blocking
 * onboarding; the Dictation page's engine card can download later.
 */
export const CleanupDownloadStep: React.FC<CleanupDownloadStepProps> = ({
  onComplete,
}) => {
  const { t } = useTranslation();
  const { getSetting } = useSettings();
  const {
    hardware,
    models,
    downloadProgress,
    initialize,
    downloadModel,
    cancelDownload,
  } = useLlmEngineStore();
  const [failed, setFailed] = useState(false);
  const startedRef = useRef(false);
  const completedRef = useRef(false);

  useEffect(() => {
    initialize();
  }, [initialize]);

  // The model the engine should run: the explicit setting, or the hardware
  // ladder's recommendation. Decided only once the hardware profile arrived
  // (an empty recommendation = Raw tier = nothing to download).
  const targetModelId = hardware
    ? getSetting("llm_model_id") || hardware.recommended_model_id || ""
    : null;
  const model =
    targetModelId !== null
      ? models.find((m) => m.info.id === targetModelId)
      : undefined;
  const progress = targetModelId ? downloadProgress[targetModelId] : undefined;
  const busy = Boolean(progress) || Boolean(model?.is_downloading);

  const finish = () => {
    if (!completedRef.current) {
      completedRef.current = true;
      onComplete();
    }
  };

  // Raw tier (no model for this hardware) or already downloaded: advance.
  useEffect(() => {
    if (completedRef.current || targetModelId === null) return;
    if (targetModelId === "" || (model?.is_downloaded && !busy)) {
      finish();
    }
  });

  // Auto-start the download once the catalog arrived. downloadModel resolves
  // when the backend command returns (finished, failed, or cancelled); the
  // refreshed store tells us which.
  useEffect(() => {
    if (!model || model.is_downloaded || busy || failed || startedRef.current) {
      return;
    }
    startedRef.current = true;
    downloadModel(model.info.id).then(() => {
      const fresh = useLlmEngineStore
        .getState()
        .models.find((m) => m.info.id === model.info.id);
      if (!fresh?.is_downloaded) setFailed(true);
    });
  }, [model, busy, failed, downloadModel]);

  const retry = () => {
    startedRef.current = false;
    setFailed(false);
  };

  const cancel = () => {
    if (targetModelId) cancelDownload(targetModelId);
  };

  const percentage = progress?.percentage ?? 0;

  return (
    <div className="motion-stagger h-screen w-screen flex flex-col items-center justify-center p-8 gap-6 text-center">
      <VaporlyWordmark width={160} />

      <div
        className={`flex items-center justify-center w-14 h-14 rounded-full ${
          failed ? "bg-danger-tint" : "bg-accent-tint"
        }`}
      >
        {failed ? (
          <AlertTriangle className="w-7 h-7 text-danger" aria-hidden="true" />
        ) : (
          <Sparkles className="w-7 h-7 text-accent" aria-hidden="true" />
        )}
      </div>

      <div className="space-y-1.5 max-w-md">
        <h1 className="text-xl font-semibold">
          {t("onboarding.cleanupDownload.title")}
        </h1>
        <p className="text-ink-muted">
          {failed
            ? t("onboarding.cleanupDownload.failed")
            : t("onboarding.cleanupDownload.description")}
        </p>
      </div>

      {failed ? (
        <Button variant="primary" size="lg" onClick={retry}>
          {t("onboarding.cleanupDownload.retry")}
        </Button>
      ) : (
        <div className="w-full max-w-md flex flex-col gap-2">
          <div
            className="h-2 w-full rounded-full bg-surface-well overflow-hidden"
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
          <span className="text-xs text-ink-subtle tabular-nums">
            {progress
              ? t("onboarding.cleanupDownload.downloading", {
                  percent: percentage.toFixed(0),
                })
              : t("onboarding.cleanupDownload.checking")}
          </span>
          {busy && (
            <Button
              variant="ghost"
              size="md"
              onClick={cancel}
              className="self-center"
            >
              {t("onboarding.cleanupDownload.cancel")}
            </Button>
          )}
        </div>
      )}

      <Button variant="ghost" size="sm" onClick={finish}>
        {t("onboarding.cleanupDownload.skip")}
      </Button>
    </div>
  );
};
