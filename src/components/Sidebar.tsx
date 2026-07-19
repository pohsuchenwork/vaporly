import React from "react";
import { useTranslation } from "react-i18next";
import { BookA, History, Info, Keyboard, Palette } from "lucide-react";
import VaporlyWordmark from "./icons/VaporlyWordmark";
import {
  AppearanceSettings,
  CustomSettings,
  DictationSettings,
  GeneralSettings,
  HistorySettings,
} from "./settings";

export type SidebarSection = keyof typeof SECTIONS_CONFIG;

interface IconProps {
  width?: number | string;
  height?: number | string;
  size?: number | string;
  className?: string;
  [key: string]: any;
}

interface SectionConfig {
  labelKey: string;
  icon: React.ComponentType<IconProps>;
  component: React.ComponentType;
}

export const SECTIONS_CONFIG = {
  dictation: {
    labelKey: "sidebar.dictation",
    icon: Keyboard,
    component: DictationSettings,
  },
  custom: {
    labelKey: "sidebar.custom",
    icon: BookA,
    component: CustomSettings,
  },
  history: {
    labelKey: "sidebar.history",
    icon: History,
    component: HistorySettings,
  },
  appearance: {
    labelKey: "sidebar.appearance",
    icon: Palette,
    component: AppearanceSettings,
  },
  general: {
    labelKey: "sidebar.general",
    icon: Info,
    component: GeneralSettings,
  },
} as const satisfies Record<string, SectionConfig>;

const SECTIONS = Object.entries(SECTIONS_CONFIG).map(([id, config]) => ({
  id: id as SidebarSection,
  ...config,
}));

interface SidebarProps {
  activeSection: SidebarSection;
  onSectionChange: (section: SidebarSection) => void;
}

export const Sidebar: React.FC<SidebarProps> = ({
  activeSection,
  onSectionChange,
}) => {
  const { t } = useTranslation();

  return (
    <div className="flex flex-col w-40 h-full items-center px-2">
      <VaporlyWordmark width={120} className="m-4" />
      {/* A real <nav> of native <button>s: keyboard operable (Enter/Space +
          natural tab order) and announced as the current page, replacing the
          former mouse-only <div onClick>. The wordmark above is a non-focusable
          SVG, so tab order flows sections then content. */}
      <nav
        aria-label={t("sidebar.navLabel")}
        className="flex flex-col w-full items-center gap-1 pt-2"
      >
        {SECTIONS.map((section) => {
          const Icon = section.icon;
          const isActive = activeSection === section.id;

          return (
            <button
              key={section.id}
              type="button"
              aria-current={isActive ? "page" : undefined}
              onClick={() => onSectionChange(section.id)}
              className={`flex gap-3 items-center px-[18px] py-3 min-h-12 w-full rounded-control cursor-pointer transition-colors focus-ring ${
                isActive
                  ? "bg-surface-selected text-on-selected"
                  : "text-ink-muted hover:text-ink hover:bg-control-ghost-hover"
              }`}
            >
              <Icon width={24} height={24} className="shrink-0" />
              <span
                className="text-sm font-medium truncate"
                title={t(section.labelKey)}
              >
                {t(section.labelKey)}
              </span>
            </button>
          );
        })}
      </nav>
    </div>
  );
};
