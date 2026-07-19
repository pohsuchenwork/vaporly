import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { useSettings } from "../../hooks/useSettings";
import { Input } from "../ui/Input";
import { Button } from "../ui/Button";
import { Chip } from "../ui/Chip";
import { Dialog } from "../ui/Dialog";
import { SettingContainer } from "../ui/SettingContainer";

interface CustomWordsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const CustomWords: React.FC<CustomWordsProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const [newWord, setNewWord] = useState("");
    const [open, setOpen] = useState(false);
    const customWords = getSetting("custom_words") || [];

    const handleAddWord = () => {
      const trimmedWord = newWord.trim();
      const sanitizedWord = trimmedWord.replace(/[<>"'&]/g, "");
      if (
        sanitizedWord &&
        !sanitizedWord.includes(" ") &&
        sanitizedWord.length <= 50
      ) {
        if (customWords.includes(sanitizedWord)) {
          toast.error(
            t("settings.custom.words.duplicate", { word: sanitizedWord }),
          );
          return;
        }
        updateSetting("custom_words", [...customWords, sanitizedWord]);
        setNewWord("");
      }
    };

    const handleRemoveWord = (wordToRemove: string) => {
      updateSetting(
        "custom_words",
        customWords.filter((word) => word !== wordToRemove),
      );
    };

    const handleKeyPress = (e: React.KeyboardEvent) => {
      if (e.key === "Enter") {
        e.preventDefault();
        handleAddWord();
      }
    };

    return (
      <SettingContainer
        title={t("settings.custom.words.title")}
        description={t("settings.custom.words.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      >
        <Button variant="secondary" size="sm" onClick={() => setOpen(true)}>
          {customWords.length > 0
            ? t("settings.custom.words.manage", { count: customWords.length })
            : t("settings.custom.words.manageEmpty")}
        </Button>
        <Dialog
          open={open}
          onOpenChange={setOpen}
          title={t("settings.custom.words.title")}
          description={t("settings.custom.words.description")}
          closeLabel={t("common.close")}
        >
          <div className="space-y-3">
            <div className="flex items-center gap-2">
              <Input
                type="text"
                className="flex-1"
                value={newWord}
                onChange={(e) => setNewWord(e.target.value)}
                onKeyDown={handleKeyPress}
                placeholder={t("settings.custom.words.placeholder")}
                variant="compact"
                disabled={isUpdating("custom_words")}
              />
              <Button
                onClick={handleAddWord}
                disabled={
                  !newWord.trim() ||
                  newWord.includes(" ") ||
                  newWord.trim().length > 50 ||
                  isUpdating("custom_words")
                }
                variant="secondary"
                size="sm"
              >
                {t("settings.custom.words.add")}
              </Button>
            </div>
            {customWords.length > 0 ? (
              <div className="flex flex-wrap gap-2">
                {customWords.map((word) => (
                  <Chip
                    key={word}
                    mode="removable"
                    disabled={isUpdating("custom_words")}
                    removeLabel={t("settings.custom.words.remove", { word })}
                    onRemove={() => handleRemoveWord(word)}
                  >
                    {word}
                  </Chip>
                ))}
              </div>
            ) : (
              <p className="text-sm text-ink-subtle">
                {t("settings.custom.words.empty")}
              </p>
            )}
          </div>
        </Dialog>
      </SettingContainer>
    );
  },
);
