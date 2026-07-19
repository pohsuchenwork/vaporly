import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { commands } from "@/bindings";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { SettingContainer } from "../../ui/SettingContainer";
import { Button } from "../../ui/Button";
import { MicrophoneSelector } from "../MicrophoneSelector";
import { OutputDeviceSelector } from "../OutputDeviceSelector";
import { AudioFeedback } from "../AudioFeedback";
import { VolumeSlider } from "../VolumeSlider";
import { SoundPicker } from "../SoundPicker";
import { AutostartToggle } from "../AutostartToggle";
import { KeepResultOnClipboard } from "../KeepResultOnClipboard";
import { AppendTrailingSpace } from "../AppendTrailingSpace";
import { UpdateChecksToggle } from "../UpdateChecksToggle";
import { LogDirectory } from "../LogDirectory";
import UpdateChecker from "../../update-checker";
import { useSettings } from "../../../hooks/useSettings";
import { useSettingsStore } from "../../../stores/settingsStore";
import { useArmedConfirm } from "../../../hooks/useArmedConfirm";

const REPO_URL = "https://github.com/pohsuchenwork/vaporly";

/**
 * The General page: microphone, feedback sounds, paste behavior, and the
 * app-keeping chores (updates, version, logs, resets).
 */
export const GeneralSettings: React.FC = () => {
  const { t } = useTranslation();
  const { audioFeedbackEnabled } = useSettings();
  const refreshSettings = useSettingsStore((st) => st.refreshSettings);
  const [version, setVersion] = useState("");

  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch((error) => {
        console.error("Failed to get app version:", error);
      });
  }, []);

  const resetAll = useArmedConfirm(async () => {
    try {
      const result = await commands.resetAllSettings();
      if (result.status === "error") throw new Error(result.error);
      await refreshSettings();
      toast.success(t("settings.general.about.danger.resetAllDone"));
    } catch (e) {
      toast.error(String(e));
    }
  }, 4000);

  const resetOnboarding = useArmedConfirm(async () => {
    try {
      const result = await commands.resetOnboarding();
      if (result.status === "error") throw new Error(result.error);
      toast.success(t("settings.general.about.danger.resetOnboardingDone"));
    } catch (e) {
      toast.error(String(e));
    }
  });

  return (
    <div className="max-w-3xl w-full mx-auto space-y-8">
      <SettingsGroup title={t("settings.general.groups.microphone")}>
        <MicrophoneSelector descriptionMode="tooltip" grouped={true} />
      </SettingsGroup>

      <SettingsGroup title={t("settings.general.groups.sounds")}>
        <AudioFeedback descriptionMode="tooltip" grouped={true} />
        <SoundPicker
          label={t("settings.sound.theme.label")}
          description={t("settings.sound.theme.description")}
        />
        <VolumeSlider disabled={!audioFeedbackEnabled} />
        <OutputDeviceSelector
          descriptionMode="tooltip"
          grouped={true}
          disabled={!audioFeedbackEnabled}
        />
      </SettingsGroup>

      <SettingsGroup title={t("settings.general.groups.behavior")}>
        <AutostartToggle descriptionMode="tooltip" grouped={true} />
        <KeepResultOnClipboard descriptionMode="tooltip" grouped={true} />
        <AppendTrailingSpace descriptionMode="tooltip" grouped={true} />
      </SettingsGroup>

      <SettingsGroup title={t("settings.general.groups.about")}>
        <UpdateChecksToggle descriptionMode="tooltip" grouped={true} />

        <SettingContainer
          title={t("settings.general.about.updateStatus.title")}
          description={t("settings.general.about.updateStatus.description")}
          descriptionMode="tooltip"
          grouped={true}
        >
          <div className="text-sm">
            <UpdateChecker />
          </div>
        </SettingContainer>

        <SettingContainer
          title={t("settings.general.about.version.title")}
          description={t("settings.general.about.version.description")}
          descriptionMode="tooltip"
          grouped={true}
        >
          {/* eslint-disable-next-line i18next/no-literal-string */}
          <span className="text-sm font-mono">v{version}</span>
        </SettingContainer>

        <SettingContainer
          title={t("settings.general.about.sourceCode.title")}
          description={t("settings.general.about.sourceCode.description")}
          descriptionMode="tooltip"
          grouped={true}
        >
          <Button
            variant="secondary"
            size="md"
            onClick={() => openUrl(REPO_URL)}
          >
            {t("settings.general.about.sourceCode.button")}
          </Button>
        </SettingContainer>

        <LogDirectory descriptionMode="tooltip" grouped={true} />

        <SettingContainer
          title={t("settings.general.about.danger.resetAllTitle")}
          description={t("settings.general.about.danger.resetAllDescription")}
          descriptionMode="tooltip"
          grouped={true}
        >
          <Button
            variant={resetAll.armed ? "danger" : "secondary"}
            size="md"
            onClick={resetAll.fire}
          >
            {resetAll.armed
              ? t("settings.general.about.danger.resetAllConfirm")
              : t("settings.general.about.danger.resetAllButton")}
          </Button>
        </SettingContainer>

        <SettingContainer
          title={t("settings.general.about.danger.resetOnboardingTitle")}
          description={t(
            "settings.general.about.danger.resetOnboardingDescription",
          )}
          descriptionMode="tooltip"
          grouped={true}
        >
          <Button
            variant={resetOnboarding.armed ? "danger" : "secondary"}
            size="md"
            onClick={resetOnboarding.fire}
          >
            {resetOnboarding.armed
              ? t("common.confirmAgain")
              : t("settings.general.about.danger.resetOnboardingButton")}
          </Button>
        </SettingContainer>
      </SettingsGroup>
    </div>
  );
};
