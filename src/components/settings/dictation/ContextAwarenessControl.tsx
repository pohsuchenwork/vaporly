import React from "react";
import { useTranslation } from "react-i18next";
import type { ContextAwarenessSettings, ContextMode } from "@/bindings";
import { SettingContainer } from "../../ui/SettingContainer";
import { Menu, type MenuOption } from "../../ui/Menu";
import { Chip } from "../../ui/Chip";
import { useSettings } from "../../../hooks/useSettings";

const CATEGORIES = [
  "email",
  "chat",
  "code",
  "browser",
  "notes",
  "general",
] as const;

const DEFAULT_CONTEXT: ContextAwarenessSettings = {
  email: true,
  chat: true,
  code: true,
  browser: true,
  notes: true,
  general: true,
  mode: "deterministic",
};

/**
 * Six app-category toggle chips plus a mode select, all writing the single
 * context_awareness settings object.
 */
export const ContextAwarenessControl: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();

  const context = getSetting("context_awareness") ?? DEFAULT_CONTEXT;
  const busy = isUpdating("context_awareness");

  const toggleCategory = (key: (typeof CATEGORIES)[number]) => {
    updateSetting("context_awareness", { ...context, [key]: !context[key] });
  };

  const setMode = (mode: ContextMode) => {
    updateSetting("context_awareness", { ...context, mode });
  };

  const modeOptions: MenuOption[] = [
    {
      value: "deterministic",
      label: t("settings.dictation.context.mode.deterministic"),
    },
    { value: "model", label: t("settings.dictation.context.mode.model") },
    { value: "both", label: t("settings.dictation.context.mode.both") },
  ];

  return (
    <SettingContainer
      title={t("settings.dictation.context.label")}
      description={t("settings.dictation.context.description")}
      descriptionMode="tooltip"
      grouped={true}
      layout="stacked"
    >
      <div className="flex flex-col gap-3">
        <div className="flex flex-wrap gap-2">
          {CATEGORIES.map((category) => (
            <Chip
              key={category}
              mode="toggle"
              pressed={context[category]}
              disabled={busy}
              onToggle={() => toggleCategory(category)}
            >
              {t(`settings.dictation.context.chips.${category}`)}
            </Chip>
          ))}
        </div>
        <div className="flex items-center justify-between gap-3">
          <span className="text-xs text-ink-muted">
            {t("settings.dictation.context.mode.label")}
          </span>
          <Menu
            className="w-40 [&>button]:min-w-0"
            options={modeOptions}
            selectedValue={context.mode}
            onSelect={(value) => setMode(value as ContextMode)}
            disabled={busy}
          />
        </div>
      </div>
    </SettingContainer>
  );
};
