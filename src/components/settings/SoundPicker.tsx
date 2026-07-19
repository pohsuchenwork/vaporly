import React from "react";
import type { SoundTheme } from "@/bindings";
import { IconButton } from "../ui/IconButton";
import { Menu, type MenuOption } from "../ui/Menu";
import { PlayIcon } from "lucide-react";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettingsStore } from "../../stores/settingsStore";
import { useSettings } from "../../hooks/useSettings";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

interface SoundPickerProps {
  label: string;
  description: string;
}

export const SoundPicker: React.FC<SoundPickerProps> = ({
  label,
  description,
}) => {
  const { getSetting, updateSetting } = useSettings();
  const { t } = useTranslation();
  const playTestSound = useSettingsStore((state) => state.playTestSound);
  const customSounds = useSettingsStore((state) => state.customSounds);

  const selectedTheme = getSetting("sound_theme") ?? "marimba";

  const options: MenuOption[] = [
    { value: "marimba", label: "Marimba" },
    { value: "pop", label: "Pop" },
    { value: "chime", label: "Chime" },
    { value: "bubble", label: "Bubble" },
    { value: "breeze", label: "Breeze" },
  ];

  // Only add Custom option if both custom sound files exist
  if (customSounds.start && customSounds.stop) {
    options.push({ value: "custom", label: "Custom" });
  }

  const handlePlayBothSounds = async () => {
    try {
      await playTestSound("start");
      await new Promise((resolve) => setTimeout(resolve, 700));
      await playTestSound("stop");
    } catch {
      toast.error(t("settings.sound.previewFailed"));
    }
  };

  return (
    <SettingContainer
      title={label}
      description={description}
      grouped
      layout="horizontal"
    >
      <div className="flex items-center gap-2">
        <Menu
          selectedValue={selectedTheme}
          onSelect={(value) =>
            updateSetting("sound_theme", value as SoundTheme)
          }
          options={options}
        />
        <IconButton
          onClick={handlePlayBothSounds}
          aria-label={t("settings.sound.preview")}
        >
          <PlayIcon className="size-4" />
        </IconButton>
      </div>
    </SettingContainer>
  );
};
