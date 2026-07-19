import React from "react";
import { useTranslation } from "react-i18next";
import type { FeatureLevel, StageEngine } from "@/bindings";
import { SettingContainer } from "../../ui/SettingContainer";
import {
  SegmentedControl,
  type SegmentedOption,
} from "../../ui/SegmentedControl";
import { Menu, type MenuOption } from "../../ui/Menu";

const LEVELS: FeatureLevel[] = ["off", "light", "medium", "high"];

interface StageLevelControlProps {
  label: string;
  description: string;
  level: FeatureLevel;
  onLevelChange: (level: FeatureLevel) => void;
  engine: StageEngine;
  onEngineChange: (engine: StageEngine) => void;
  disabled?: boolean;
}

/**
 * One cleanup stage as a labeled row: a 4-way Off/Light/Medium/High segmented
 * control plus a small Deterministic/Model engine select. Shared by the
 * Filler fix up and Mind-change check rows on the Dictation page.
 */
export const StageLevelControl: React.FC<StageLevelControlProps> = ({
  label,
  description,
  level,
  onLevelChange,
  engine,
  onEngineChange,
  disabled = false,
}) => {
  const { t } = useTranslation();

  const levelOptions: SegmentedOption<FeatureLevel>[] = LEVELS.map((l) => ({
    value: l,
    label: t(`common.levels.${l}`),
  }));

  const engineOptions: MenuOption[] = [
    {
      value: "deterministic",
      label: t("settings.dictation.stage.engine.deterministic"),
    },
    { value: "model", label: t("settings.dictation.stage.engine.model") },
  ];

  return (
    <SettingContainer
      title={label}
      description={description}
      descriptionMode="tooltip"
      grouped={true}
      layout="stacked"
      disabled={disabled}
    >
      <div className="flex items-center justify-between gap-3 flex-wrap">
        <SegmentedControl
          options={levelOptions}
          value={level}
          onChange={onLevelChange}
          disabled={disabled}
          aria-label={label}
        />
        <Menu
          className="w-40 [&>button]:min-w-0"
          options={engineOptions}
          selectedValue={engine}
          onSelect={(value) => onEngineChange(value as StageEngine)}
          disabled={disabled || level === "off"}
        />
      </div>
    </SettingContainer>
  );
};
