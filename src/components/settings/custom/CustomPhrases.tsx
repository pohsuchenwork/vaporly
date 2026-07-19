import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Trash2 } from "lucide-react";
import type { CustomPhrase } from "@/bindings";
import { Dialog, SettingContainer } from "@/components/ui";
import { Button } from "../../ui/Button";
import { IconButton } from "../../ui/IconButton";
import { Input } from "../../ui/Input";
import { Textarea } from "../../ui/Textarea";
import { useSettings } from "../../../hooks/useSettings";
import { useArmedConfirm } from "../../../hooks/useArmedConfirm";

const SAY_MAX = 100;
const WRITE_MAX = 10000;
/** Quiet period after the last keystroke before an edit is persisted. */
const PERSIST_DEBOUNCE_MS = 400;
const stripControl = (v: string, keepNewlines: boolean) =>
  keepNewlines
    ? v.replace(/[\u0000-\u0008\u000B-\u001F\u007F]/g, "")
    : v.replace(/[\u0000-\u001F\u007F]/g, "");

interface RowProps {
  row: CustomPhrase;
  onEdit: (patch: Partial<CustomPhrase>) => void;
  onBlur: () => void;
  onDelete: () => void;
}

const PhraseRow: React.FC<RowProps> = ({ row, onEdit, onBlur, onDelete }) => {
  const { t } = useTranslation();
  const { armed, fire } = useArmedConfirm(onDelete);
  return (
    <div className="flex gap-2 items-start">
      <Input
        type="text"
        value={row.say}
        variant="compact"
        maxLength={SAY_MAX}
        placeholder={t("settings.custom.phrases.sayPlaceholder")}
        aria-label={t("settings.custom.phrases.sayPlaceholder")}
        onChange={(e) => onEdit({ say: stripControl(e.target.value, false) })}
        onBlur={onBlur}
        className="w-44 shrink-0"
      />
      <Textarea
        value={row.write}
        maxLength={WRITE_MAX}
        rows={row.write.includes("\n") ? 3 : 1}
        placeholder={t("settings.custom.phrases.writePlaceholder")}
        aria-label={t("settings.custom.phrases.writePlaceholder")}
        onChange={(e) => onEdit({ write: stripControl(e.target.value, true) })}
        onBlur={onBlur}
        className="flex-1"
      />
      <IconButton
        onClick={fire}
        variant="danger"
        armed={armed}
        aria-label={t("settings.custom.phrases.delete")}
        title={
          armed
            ? t("settings.custom.phrases.confirmDelete")
            : t("settings.custom.phrases.delete")
        }
        className="rounded-control bg-surface-well"
      >
        <Trash2 className="size-4" />
      </IconButton>
    </div>
  );
};

/**
 * Custom Phrases: say a trigger, Vaporly writes the saved text instead.
 * "btw" becomes "by the way"; "write my email format" inserts a template.
 * Applied deterministically after transcription (works with cleanup off) and
 * listed for the cleanup LLM via the prompts' custom-phrases block.
 *
 * Persistence: every edit persists after a short debounce (so "type a
 * phrase, hit the hotkey" works without ever clicking elsewhere), blur
 * flushes immediately, delete is immediate. The adopt-external effect skips
 * our own persist echoing back, so an in-progress incomplete row survives.
 */
export const CustomPhrases: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting } = useSettings();
  const [open, setOpen] = useState(false);
  const saved =
    (getSetting("custom_phrases") as CustomPhrase[] | undefined) ?? [];
  const [rows, setRows] = useState<CustomPhrase[]>(saved);
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  const debounceRef = useRef<number | null>(null);
  // JSON of the filtered array we last wrote: the self-echo marker for the
  // adopt-external effect below.
  const lastWrittenRef = useRef<string | null>(null);

  const cancelPending = () => {
    if (debounceRef.current !== null) {
      window.clearTimeout(debounceRef.current);
      debounceRef.current = null;
    }
  };

  const write = (next: CustomPhrase[]) => {
    const filtered = next.filter((p) => p.say.trim() && p.write.trim());
    lastWrittenRef.current = JSON.stringify(filtered);
    updateSetting("custom_phrases", filtered);
  };

  // Adopt external changes (fresh load, auto-learn, another window) when the
  // store's value is not just our own persist coming back: adopting the
  // filtered echo would wipe a row the user is still filling in.
  const savedKey = JSON.stringify(saved);
  useEffect(() => {
    if (savedKey === lastWrittenRef.current) {
      return;
    }
    cancelPending();
    setRows(JSON.parse(savedKey));
  }, [savedKey]);

  // Flush a pending debounced edit if the component unmounts mid-typing.
  useEffect(() => {
    return () => {
      if (debounceRef.current !== null) {
        window.clearTimeout(debounceRef.current);
        write(rowsRef.current);
      }
    };
  }, []);

  const persist = (next: CustomPhrase[]) => {
    cancelPending();
    setRows(next);
    write(next);
  };

  const edit = (i: number, patch: Partial<CustomPhrase>) => {
    const next = rows.map((r, j) => (j === i ? { ...r, ...patch } : r));
    setRows(next);
    cancelPending();
    debounceRef.current = window.setTimeout(() => {
      debounceRef.current = null;
      write(rowsRef.current);
    }, PERSIST_DEBOUNCE_MS);
  };

  return (
    <SettingContainer
      title={t("settings.custom.phrases.title")}
      description={t("settings.custom.phrases.description")}
      descriptionMode="tooltip"
      grouped={true}
    >
      <Button variant="secondary" size="sm" onClick={() => setOpen(true)}>
        {saved.length > 0
          ? t("settings.custom.phrases.manage", { count: saved.length })
          : t("settings.custom.phrases.manageEmpty")}
      </Button>
      <Dialog
        open={open}
        onOpenChange={setOpen}
        title={t("settings.custom.phrases.title")}
        description={t("settings.custom.phrases.description")}
        closeLabel={t("common.close")}
      >
        <div className="space-y-3">
          {rows.length === 0 && (
            <p className="text-sm text-ink-subtle">
              {t("settings.custom.phrases.empty")}
            </p>
          )}
          {rows.map((row, i) => (
            <PhraseRow
              key={i}
              row={row}
              onEdit={(patch) => edit(i, patch)}
              onBlur={() => persist(rows)}
              onDelete={() => persist(rows.filter((_, j) => j !== i))}
            />
          ))}
          <Button
            onClick={() => setRows([...rows, { say: "", write: "" }])}
            variant="secondary"
            size="sm"
          >
            <Plus className="size-4" aria-hidden="true" />
            {t("settings.custom.phrases.add")}
          </Button>
          <p className="text-xs text-ink-subtle">
            {t("settings.custom.phrases.hint")}
          </p>
        </div>
      </Dialog>
    </SettingContainer>
  );
};
