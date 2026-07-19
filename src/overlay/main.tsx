import React from "react";
import ReactDOM from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import RecordingOverlay from "./RecordingOverlay";
import "@/i18n";
import { commands } from "@/bindings";
import { applyAppearance, bootstrapAccent } from "@/styles/applyAccent";

bootstrapAccent();

// The overlay has no settings store: pull the stored appearance once at boot
// and re-pull whenever the backend broadcasts a change (the Appearance page
// lives in the other window).
async function syncAppearance(): Promise<void> {
  try {
    const res = await commands.getAppSettings();
    if (res.status === "ok") {
      applyAppearance(
        res.data.theme_mode ?? "system",
        res.data.accent_preset ?? "sakura",
      );
    }
  } catch (e) {
    console.error("Failed to sync overlay appearance:", e);
  }
}
void syncAppearance();
void listen("appearance-changed", () => void syncAppearance());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <RecordingOverlay />
  </React.StrictMode>,
);
