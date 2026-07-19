import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface KeepResultOnClipboardProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const KeepResultOnClipboard: React.FC<KeepResultOnClipboardProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const enabled = getSetting("keep_result_on_clipboard") ?? false;

    return (
      <ToggleSwitch
        checked={enabled}
        onChange={(value) => updateSetting("keep_result_on_clipboard", value)}
        isUpdating={isUpdating("keep_result_on_clipboard")}
        label={t("settings.general.behavior.keepClipboard.label")}
        description={t("settings.general.behavior.keepClipboard.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      />
    );
  });
