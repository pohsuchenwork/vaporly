import React from "react";
import { useTranslation } from "react-i18next";
import type { AutoLearnMode } from "@/bindings";
import { SettingContainer } from "../../ui/SettingContainer";
import { Menu, type MenuOption } from "../../ui/Menu";
import { useSettings } from "../../../hooks/useSettings";

const MODES: AutoLearnMode[] = [
  "off",
  "history_edits",
  "repeated_words",
  "both",
  "watch_post_paste",
];

/**
 * Auto-learn mode select. The learning engine itself lands in a later build;
 * the chosen mode persists now so it is ready the moment it does.
 */
export const AutoLearnControl: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();

  const mode = (getSetting("auto_learn_mode") ?? "off") as AutoLearnMode;

  const options: MenuOption[] = MODES.map((value) => ({
    value,
    label: t(`settings.custom.autoLearn.modes.${value}.label`),
  }));

  return (
    <>
      <SettingContainer
        title={t("settings.custom.autoLearn.title")}
        description={t(`settings.custom.autoLearn.modes.${mode}.description`)}
        descriptionMode="inline"
        grouped={true}
      >
        <Menu
          options={options}
          selectedValue={mode}
          onSelect={(value) =>
            updateSetting("auto_learn_mode", value as AutoLearnMode)
          }
          disabled={isUpdating("auto_learn_mode")}
        />
      </SettingContainer>
      {mode !== "off" && (
        <div className="mx-4 py-3 text-xs text-ink-muted">
          {t("settings.custom.autoLearn.note")}
        </div>
      )}
    </>
  );
};
