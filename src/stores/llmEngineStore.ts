import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";
import i18n from "../i18n";
import {
  commands,
  type EngineStatus,
  type HardwareProfile,
  type LlmModelStatus,
} from "@/bindings";

interface LlmDownloadProgress {
  model_id: string;
  downloaded: number;
  total: number;
  percentage: number;
}

interface LlmEngineStore {
  status: EngineStatus | null;
  hardware: HardwareProfile | null;
  models: LlmModelStatus[];
  downloadProgress: Record<string, LlmDownloadProgress>;
  initialized: boolean;

  initialize: () => Promise<void>;
  refresh: () => Promise<void>;
  downloadModel: (id: string) => Promise<void>;
  cancelDownload: (id: string) => Promise<void>;
  deleteModel: (id: string) => Promise<void>;
  selectModel: (id: string) => Promise<void>;
  repair: () => Promise<void>;
  selftest: () => Promise<string>;
}

export const useLlmEngineStore = create<LlmEngineStore>((set, get) => ({
  status: null,
  hardware: null,
  models: [],
  downloadProgress: {},
  initialized: false,

  refresh: async () => {
    try {
      const [status, models] = await Promise.all([
        commands.getLlmEngineStatus(),
        commands.getLlmModels(),
      ]);
      set({ status, models });
    } catch (e) {
      console.error("llm engine refresh failed:", e);
    }
  },

  initialize: async () => {
    if (get().initialized) return;
    set({ initialized: true });

    try {
      const hardware = await commands.getHardwareProfile();
      set({ hardware });
    } catch (e) {
      console.error("hardware profile failed:", e);
    }
    await get().refresh();

    listen<EngineStatus>("llm-engine-status", (e) => {
      set({ status: e.payload });
    });
    listen<LlmDownloadProgress>("llm-model-download-progress", (e) => {
      set((s) => ({
        downloadProgress: {
          ...s.downloadProgress,
          [e.payload.model_id]: e.payload,
        },
      }));
    });
    const clearProgress = (modelId: string) => {
      set((s) => {
        const next = { ...s.downloadProgress };
        delete next[modelId];
        return { downloadProgress: next };
      });
      get().refresh();
    };
    listen<string>("llm-model-download-complete", (e) =>
      clearProgress(e.payload),
    );
    listen<string>("llm-model-download-cancelled", (e) =>
      clearProgress(e.payload),
    );
    listen<{ model_id: string; error: string }>(
      "llm-model-download-failed",
      (e) => {
        toast.error(
          i18n.t("settings.dictation.engine.downloadFailed", {
            error: e.payload.error,
          }),
        );
        clearProgress(e.payload.model_id);
      },
    );
    // The paste already happened with raw text, explain why, quietly.
    listen<{ reason: string }>("post-process-skipped", (e) => {
      const key =
        e.payload.reason === "engine_warming"
          ? "settings.dictation.engine.skippedWarming"
          : "settings.dictation.engine.skippedDown";
      toast.info(i18n.t(key), { duration: 3500 });
    });
  },

  downloadModel: async (id) => {
    // Optimistic flag so the picker shows a spinner immediately.
    set((s) => ({
      models: s.models.map((m) =>
        m.info.id === id ? { ...m, is_downloading: true } : m,
      ),
    }));
    try {
      const result = await commands.downloadLlmModel(id);
      if (result.status === "error") throw new Error(result.error);
    } catch (e) {
      console.error("llm model download failed:", e);
    } finally {
      get().refresh();
    }
  },

  cancelDownload: async (id) => {
    try {
      await commands.cancelLlmModelDownload(id);
    } finally {
      get().refresh();
    }
  },

  deleteModel: async (id) => {
    try {
      const result = await commands.deleteLlmModel(id);
      if (result.status === "error") throw new Error(result.error);
    } catch (e) {
      console.error("llm model delete failed:", e);
    } finally {
      get().refresh();
    }
  },

  selectModel: async (id) => {
    try {
      const result = await commands.setLlmModel(id);
      if (result.status === "error") throw new Error(result.error);
    } catch (e) {
      console.error("llm model select failed:", e);
    } finally {
      get().refresh();
    }
  },

  repair: async () => {
    await commands.restartLlmEngine();
    await get().refresh();
  },

  selftest: async () => {
    const result = await commands.llmEngineSelftest();
    if (result.status === "error") throw new Error(result.error);
    return result.data;
  },
}));
