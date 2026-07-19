import React from "react";
import { useTranslation } from "react-i18next";
import type { OverlayPosition, OverlayStyle } from "@/bindings";
import { SettingContainer } from "../../ui/SettingContainer";
import { Menu, type MenuOption } from "../../ui/Menu";
import { useSettings } from "../../../hooks/useSettings";

const STYLES: OverlayStyle[] = [
  "none",
  "bar",
  "bar_live",
  "textbox_raw",
  "textbox_clean",
  "inline",
];

/**
 * Overlay style picker: six styles with a compact all-styles tooltip behind
 * the ? affordance, plus a Top/Bottom position select that goes quiet when
 * the overlay is off.
 */
export const OverlayStylePicker: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();

  const selectedStyle = (getSetting("overlay_style") ??
    "bar_live") as OverlayStyle;
  // Only "top" and "bottom" are selectable; anything else falls back to
  // "bottom" (matches the backend default).
  const selectedPosition: OverlayPosition =
    getSetting("overlay_position") === "top" ? "top" : "bottom";

  const styleOptions: MenuOption[] = STYLES.map((style) => ({
    value: style,
    label: t(`settings.dictation.overlay.style.options.${style}`),
  }));

  const positionOptions: MenuOption[] = [
    {
      value: "top",
      label: t("settings.dictation.overlay.position.options.top"),
    },
    {
      value: "bottom",
      label: t("settings.dictation.overlay.position.options.bottom"),
    },
  ];

  return (
    <>
      <SettingContainer
        title={t("settings.dictation.overlay.style.title")}
        description={t("settings.dictation.overlay.style.tooltip")}
        descriptionMode="tooltip"
        grouped={true}
      >
        <Menu
          options={styleOptions}
          selectedValue={selectedStyle}
          onSelect={(value) =>
            updateSetting("overlay_style", value as OverlayStyle)
          }
          disabled={isUpdating("overlay_style")}
        />
      </SettingContainer>

      <SettingContainer
        title={t("settings.dictation.overlay.position.title")}
        description={t("settings.dictation.overlay.position.description")}
        descriptionMode="tooltip"
        grouped={true}
        disabled={selectedStyle === "none"}
      >
        <Menu
          options={positionOptions}
          selectedValue={selectedPosition}
          onSelect={(value) =>
            updateSetting("overlay_position", value as OverlayPosition)
          }
          disabled={selectedStyle === "none" || isUpdating("overlay_position")}
        />
      </SettingContainer>
    </>
  );
};
