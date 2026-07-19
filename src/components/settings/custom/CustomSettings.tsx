import React from "react";
import { useTranslation } from "react-i18next";
import type { FeatureLevel } from "@/bindings";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { SettingContainer } from "../../ui/SettingContainer";
import { Menu, type MenuOption } from "../../ui/Menu";
import { useSettings } from "../../../hooks/useSettings";
import { CustomWords } from "../CustomWords";
import { CustomPhrases } from "./CustomPhrases";
import { AutoLearnControl } from "./AutoLearnControl";

const LEVELS: FeatureLevel[] = ["off", "light", "medium", "high"];

/**
 * The Custom section: everything you teach Vaporly about YOUR vocabulary.
 * Words (fuzzy auto-correct toward names and jargon), Phrases (say a trigger,
 * it writes your saved text), and Auto-learn (Vaporly grows the word list
 * from your corrections).
 */
export const CustomSettings: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();

  const level = (getSetting("custom_words_level") ?? "medium") as FeatureLevel;
  const phrasesLevel = (getSetting("custom_phrases_level") ??
    "medium") as FeatureLevel;
  const levelOptions: MenuOption[] = LEVELS.map((value) => ({
    value,
    label: t(`common.levels.${value}`),
  }));

  return (
    <div className="max-w-3xl w-full mx-auto space-y-8">
      <SettingsGroup title={t("settings.custom.groups.words")}>
        <CustomWords descriptionMode="tooltip" grouped />
        <SettingContainer
          title={t("settings.custom.words.level.title")}
          description={t("settings.custom.words.level.description")}
          descriptionMode="tooltip"
          grouped={true}
        >
          <Menu
            className="w-40 [&>button]:min-w-0"
            options={levelOptions}
            selectedValue={level}
            onSelect={(value) =>
              updateSetting("custom_words_level", value as FeatureLevel)
            }
            disabled={isUpdating("custom_words_level")}
          />
        </SettingContainer>
      </SettingsGroup>
      <SettingsGroup title={t("settings.custom.groups.phrases")}>
        <CustomPhrases />
        <SettingContainer
          title={t("settings.custom.phrases.level.title")}
          description={t("settings.custom.phrases.level.description")}
          descriptionMode="tooltip"
          grouped={true}
        >
          <Menu
            className="w-40 [&>button]:min-w-0"
            options={levelOptions}
            selectedValue={phrasesLevel}
            onSelect={(value) =>
              updateSetting("custom_phrases_level", value as FeatureLevel)
            }
            disabled={isUpdating("custom_phrases_level")}
          />
        </SettingContainer>
      </SettingsGroup>
      <SettingsGroup title={t("settings.custom.groups.autoLearn")}>
        <AutoLearnControl />
      </SettingsGroup>
    </div>
  );
};
