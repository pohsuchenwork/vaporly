import React, { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { type } from "@tauri-apps/plugin-os";
import { AlertTriangle, Globe } from "lucide-react";
import type { FeatureLevel, StageEngine, WhisperStrength } from "@/bindings";
import { ShortcutInput } from "../ShortcutInput";
import { Notice } from "../../ui/Notice";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { SettingContainer } from "../../ui/SettingContainer";
import { ToggleSwitch } from "../../ui/ToggleSwitch";
import { Menu, type MenuOption } from "../../ui/Menu";
import { useSettings } from "../../../hooks/useSettings";
import { useOsType } from "../../../hooks/useOsType";
import { formatKeyCombination } from "../../../lib/utils/keyboard";
import { OverlayStylePicker } from "./OverlayStylePicker";
import { StageLevelControl } from "./StageLevelControl";
import { ContextAwarenessControl } from "./ContextAwarenessControl";
import { EngineStatusCard } from "./EngineStatusCard";
import { WhisperCalibrationControl } from "./WhisperCalibrationControl";

/**
 * The Dictation page: the hotkey, the overlay you see while speaking, and the
 * cleanup dials that decide how your words are polished before pasting.
 */
export const DictationSettings: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const osType = useOsType();
  const isLinux = type() === "linux";
  const isMac = type() === "macos";

  const bindings = getSetting("bindings") || {};
  const globeDismissed = getSetting("globe_key_notice_dismissed");
  const whisperMode = getSetting("whisper_mode") || false;
  const whisperStrength = (getSetting("whisper_strength") ??
    "medium") as WhisperStrength;
  const whisperStrengthOptions: MenuOption[] = (
    ["light", "medium", "high"] as WhisperStrength[]
  ).map((value) => ({
    value,
    label: t(`common.levels.${value}`),
  }));
  // macOS maps a bare globe/fn tap to system actions (emoji picker by
  // default); show a one-time pointer whenever any binding uses fn.
  const anyFnBinding = useMemo(
    () =>
      Object.values(bindings).some((b) =>
        (b?.current_binding || "")
          .toLowerCase()
          .split("+")
          .some((part) => part.trim() === "fn" || part.trim() === "function"),
      ),
    [bindings],
  );
  const showGlobeNotice = isMac && anyFnBinding && !globeDismissed;

  // Group binding ids by their normalized key combination; any group with more
  // than one action is a conflict (the OS delivers the hotkey to only one).
  const conflicts = useMemo(() => {
    const byKeys = new Map<string, string[]>();
    for (const [id, b] of Object.entries(bindings)) {
      const key = (b?.current_binding || "").trim().toLowerCase();
      if (!key) continue;
      const list = byKeys.get(key) ?? [];
      list.push(id);
      byKeys.set(key, list);
    }
    return [...byKeys.entries()]
      .filter(([, ids]) => ids.length > 1)
      .map(([key, ids]) => ({
        keys: formatKeyCombination(key, osType),
        actions: ids.map((id) =>
          t(`settings.general.shortcut.bindings.${id}.name`, {
            defaultValue: bindings[id]?.name || id,
          }),
        ),
      }));
  }, [bindings, osType, t]);

  const fillerLevel = (getSetting("filler_level") ?? "medium") as FeatureLevel;
  const fillerEngine = (getSetting("filler_engine") ??
    "deterministic") as StageEngine;
  const mindChangeLevel = (getSetting("mind_change_level") ??
    "medium") as FeatureLevel;
  const mindChangeEngine = (getSetting("mind_change_engine") ??
    "deterministic") as StageEngine;
  const contextMode = getSetting("context_awareness")?.mode ?? "deterministic";

  // The local engine only matters once some stage actually asks for it.
  const modelNeeded =
    (fillerEngine === "model" && fillerLevel !== "off") ||
    (mindChangeEngine === "model" && mindChangeLevel !== "off") ||
    contextMode === "model" ||
    contextMode === "both";

  return (
    <div className="max-w-3xl w-full mx-auto space-y-8">
      {conflicts.length > 0 && (
        <div className="space-y-2">
          {conflicts.map((c) => (
            <Notice
              key={c.keys}
              tone="warning"
              icon={<AlertTriangle className="w-4 h-4" />}
            >
              <span className="font-semibold">
                {t("settings.dictation.conflict.title")}
              </span>
              <span className="text-ink-muted">
                {", "}
                {t("settings.dictation.conflict.description", {
                  keys: c.keys,
                  actions: c.actions.join(", "),
                })}
              </span>
            </Notice>
          ))}
        </div>
      )}

      <SettingsGroup title={t("settings.dictation.groups.hotkey")}>
        <ShortcutInput shortcutId="transcribe" grouped={true} />
        <ShortcutInput shortcutId="hands_free" grouped={true} />
        {/* Cancel is hidden on Linux (dynamic-shortcut instability). */}
        {!isLinux && <ShortcutInput shortcutId="cancel" grouped={true} />}
        <div className="mx-4 py-3 text-xs text-ink-muted">
          {t("settings.dictation.hotkeyHint")}
        </div>
      </SettingsGroup>

      <SettingsGroup title={t("settings.dictation.groups.whisper")}>
        <ToggleSwitch
          checked={whisperMode}
          onChange={(enabled) => updateSetting("whisper_mode", enabled)}
          isUpdating={isUpdating("whisper_mode")}
          label={t("settings.dictation.whisper.mode.title")}
          description={t("settings.dictation.whisper.mode.description")}
          descriptionMode="tooltip"
          grouped={true}
        />
        <SettingContainer
          title={t("settings.dictation.whisper.strength.title")}
          description={t("settings.dictation.whisper.strength.description")}
          descriptionMode="tooltip"
          grouped={true}
          disabled={!whisperMode}
        >
          <Menu
            className="w-40 [&>button]:min-w-0"
            options={whisperStrengthOptions}
            selectedValue={whisperStrength}
            onSelect={(value) =>
              updateSetting("whisper_strength", value as WhisperStrength)
            }
            disabled={!whisperMode || isUpdating("whisper_strength")}
          />
        </SettingContainer>
        <ShortcutInput shortcutId="whisper_toggle" grouped={true} />
        <WhisperCalibrationControl disabled={!whisperMode} />
        <div className="mx-4 py-3 text-xs text-ink-muted">
          {t("settings.dictation.whisper.calibrationHint")}
        </div>
      </SettingsGroup>

      {showGlobeNotice && (
        <Notice
          tone="warning"
          icon={<Globe className="w-4 h-4" />}
          onDismiss={() => updateSetting("globe_key_notice_dismissed", true)}
          dismissLabel={t("settings.dictation.globeNotice.dismiss")}
        >
          <span className="font-semibold">
            {t("settings.dictation.globeNotice.title")}
          </span>
          <span className="text-ink-muted">
            {" "}
            {t("settings.dictation.globeNotice.body")}
          </span>
        </Notice>
      )}

      <SettingsGroup title={t("settings.dictation.groups.overlay")}>
        <OverlayStylePicker />
      </SettingsGroup>

      <SettingsGroup title={t("settings.dictation.groups.cleanup")}>
        <StageLevelControl
          label={t("settings.dictation.stage.filler.label")}
          description={t("settings.dictation.stage.filler.description")}
          level={fillerLevel}
          onLevelChange={(level) => updateSetting("filler_level", level)}
          engine={fillerEngine}
          onEngineChange={(engine) => updateSetting("filler_engine", engine)}
        />
        <StageLevelControl
          label={t("settings.dictation.stage.mindChange.label")}
          description={t("settings.dictation.stage.mindChange.description")}
          level={mindChangeLevel}
          onLevelChange={(level) => updateSetting("mind_change_level", level)}
          engine={mindChangeEngine}
          onEngineChange={(engine) =>
            updateSetting("mind_change_engine", engine)
          }
        />
        <ContextAwarenessControl />
      </SettingsGroup>

      {modelNeeded && (
        <SettingsGroup title={t("settings.dictation.groups.engine")}>
          <EngineStatusCard />
        </SettingsGroup>
      )}
    </div>
  );
};
