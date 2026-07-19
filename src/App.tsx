import {
  lazy,
  Suspense,
  useEffect,
  useState,
  useRef,
  type CSSProperties,
} from "react";
import { toast, Toaster } from "sonner";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { platform } from "@tauri-apps/plugin-os";
import {
  checkAccessibilityPermission,
  checkMicrophonePermission,
} from "tauri-plugin-macos-permissions-api";
import { ModelStateEvent, RecordingErrorEvent } from "./lib/types/events";
import { applyAppearance } from "./styles/applyAccent";
import "./App.css";
import AccessibilityPermissions from "./components/AccessibilityPermissions";
import {
  AccessibilityOnboarding,
  CleanupDownloadStep,
  IntroReveal,
  SttDownloadStep,
  TryItStep,
} from "./components/onboarding";
import { Sidebar, SidebarSection, SECTIONS_CONFIG } from "./components/Sidebar";
import { useSettings } from "./hooks/useSettings";
import { useLabGate } from "./hooks/useLabGate";
import { useSettingsStore } from "./stores/settingsStore";
import { commands } from "@/bindings";

// The dev-only Lab (a hidden component + motion gallery). The import lives in a
// dead `import.meta.env.DEV ? ... : null` branch, so a production build sees
// `false ? lazy(import(...)) : null` and Rollup drops the module (and its CSS)
// entirely. Never reachable, never bundled, in shipped builds.
const Lab = import.meta.env.DEV
  ? lazy(() => import("./components/dev/Lab"))
  : null;

type OnboardingStep =
  "accessibility" | "stt_download" | "cleanup_download" | "tryit" | "done";

const renderSettingsContent = (section: SidebarSection) => {
  const ActiveComponent =
    SECTIONS_CONFIG[section]?.component || SECTIONS_CONFIG.dictation.component;
  return <ActiveComponent />;
};

