import React, { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Download, RefreshCw, Play } from "lucide-react";
import { SettingContainer } from "../../ui/SettingContainer";
import { Button } from "../../ui/Button";
import { useLlmEngineStore } from "../../../stores/llmEngineStore";
import { useSettings } from "../../../hooks/useSettings";
import type { EngineState } from "@/bindings";

const STATE_STYLE: Record<EngineState, string> = {
  ready: "bg-success",
  spawning: "bg-warning animate-pulse",
  loading_model: "bg-warning animate-pulse",
  installing: "bg-warning animate-pulse",
  restarting: "bg-warning animate-pulse",
  model_missing: "bg-warning",
  not_installed: "bg-danger",
  crashed: "bg-danger",
  disabled: "bg-ink-subtle",
  stopped: "bg-ink-subtle",
};

/**
 * Live status chip + Repair/Test controls for the bundled cleanup engine,
 * plus the lazy download path: when a stage wants the model but it was never
 * downloaded (model_missing), a one-click download with progress appears.
 */
export const EngineStatusCard: React.FC = () => {
  const { t } = useTranslation();
  const {
    status,
    hardware,
    models,
    downloadProgress,
    initialize,
    downloadModel,
    repair,
    selftest,
  } = useLlmEngineStore();
  const { getSetting } = useSettings();
  const [testing, setTesting] = useState(false);

  useEffect(() => {
    initialize();
  }, [initialize]);

  const state = status?.state ?? "stopped";

  // The model the engine should run: the explicit setting, or the hardware
  // ladder's recommendation when the setting is empty.
  const targetModelId =
    getSetting("llm_model_id") || hardware?.recommended_model_id || "";
  const targetModel = useMemo(
    () => models.find((m) => m.info.id === targetModelId),
    [models, targetModelId],
  );
  const progress = targetModelId ? downloadProgress[targetModelId] : undefined;
  const downloading = Boolean(progress) || targetModel?.is_downloading;

  const runTest = async () => {
    setTesting(true);
    try {
      const out = await selftest();
      toast.success(t("settings.dictation.engine.testOk", { output: out }));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setTesting(false);
    }
  };

  return (
    <>
      <SettingContainer
        title={t("settings.dictation.engine.status.title")}
        description={t("settings.dictation.engine.status.description")}
        descriptionMode="tooltip"
        layout="horizontal"
        grouped={true}
      >
        <div className="flex items-center gap-3">
          <span className="flex items-center gap-2 text-sm">
            <span
              className={`inline-block w-2.5 h-2.5 rounded-full ${STATE_STYLE[state]}`}
            />
            {t(`settings.dictation.engine.state.${state}`)}
          </span>
          {status?.engine_version ? (
            // eslint-disable-next-line i18next/no-literal-string
            <span className="text-xs text-ink-muted font-mono">
              llama.cpp {status.engine_version}
            </span>
          ) : null}
          {(state === "crashed" ||
            state === "not_installed" ||
            state === "stopped") && (
            <Button variant="secondary" size="sm" onClick={repair}>
              <RefreshCw className="size-4" />
              {t("settings.dictation.engine.repair")}
            </Button>
          )}
          <Button
            variant="ghost"
            size="sm"
            onClick={runTest}
            disabled={testing || state !== "ready"}
          >
            <Play className="size-4" />
            {testing
              ? t("settings.dictation.engine.testing")
              : t("settings.dictation.engine.test")}
          </Button>
        </div>
      </SettingContainer>
      {state === "model_missing" && targetModel && (
        <div className="mx-4 py-3">
          {downloading ? (
            <div className="flex flex-col gap-2">
              <div className="h-1.5 w-full rounded-full bg-surface-well overflow-hidden">
                <div
                  className="h-full rounded-full bg-accent"
                  style={{
                    transition: "width 220ms ease-out",
                    width: `${(progress?.percentage ?? 0).toFixed(1)}%`,
                  }}
                />
              </div>
              <span className="text-xs text-ink-muted">
                {t("settings.dictation.engine.downloading", {
                  percent: (progress?.percentage ?? 0).toFixed(0),
                })}
              </span>
            </div>
          ) : (
            <Button
              variant="secondary"
              size="sm"
              onClick={() => downloadModel(targetModel.info.id)}
            >
              <Download className="size-4" />
              {t("settings.dictation.engine.download", {
                size: (targetModel.info.total_bytes / 1024 ** 3).toFixed(1),
              })}
            </Button>
          )}
        </div>
      )}
      {status?.detail ? (
        <div className="mx-4 py-3 text-xs text-ink-muted font-mono select-text">
          {status.detail}
        </div>
      ) : null}
    </>
  );
};
