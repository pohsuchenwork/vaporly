import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Download } from "lucide-react";
import VaporlyWordmark from "../icons/VaporlyWordmark";
import { Button } from "../ui/Button";
import { useModelStore } from "../../stores/modelStore";

interface SttDownloadStepProps {
  onComplete: () => void;
}

/**
 * Onboarding: fetch the one fixed speech model. No picking, no sizes table:
 * if the model is already on disk we advance instantly, otherwise the
 * download starts by itself and the step ends the moment it lands.
 */
export const SttDownloadStep: React.FC<SttDownloadStepProps> = ({
  onComplete,
}) => {
  const { t } = useTranslation();
  const {
    models,
    initialize,
    downloadModel,
    cancelDownload,
    downloadingModels,
    verifyingModels,
    extractingModels,
    downloadProgress,
    downloadStats,
  } = useModelStore();
  const [failed, setFailed] = useState(false);
  const startedRef = useRef(false);
  const completedRef = useRef(false);

  useEffect(() => {
    initialize();
  }, [initialize]);

  // The catalog holds exactly one model in v2; it is the model.
  const model = models[0];
  const busy =
    !!model &&
    (model.id in downloadingModels ||
      model.id in verifyingModels ||
      model.id in extractingModels);
  const progress = model ? downloadProgress[model.id] : undefined;
  const speed = model ? downloadStats[model.id]?.speed : undefined;
  const preparing =
    !!model && (model.id in verifyingModels || model.id in extractingModels);

  // Already downloaded (now or after this download finishes): advance.
  useEffect(() => {
    if (!model || completedRef.current) return;
    if (model.is_downloaded && !busy) {
      completedRef.current = true;
      onComplete();
    }
  }, [model, busy, onComplete]);

  // Auto-start the download once the catalog arrives. downloadModel resolves
  // when the download finishes or fails (including cancellation).
  useEffect(() => {
    if (!model || model.is_downloaded || busy || failed || startedRef.current) {
      return;
    }
    startedRef.current = true;
    downloadModel(model.id).then((ok) => {
      if (!ok) setFailed(true);
    });
  }, [model, busy, failed, downloadModel]);

  const retry = () => {
    startedRef.current = false;
    setFailed(false);
  };

  const cancel = () => {
    if (model) cancelDownload(model.id);
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
          <Download className="w-7 h-7 text-accent" aria-hidden="true" />
        )}
      </div>

      <div className="space-y-1.5 max-w-md">
        <h1 className="text-xl font-semibold">
          {t("onboarding.sttDownload.title")}
        </h1>
        <p className="text-ink-muted">
          {failed
            ? t("onboarding.sttDownload.failed")
            : t("onboarding.sttDownload.description")}
        </p>
      </div>

      {failed ? (
        <Button variant="primary" size="lg" onClick={retry}>
          {t("onboarding.sttDownload.retry")}
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
          <div className="flex items-center justify-between text-xs text-ink-subtle tabular-nums">
            <span>
              {preparing
                ? t("onboarding.sttDownload.preparing")
                : progress
                  ? t("onboarding.sttDownload.downloading", {
                      percent: percentage.toFixed(0),
                    })
                  : t("onboarding.sttDownload.checking")}
            </span>
            {!preparing && speed !== undefined && speed > 0 && (
              // eslint-disable-next-line i18next/no-literal-string
              <span>{speed.toFixed(1)} MB/s</span>
            )}
          </div>
          {busy && !preparing && (
            <Button
              variant="ghost"
              size="md"
              onClick={cancel}
              className="self-center"
            >
              {t("onboarding.sttDownload.cancel")}
            </Button>
          )}
        </div>
      )}
    </div>
  );
};
