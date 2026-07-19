import React from "react";
import { useTranslation } from "react-i18next";
import type { AccentPreset, ThemeMode } from "@/bindings";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { SettingContainer } from "../../ui/SettingContainer";
import { SegmentedControl } from "../../ui/SegmentedControl";
import { useSettings } from "../../../hooks/useSettings";
import { PRESETS } from "../../../styles/applyAccent";
import { cn } from "@/lib/utils/cn";

const PRESET_ORDER: AccentPreset[] = [
  "sakura",
  "rose",
  "amber",
  "green",
  "blue",
  "violet",
];
const MODES: ThemeMode[] = ["system", "light", "dark"];

/**
 * The Appearance page: theme mode (follow the OS or force light/dark) and the
 * accent preset. Both apply live to BOTH windows: the main window reacts to
 * the settings store (App.tsx applies applyAppearance), and the overlay
 * listens for the backend's appearance-changed event.
 */
export const AppearanceSettings: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();

  const mode = (getSetting("theme_mode") ?? "system") as ThemeMode;
  const preset = (getSetting("accent_preset") ?? "sakura") as AccentPreset;

  return (
    <div className="max-w-3xl w-full mx-auto space-y-8">
      <SettingsGroup title={t("settings.appearance.groups.theme")}>
        <SettingContainer
          title={t("settings.appearance.mode.title")}
          description={t("settings.appearance.mode.description")}
          descriptionMode="tooltip"
          grouped={true}
        >
          <SegmentedControl
            options={MODES.map((value) => ({
              value,
              label: t(`settings.appearance.mode.options.${value}`),
            }))}
            value={mode}
            onChange={(value) => updateSetting("theme_mode", value)}
            disabled={isUpdating("theme_mode")}
            aria-label={t("settings.appearance.mode.title")}
          />
        </SettingContainer>
      </SettingsGroup>

      <SettingsGroup title={t("settings.appearance.groups.accent")}>
        <SettingContainer
          title={t("settings.appearance.accent.title")}
          description={t("settings.appearance.accent.description")}
          descriptionMode="tooltip"
          grouped={true}
        >
          <div
            role="radiogroup"
            aria-label={t("settings.appearance.accent.title")}
            className="flex items-center gap-2"
          >
            {PRESET_ORDER.map((p) => (
              <button
                key={p}
                type="button"
                role="radio"
                aria-checked={preset === p}
                aria-label={t(`settings.appearance.accent.presets.${p}`)}
                title={t(`settings.appearance.accent.presets.${p}`)}
                onClick={() => updateSetting("accent_preset", p)}
                disabled={isUpdating("accent_preset")}
                className={cn(
                  "size-6 shrink-0 rounded-full cursor-pointer transition-transform focus-ring",
                  preset === p
                    ? "ring-2 ring-focus-ring ring-offset-2 ring-offset-surface-panel"
                    : "hover:scale-110",
                )}
                style={{ backgroundColor: PRESETS[p].light.accent }}
              />
            ))}
          </div>
        </SettingContainer>
      </SettingsGroup>
    </div>
  );
};
