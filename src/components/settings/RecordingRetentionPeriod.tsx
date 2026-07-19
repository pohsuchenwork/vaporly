import React from "react";
import { useTranslation } from "react-i18next";
import { Menu } from "../ui/Menu";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettings } from "../../hooks/useSettings";
import { RecordingRetentionPeriod } from "@/bindings";

interface RecordingRetentionPeriodProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const RecordingRetentionPeriodSelector: React.FC<RecordingRetentionPeriodProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const selectedRetentionPeriod =
      getSetting("recording_retention_period") || "never";
    const historyLimit = getSetting("history_limit") || 5;

    const handleRetentionPeriodSelect = async (period: string) => {
      await updateSetting(
        "recording_retention_period",
        period as RecordingRetentionPeriod,
      );
    };

    const retentionOptions = [
      { value: "never", label: t("settings.history.storage.retention.never") },
      {
        value: "preserve_limit",
        label: t("settings.history.storage.retention.preserveLimit", {
          count: Number(historyLimit),
        }),
      },
      { value: "days3", label: t("settings.history.storage.retention.days3") },
      {
        value: "weeks2",
        label: t("settings.history.storage.retention.weeks2"),
      },
      {
        value: "months3",
        label: t("settings.history.storage.retention.months3"),
      },
    ];

    return (
      <SettingContainer
        title={t("settings.history.storage.retention.title")}
        description={t("settings.history.storage.retention.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      >
        <Menu
          options={retentionOptions}
          selectedValue={selectedRetentionPeriod}
          onSelect={handleRetentionPeriodSelect}
          placeholder={t("settings.history.storage.retention.placeholder")}
          disabled={isUpdating("recording_retention_period")}
        />
      </SettingContainer>
    );
  });

RecordingRetentionPeriodSelector.displayName =
  "RecordingRetentionPeriodSelector";
