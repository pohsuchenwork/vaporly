import React, { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { readFile } from "@tauri-apps/plugin-fs";
import {
  BookPlus,
  Check,
  Copy,
  FolderOpen,
  Pencil,
  RotateCcw,
  Trash2,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { useArmedConfirm } from "../../../hooks/useArmedConfirm";
import {
  commands,
  events,
  type HistoryEntry,
  type HistoryUpdatePayload,
} from "@/bindings";
import { useOsType } from "@/hooks/useOsType";
import { useSettings } from "@/hooks/useSettings";
import { formatDateTime } from "@/utils/dateFormat";
import { AudioPlayer } from "../../ui/AudioPlayer";
import { Button } from "../../ui/Button";
import { Chip } from "../../ui/Chip";
import { IconButton } from "../../ui/IconButton";
import { Input } from "../../ui/Input";
import { Card } from "../../ui/Card";
import { Textarea } from "../../ui/Textarea";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { HistoryLimit } from "../HistoryLimit";
import { RecordingRetentionPeriodSelector } from "../RecordingRetentionPeriod";

const PAGE_SIZE = 12;

interface OpenRecordingsButtonProps {
  onClick: () => void;
  label: string;
}

const OpenRecordingsButton: React.FC<OpenRecordingsButtonProps> = ({
  onClick,
  label,
}) => (
  <Button
    onClick={onClick}
    variant="secondary"
    size="sm"
    className="flex items-center gap-2"
    title={label}
  >
    <FolderOpen className="w-4 h-4" />
    <span>{label}</span>
  </Button>
);

export const HistorySettings: React.FC = () => {
  const { t } = useTranslation();
  const osType = useOsType();
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [hasMore, setHasMore] = useState(true);
  const [query, setQuery] = useState("");
  const [searchResults, setSearchResults] = useState<HistoryEntry[] | null>(
    null,
  );
  const sentinelRef = useRef<HTMLDivElement>(null);
  const entriesRef = useRef<HistoryEntry[]>([]);
  const loadingRef = useRef(false);
  const searching = searchResults !== null;

  // Keep ref in sync for use in IntersectionObserver callback
  useEffect(() => {
    entriesRef.current = entries;
  }, [entries]);

  const loadPage = useCallback(async (cursor?: number) => {
    const isFirstPage = cursor === undefined;
    if (!isFirstPage && loadingRef.current) return;
    loadingRef.current = true;

    if (isFirstPage) setLoading(true);

    try {
      const result = await commands.getHistoryEntries(
        cursor ?? null,
        PAGE_SIZE,
      );
      if (result.status === "ok") {
        const { entries: newEntries, has_more } = result.data;
        setEntries((prev) =>
          isFirstPage ? newEntries : [...prev, ...newEntries],
        );
        setHasMore(has_more);
      }
    } catch (error) {
      console.error("Failed to load history entries:", error);
    } finally {
      setLoading(false);
      loadingRef.current = false;
    }
  }, []);

  // Initial load
  useEffect(() => {
    loadPage();
  }, [loadPage]);

  // Debounced search: non-empty query switches the list to search results;
  // clearing it falls back to the paginated list untouched.
  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setSearchResults(null);
      return;
    }
    const handle = setTimeout(async () => {
      try {
        const result = await commands.searchHistoryEntries(q, 100);
        if (result.status === "ok") setSearchResults(result.data);
      } catch (error) {
        console.error("History search failed:", error);
      }
    }, 250);
    return () => clearTimeout(handle);
  }, [query]);

  // Infinite scroll via IntersectionObserver
  useEffect(() => {
    if (loading) return;

    const sentinel = sentinelRef.current;
    if (!sentinel || !hasMore) return;

    const observer = new IntersectionObserver(
      (observerEntries) => {
        const first = observerEntries[0];
        if (first.isIntersecting) {
          const lastEntry = entriesRef.current[entriesRef.current.length - 1];
          if (lastEntry) {
            loadPage(lastEntry.id);
          }
        }
      },
      { threshold: 0 },
    );

    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [loading, hasMore, loadPage]);

  // Listen for entries added or updated from the transcription pipeline.
  // "updated" also reconciles our optimistic inline edits with what the
  // backend actually persisted.
  useEffect(() => {
    const unlisten = events.historyUpdatePayload.listen((event) => {
      const payload: HistoryUpdatePayload = event.payload;
      if (payload.action === "added") {
        setEntries((prev) => [payload.entry, ...prev]);
      } else if (payload.action === "updated") {
        setEntries((prev) =>
          prev.map((e) => (e.id === payload.entry.id ? payload.entry : e)),
        );
        setSearchResults((prev) =>
          prev
            ? prev.map((e) => (e.id === payload.entry.id ? payload.entry : e))
            : prev,
        );
      }
      // "deleted" is handled by optimistic updates only, so it is
      // intentionally ignored here to avoid double-mutation.
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const copyToClipboard = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
    } catch (error) {
      console.error("Failed to copy to clipboard:", error);
    }
  };

  const getAudioUrl = useCallback(
    async (fileName: string) => {
      try {
        const result = await commands.getAudioFilePath(fileName);
        if (result.status === "ok") {
          if (osType === "linux") {
            const fileData = await readFile(result.data);
            const blob = new Blob([fileData], { type: "audio/wav" });
            return URL.createObjectURL(blob);
          }
          return convertFileSrc(result.data, "asset");
        }
        return null;
      } catch (error) {
        console.error("Failed to get audio file path:", error);
        return null;
      }
    },
    [osType],
  );

  const deleteAudioEntry = async (id: number) => {
    // Optimistically remove
    setEntries((prev) => prev.filter((e) => e.id !== id));
    setSearchResults((prev) => (prev ? prev.filter((e) => e.id !== id) : prev));
    try {
      const result = await commands.deleteHistoryEntry(id);
      if (result.status !== "ok") {
        // Reload on failure
        loadPage();
      }
    } catch (error) {
      console.error("Failed to delete entry:", error);
      loadPage();
    }
  };

  const retryHistoryEntry = async (id: number) => {
    const result = await commands.retryHistoryEntryTranscription(id);
    if (result.status !== "ok") {
      throw new Error(String(result.error));
    }
  };

  // Inline edit: optimistic swap, the backend's "updated" event reconciles.
  const updateEntryText = async (id: number, text: string) => {
    const swap = (list: HistoryEntry[]) =>
      list.map((e) => (e.id === id ? { ...e, transcription_text: text } : e));
    setEntries(swap);
    setSearchResults((prev) => (prev ? swap(prev) : prev));
    const result = await commands.updateHistoryEntryText(id, text);
    if (result.status !== "ok") {
      loadPage();
      throw new Error(String(result.error));
    }
  };

  const openRecordingsFolder = async () => {
    try {
      const result = await commands.openRecordingsFolder();
      if (result.status !== "ok") {
        throw new Error(String(result.error));
      }
    } catch (error) {
      console.error("Failed to open recordings folder:", error);
    }
  };

  const visibleEntries = searching ? searchResults : entries;

  let content: React.ReactNode;

  if (loading && !searching) {
    content = (
      <div className="mx-4 py-3 text-center text-ink-muted">
        {t("settings.history.loading")}
      </div>
    );
  } else if (visibleEntries.length === 0) {
    content = (
      <div className="mx-4 py-3 text-center text-ink-muted">
        {searching
          ? t("settings.history.noResults")
          : t("settings.history.empty")}
      </div>
    );
  } else {
    content = (
      <>
        <div className="divide-y divide-hairline">
          {visibleEntries.map((entry) => (
            <HistoryEntryComponent
              key={entry.id}
              entry={entry}
              onCopyText={() => copyToClipboard(entry.transcription_text)}
              getAudioUrl={getAudioUrl}
              deleteAudio={deleteAudioEntry}
              retryTranscription={retryHistoryEntry}
              saveText={updateEntryText}
            />
          ))}
        </div>
        {/* Sentinel for infinite scroll (paginated list only) */}
        {!searching && <div ref={sentinelRef} className="h-1" />}
      </>
    );
  }

  return (
    <div className="max-w-3xl w-full mx-auto space-y-8">
      <div className="pr-4 flex items-center justify-between gap-3">
        <div>
          <h2 className="text-xs font-medium text-ink-muted uppercase tracking-wide">
            {t("settings.history.title")}
          </h2>
        </div>
        <div className="flex items-center gap-2 flex-1 justify-end">
          <Input
            type="search"
            variant="compact"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("settings.history.searchPlaceholder")}
            aria-label={t("settings.history.searchPlaceholder")}
            className="max-w-56"
          />
          <OpenRecordingsButton
            onClick={openRecordingsFolder}
            label={t("settings.history.openFolder")}
          />
        </div>
      </div>

      <SettingsGroup title={t("settings.history.storage.title")}>
        <HistoryLimit descriptionMode="tooltip" grouped={true} />
        <RecordingRetentionPeriodSelector
          descriptionMode="tooltip"
          grouped={true}
        />
      </SettingsGroup>

      <Card className="overflow-visible">{content}</Card>
    </div>
  );
};

interface HistoryEntryProps {
  entry: HistoryEntry;
  onCopyText: () => void;
  getAudioUrl: (fileName: string) => Promise<string | null>;
  deleteAudio: (id: number) => Promise<void>;
  retryTranscription: (id: number) => Promise<void>;
  saveText: (id: number, text: string) => Promise<void>;
}

const HistoryEntryComponent: React.FC<HistoryEntryProps> = ({
  entry,
  onCopyText,
  getAudioUrl,
  deleteAudio,
  retryTranscription,
  saveText,
}) => {
  const { t, i18n } = useTranslation();
  const [showCopied, setShowCopied] = useState(false);
  const [retrying, setRetrying] = useState(false);
  // Inline editing: pencil swaps the transcript for a textarea with
  // Save/Cancel; Save persists via update_history_entry_text.
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);
  // Select-to-add-to-dictionary: track a short selection inside this entry's
  // transcript and offer a one-click dictionary add for it.
  const [selectedTerm, setSelectedTerm] = useState("");
  const transcriptRef = useRef<HTMLParagraphElement>(null);
  const { settings: dictSettings, updateSetting } = useSettings();

  const handleTranscriptMouseUp = () => {
    const sel = window.getSelection();
    const text = sel?.toString().trim() ?? "";
    const inTranscript =
      !!sel &&
      sel.rangeCount > 0 &&
      !!transcriptRef.current &&
      transcriptRef.current.contains(sel.getRangeAt(0).commonAncestorContainer);
    setSelectedTerm(
      inTranscript && text.length > 0 && text.length <= 60 ? text : "",
    );
  };

  const addSelectionToDictionary = () => {
    if (!selectedTerm) return;
    const words = dictSettings?.custom_words ?? [];
    if (!words.some((w) => w.toLowerCase() === selectedTerm.toLowerCase())) {
      updateSetting("custom_words", [...words, selectedTerm]);
    }
    toast.success(
      t("settings.history.dictionaryAdded", { word: selectedTerm }),
    );
    setSelectedTerm("");
    window.getSelection()?.removeAllRanges();
  };

  const hasTranscription = entry.transcription_text.trim().length > 0;

  const handleLoadAudio = useCallback(
    () => getAudioUrl(entry.file_name),
    [getAudioUrl, entry.file_name],
  );

  const handleCopyText = () => {
    if (!hasTranscription) {
      return;
    }

    onCopyText();
    setShowCopied(true);
    setTimeout(() => setShowCopied(false), 2000);
  };

  const handleDeleteEntry = async () => {
    try {
      await deleteAudio(entry.id);
    } catch (error) {
      console.error("Failed to delete entry:", error);
      toast.error(t("settings.history.deleteError"));
    }
  };
  // Deleting a dictation is destructive: first click arms, second confirms.
  const deleteConfirm = useArmedConfirm(handleDeleteEntry);

  const handleRetranscribe = async () => {
    try {
      setRetrying(true);
      await retryTranscription(entry.id);
    } catch (error) {
      console.error("Failed to re-transcribe:", error);
      toast.error(t("settings.history.retranscribeError"));
    } finally {
      setRetrying(false);
    }
  };

  const startEdit = () => {
    setDraft(entry.transcription_text);
    setSelectedTerm("");
    setEditing(true);
  };

  const saveEdit = async () => {
    const text = draft.trim();
    if (!text || text === entry.transcription_text) {
      setEditing(false);
      return;
    }
    setSaving(true);
    try {
      await saveText(entry.id, text);
      setEditing(false);
    } catch (error) {
      console.error("Failed to save history edit:", error);
      toast.error(t("settings.history.edit.failed"));
    } finally {
      setSaving(false);
    }
  };

  const formattedDate = formatDateTime(String(entry.timestamp), i18n.language);

  return (
    <div className="mx-4 py-3 flex flex-col gap-3">
      <div className="flex justify-between items-center">
        <p className="text-sm font-medium">
          {formattedDate}
          {entry.app_name && (
            <span className="text-ink-subtle font-normal">
              {" "}
              · {entry.app_name}
            </span>
          )}
        </p>
        <div className="flex items-center">
          <IconButton
            onClick={handleCopyText}
            disabled={!hasTranscription || retrying}
            aria-label={t("settings.history.copyToClipboard")}
            title={t("settings.history.copyToClipboard")}
          >
            {showCopied ? (
              <Check width={16} height={16} />
            ) : (
              <Copy width={16} height={16} />
            )}
          </IconButton>
          <IconButton
            onClick={startEdit}
            disabled={!hasTranscription || retrying || editing}
            variant={editing ? "accent" : "default"}
            aria-label={t("settings.history.edit.edit")}
            title={t("settings.history.edit.edit")}
          >
            <Pencil width={16} height={16} />
          </IconButton>
          <IconButton
            onClick={handleRetranscribe}
            disabled={retrying || editing}
            aria-label={t("settings.history.retranscribe")}
            title={t("settings.history.retranscribe")}
          >
            <RotateCcw
              width={16}
              height={16}
              style={
                retrying
                  ? { animation: "spin 1s linear infinite reverse" }
                  : undefined
              }
            />
          </IconButton>
          <IconButton
            onClick={deleteConfirm.fire}
            disabled={retrying}
            variant="danger"
            armed={deleteConfirm.armed}
            aria-label={t("settings.history.delete")}
            title={
              deleteConfirm.armed
                ? t("common.confirmAgain")
                : t("settings.history.delete")
            }
          >
            <Trash2 width={16} height={16} />
          </IconButton>
        </div>
      </div>

      {editing ? (
        <div className="flex flex-col gap-2">
          <Textarea
            value={draft}
            rows={Math.min(6, Math.max(2, draft.split("\n").length + 1))}
            autoFocus
            disabled={saving}
            aria-label={t("settings.history.edit.edit")}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") setEditing(false);
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) saveEdit();
            }}
            className="w-full text-sm"
          />
          <div className="flex items-center gap-2 self-end">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setEditing(false)}
              disabled={saving}
            >
              {t("settings.history.edit.cancel")}
            </Button>
            <Button
              variant="primary"
              size="sm"
              onClick={saveEdit}
              disabled={
                saving ||
                !draft.trim() ||
                draft.trim() === entry.transcription_text
              }
            >
              {t("settings.history.edit.save")}
            </Button>
          </div>
        </div>
      ) : (
        <p
          ref={transcriptRef}
          onMouseUp={handleTranscriptMouseUp}
          className={`italic text-sm pb-2 ${
            retrying
              ? "motion-pulse"
              : hasTranscription
                ? "text-ink select-text cursor-text whitespace-pre-wrap break-words"
                : "text-ink-subtle"
          }`}
        >
          {retrying
            ? t("settings.history.transcribing")
            : hasTranscription
              ? entry.transcription_text
              : t("settings.history.transcriptionFailed")}
        </p>
      )}

      {selectedTerm && !editing && (
        <Chip
          mode="toggle"
          pressed={false}
          onToggle={addSelectionToDictionary}
          className="self-start -mt-2 mb-1 text-accent-ink"
        >
          <BookPlus className="size-4" aria-hidden="true" />
          {t("settings.history.addToDictionary", {
            word:
              selectedTerm.length > 24
                ? `${selectedTerm.slice(0, 24)}…`
                : selectedTerm,
          })}
        </Chip>
      )}

      <AudioPlayer onLoadRequest={handleLoadAudio} className="w-full" />
    </div>
  );
};