function App() {
  const { t } = useTranslation();
  // Dev-only: summon the Lab (g then l, or #lab). No-op / false in production.
  const labOpen = useLabGate();
  const [onboardingStep, setOnboardingStep] = useState<OnboardingStep | null>(
    null,
  );
  // Track if this is a returning user who just needs to grant permissions
  // (vs a new user who needs full onboarding including the model download)
  const [isReturningUser, setIsReturningUser] = useState(false);
  // First-run reveal plays once per session on the fresh-onboarding path only.
  // Seed from sessionStorage so it never replays across re-renders or section
  // navigation within a session.
  const [introPlayed, setIntroPlayed] = useState(
    () =>
      typeof sessionStorage !== "undefined" &&
      sessionStorage.getItem("vaporly.introShown") === "1",
  );
  const [currentSection, setCurrentSection] =
    useState<SidebarSection>("dictation");
  // Keep each visited section mounted so switching is an instant show/hide,
  // not an unmount+remount (which re-ran History's SQL query, rebuilt its rows,
  // and replayed an entrance animation over the mounting subtree = the jank).
  const [visitedSections, setVisitedSections] = useState<Set<SidebarSection>>(
    () => new Set<SidebarSection>(["dictation"]),
  );
  // Pre-mount every non-default section off the critical path so the FIRST
  // visit to each is an instant visibility toggle, not a synchronous build.
  // Otherwise custom and general mount inside the click handler (the jitter),
  // and history builds ~12 rows each with an audio element. Stagger them so no
  // single frame is heavy; general goes last because its UpdateChecker fires a
  // network check on mount. setTimeout, since WKWebView has no reliable
  // requestIdleCallback.
  useEffect(() => {
    const order: SidebarSection[] = [
      "custom",
      "history",
      "appearance",
      "general",
    ];
    const timers = order.map((section, i) =>
      setTimeout(
        () =>
          setVisitedSections((prev) =>
            prev.has(section) ? prev : new Set(prev).add(section),
          ),
        400 * (i + 1),
      ),
    );
    return () => timers.forEach(clearTimeout);
  }, []);
  const { updateSetting } = useSettings();
  // Apply the stored appearance (theme mode + accent preset) whenever it
  // loads or changes; the overlay window syncs via the appearance-changed
  // event instead (it has no settings store).
  const themeMode = useSettingsStore((state) => state.settings?.theme_mode);
  const accentPreset = useSettingsStore(
    (state) => state.settings?.accent_preset,
  );
  useEffect(() => {
    applyAppearance(themeMode ?? "system", accentPreset ?? "sakura");
  }, [themeMode, accentPreset]);
  const refreshAudioDevices = useSettingsStore(
    (state) => state.refreshAudioDevices,
  );
  const refreshOutputDevices = useSettingsStore(
    (state) => state.refreshOutputDevices,
  );
  const hasCompletedPostOnboardingInit = useRef(false);

  useEffect(() => {
    checkOnboardingStatus();
  }, []);

  // Initialize Enigo, shortcuts, and refresh audio devices when main app loads.
  // The try-it step needs live shortcuts + paste too, so it counts as loaded.
  useEffect(() => {
    if (
      (onboardingStep === "done" || onboardingStep === "tryit") &&
      !hasCompletedPostOnboardingInit.current
    ) {
      hasCompletedPostOnboardingInit.current = true;
      Promise.all([
        commands.initializeEnigo(),
        commands.initializeShortcuts(),
      ]).catch((e) => {
        console.warn("Failed to initialize:", e);
      });
      refreshAudioDevices();
      refreshOutputDevices();
    }
  }, [onboardingStep, refreshAudioDevices, refreshOutputDevices]);

  // Listen for recording errors from the backend and show a toast
  useEffect(() => {
    const unlisten = listen<RecordingErrorEvent>("recording-error", (event) => {
      const { error_type, detail } = event.payload;

      if (error_type === "microphone_permission_denied") {
        const currentPlatform = platform();
        const platformKey = `errors.micPermissionDenied.${currentPlatform}`;
        const description = t(platformKey, {
          defaultValue: t("errors.micPermissionDenied.generic"),
        });
        toast.error(t("errors.micPermissionDeniedTitle"), { description });
      } else if (error_type === "no_input_device") {
        toast.error(t("errors.noInputDeviceTitle"), {
          description: t("errors.noInputDevice"),
        });
      } else {
        toast.error(
          t("errors.recordingFailed", { error: detail ?? "Unknown error" }),
        );
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [t]);

  // Listen for paste failures and show a toast.
  // The technical error detail is logged to vaporly.log on the Rust side
  // (see actions.rs `error!("Failed to paste transcription: ...")`),
  // so we show a localized, user-friendly message here instead of the raw error.
  useEffect(() => {
    const unlisten = listen("paste-error", () => {
      toast.error(t("errors.pasteFailedTitle"), {
        description: t("errors.pasteFailed"),
      });
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [t]);

  // Listen for transcription failures and show a toast.
  // The payload is the backend error message (also logged to vaporly.log).
  useEffect(() => {
    const unlisten = listen<string>("transcription-error", (event) => {
      toast.error(t("errors.transcriptionFailedTitle"), {
        description: event.payload,
      });
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [t]);

  // One-minute warning before a hands-free session hits the 20 minute cap.
  useEffect(() => {
    const unlisten = listen("hands-free-warning", () => {
      toast.warning(t("toasts.handsFreeWarning"));
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [t]);

  // F4 auto-learn: announce words the backend just added to Custom Words.
  useEffect(() => {
    const unlisten = listen<{ words: string[]; source: string }>(
      "custom-words-learned",
      (event) => {
        toast.success(
          t("toasts.wordsLearned", {
            words: event.payload.words.join(", "),
          }),
        );
      },
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [t]);

  // Tray "Check for Updates" shows the window and emits this event; jump to
  // the General section so the UpdateChecker (which checks on mount and on
  // this same event) is actually on screen.
  useEffect(() => {
    const unlisten = listen("check-for-updates", () => {
      setCurrentSection("general");
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Listen for model loading failures and show a toast
  useEffect(() => {
    const unlisten = listen<ModelStateEvent>("model-state-changed", (event) => {
      if (event.payload.event_type === "loading_failed") {
        toast.error(
          t("errors.modelLoadFailed", {
            model:
              event.payload.model_name || t("errors.modelLoadFailedUnknown"),
          }),
          {
            description: event.payload.error,
          },
        );
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [t]);

  const revealMainWindowForPermissions = async () => {
    try {
      await commands.showMainWindowCommand();
    } catch (e) {
      console.warn("Failed to show main window for permission onboarding:", e);
    }
  };

  const checkOnboardingStatus = async () => {
    try {
      const settingsResult = await commands.getAppSettings();
      const hasCompletedOnboarding =
        settingsResult.status === "ok" &&
        settingsResult.data.onboarding_completed === true;
      const currentPlatform = platform();

      if (hasCompletedOnboarding) {
        // Returning user - check if they need to grant permissions first
        setIsReturningUser(true);

        if (currentPlatform === "macos") {
          try {
            const [hasAccessibility, hasMicrophone] = await Promise.all([
              checkAccessibilityPermission(),
              checkMicrophonePermission(),
            ]);
            if (!hasAccessibility || !hasMicrophone) {
              await revealMainWindowForPermissions();
              setOnboardingStep("accessibility");
              return;
            }
          } catch (e) {
            console.warn("Failed to check macOS permissions:", e);
            // If we can't check, proceed to main app and let them fix it there
          }
        }

        if (currentPlatform === "windows") {
          try {
            const microphoneStatus =
              await commands.getWindowsMicrophonePermissionStatus();
            if (
              microphoneStatus.supported &&
              microphoneStatus.overall_access === "denied"
            ) {
              await revealMainWindowForPermissions();
              setOnboardingStep("accessibility");
              return;
            }
          } catch (e) {
            console.warn("Failed to check Windows microphone permissions:", e);
            // If we can't check, proceed to main app and let them fix it there
          }
        }

        setOnboardingStep("done");
      } else {
        // New user - start full onboarding
        setIsReturningUser(false);
        setOnboardingStep("accessibility");
      }
    } catch (error) {
      console.error("Failed to check onboarding status:", error);
      setOnboardingStep("accessibility");
    }
  };

  const handleAccessibilityComplete = () => {
    // Returning users already have the model, skip to main app.
    // New users download the fixed speech model next.
    setOnboardingStep(isReturningUser ? "done" : "stt_download");
  };

  // D1: legacy default STT model auto-upgraded in the background, tell the user.
  useEffect(() => {
    const un = listen<string>("stt-model-upgraded", (e) => {
      toast.success(t("toasts.sttUpgraded", { model: e.payload }));
    });
    return () => {
      un.then((f) => f());
    };
  }, [t]);

  const handleSttDownloadComplete = () => {
    // Round-2 defaults run mind-change on the Model engine, so a fresh
    // install also fetches the cleanup model (the step auto-advances when
    // it is already present, and skips itself on the Raw hardware tier).
    setOnboardingStep("cleanup_download");
  };

  const handleCleanupDownloadComplete = () => {
    // First success moment before the main app: try one dictation
    setOnboardingStep("tryit");
  };

  const handleTryItComplete = () => {
    // Persist completion so the next launch skips onboarding.
    updateSetting("onboarding_completed", true);
    setOnboardingStep("done");
  };

  // Dev-only: the Lab fully replaces the app when summoned. Guarded by
  // import.meta.env.DEV so this branch (and the Lab import) is dead code in
  // production. Placed after every hook so hook order is stable.
  if (import.meta.env.DEV && labOpen && Lab) {
    return (
      <Suspense fallback={null}>
        <Lab />
      </Suspense>
    );
  }

  // Still checking onboarding status
  if (onboardingStep === null) {
    return null;
  }

  // First-run reveal, before the step machine. Only on the fresh-onboarding
  // path (never returning users, never settings re-renders / section nav), and
  // only once per session. It hands off to the first step below on completion.
  if (!introPlayed && !isReturningUser && onboardingStep !== "done") {
    return (
      <IntroReveal
        onDone={() => {
          try {
            sessionStorage.setItem("vaporly.introShown", "1");
          } catch {
            // sessionStorage may be unavailable; the in-memory flag still gates.
          }
          setIntroPlayed(true);
        }}
      />
    );
  }

  if (onboardingStep === "accessibility") {
    return <AccessibilityOnboarding onComplete={handleAccessibilityComplete} />;
  }

  if (onboardingStep === "stt_download") {
    return <SttDownloadStep onComplete={handleSttDownloadComplete} />;
  }

  if (onboardingStep === "cleanup_download") {
    return <CleanupDownloadStep onComplete={handleCleanupDownloadComplete} />;
  }

  if (onboardingStep === "tryit") {
    return <TryItStep onComplete={handleTryItComplete} />;
  }

  return (
    <div className="h-dvh flex flex-col select-none cursor-default">
      <Toaster
        theme="system"
        position="top-right"
        style={{ zIndex: "var(--z-toast)" } as CSSProperties}
        toastOptions={{
          unstyled: true,
          classNames: {
            toast:
              "bg-surface-raised rounded-card border border-hairline px-4 py-3 flex items-center gap-3 text-sm",
            title: "text-ink font-medium",
            description: "text-ink-muted",
          },
        }}
      />
      {/* Main content area that takes remaining space */}
      <div className="flex-1 flex overflow-hidden">
        <Sidebar
          activeSection={currentSection}
          onSectionChange={(section) => {
            setVisitedSections((prev) =>
              prev.has(section) ? prev : new Set(prev).add(section),
            );
            setCurrentSection(section);
          }}
        />
        {/* Scrollable content area */}
        <div className="flex-1 flex flex-col overflow-hidden">
          <div className="flex-1 overflow-y-auto overscroll-y-contain">
            <div className="flex flex-col items-center p-6 gap-6">
              <AccessibilityPermissions />
              {/* Each visited section stays mounted and toggles visibility, so
                  switching is an instant show/hide: no unmount+remount, no SQL
                  re-query, no replayed entrance animation over a mounting tree. */}
              {(Object.keys(SECTIONS_CONFIG) as SidebarSection[])
                .filter((section) => visitedSections.has(section))
                .map((section) => (
                  <div
                    key={section}
                    className={
                      section === currentSection
                        ? "w-full flex flex-col items-center"
                        : "hidden"
                    }
                  >
                    {renderSettingsContent(section)}
                  </div>
                ))}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

export default App;
