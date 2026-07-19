import { listen } from "@tauri-apps/api/event";
import React, { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import "./RecordingOverlay.css";
import { commands, events } from "@/bindings";
import type {
  StreamPhase,
  StreamPhaseEvent,
  StreamTextEvent,
  StreamWorkKind,
} from "@/bindings";
import "@/i18n";

type OverlayState =
  | "recording"
  | "streaming"
  | "transcribing"
  | "processing"
  | "inserted"
  | "error";

// Number of reactive bars in the waveform (the simple, smoothed style shared by
// every overlay form). Mic levels arrive as 16 FFT buckets; we take the first N.
const WAVE_BARS = 9;

const RecordingOverlay: React.FC = () => {
  const { t } = useTranslation();
  const [isVisible, setIsVisible] = useState(false);
  const [state, setState] = useState<OverlayState>("recording");
  const [levels, setLevels] = useState<number[]>(Array(WAVE_BARS).fill(0));
  const [streamText, setStreamText] = useState<StreamTextEvent>({
    committed: "",
    tentative: "",
  });
  const [phase, setPhase] = useState<StreamPhase>("listening");
  const [workKind, setWorkKind] = useState<StreamWorkKind>("transcribing");
  const [elapsed, setElapsed] = useState(0);
  // Bumped on each new streaming session so the Live card remounts fresh (replays
  // the pop-in, and never animates in from the previous panel's open size).
  const [session, setSession] = useState(0);
  // Overlay placement (top vs bottom of the screen). The Live panel grows downward
  // from a top overlay (oldest line under the pill) and upward from a bottom one.
  const [position, setPosition] = useState<"top" | "bottom">("bottom");
  // True once live text overflows the cap. A top overlay fades its top edge only
  // while overflowing, so the resting first line stays crisp flush under the pill.
  const [overflowing, setOverflowing] = useState(false);
  // Hands-free latch: swaps the recording dot for a lock glyph, plus the
  // one-minute warning chip before the session cap stops dictation.
  const [handsFree, setHandsFree] = useState(false);
  const [capWarning, setCapWarning] = useState(false);
  // Sticky per session: once ANY live text has shown, the panel stays open
  // until hide-overlay, so the window never visibly collapses mid-speech.
  const [hasShownText, setHasShownText] = useState(false);
  // Beat 2 (vapor-condense): the committed string is append-only within a session.
  // `deltaStart` is the index where the current vapor-to-clear delta begins, so only
  // the freshly committed span re-mounts and animates (O(1) DOM churn) while the
  // text before it rests clear. `committedLenRef` holds the committed length seen
  // at the previous stream event so a new commit is detectable. Both reset per
  // session (in the show-overlay handler) and defensively on any shrink. The
  // diff lives in the event handler, never in render, so StrictMode's double
  // render cannot corrupt it.
  const [deltaStart, setDeltaStart] = useState(0);
  const committedLenRef = useRef(0);

  const smoothedLevelsRef = useRef<number[]>(Array(16).fill(0));
  // Live-text scroll-back: the text region "sticks" to the newest line while the
  // user is at the bottom; if they scroll up to read history, auto-follow pauses
  // until they scroll back down.
  const capRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);

  useEffect(() => {
    const setupEventListeners = async () => {
      const unlistenShow = await listen("show-overlay", async (event) => {
        // The Live panel flows downward from a top overlay and upward from a
        // bottom one; read the placement so the layout can flip to match.
        try {
          const settings = await commands.getAppSettings();
          if (settings.status === "ok") {
            setPosition(
              settings.data.overlay_position === "top" ? "top" : "bottom",
            );
          }
        } catch {
          // Keep the previous/default placement if settings can't be read.
        }
        const overlayState = event.payload as OverlayState;
        setState(overlayState);
        if (overlayState === "recording" || overlayState === "streaming") {
          setStreamText({ committed: "", tentative: "" });
          setHandsFree(false);
          setCapWarning(false);
          setHasShownText(false);
          // Beat 2: a fresh session restarts the ink diff.
          committedLenRef.current = 0;
          setDeltaStart(0);
        }
        if (overlayState === "streaming") {
          setPhase("listening");
          setWorkKind("transcribing");
          setElapsed(0);
          setSession((s) => s + 1); // remount the card fresh for this session
        }
        setIsVisible(true);
      });

      const unlistenHide = await listen("hide-overlay", () => {
        setIsVisible(false);
      });

      const unlistenLevel = await listen<number[]>("mic-level", (event) => {
        const newLevels = event.payload as number[];
        // Exponential smoothing across the 16 buckets, then take the first N
        // bars for the shared waveform.
        const smoothed = smoothedLevelsRef.current.map((prev, i) => {
          const target = newLevels[i] || 0;
          return prev * 0.5 + target * 0.5;
        });
        smoothedLevelsRef.current = smoothed;
        setLevels(smoothed.slice(0, WAVE_BARS));
      });

      const unlistenStream = await events.streamTextEvent.listen((event) => {
        // Beat 2: diff the append-only committed string HERE (not during render,
        // which StrictMode double-invokes) to find where the fresh delta begins.
        const { committed } = event.payload;
        const prevLen = committedLenRef.current;
        if (committed.length > prevLen) {
          // New text committed: the delta begins at the previous boundary, so it
          // condenses from vapor while everything before it stays clear.
          setDeltaStart(prevLen);
        } else if (committed.length < prevLen) {
          // Committed shrank (session reset / re-open): drop the delta.
          setDeltaStart(committed.length);
        }
        committedLenRef.current = committed.length;
        setStreamText(event.payload);
      });

      const unlistenPhase = await events.streamPhaseEvent.listen((event) => {
        const payload: StreamPhaseEvent = event.payload;
        setPhase(payload.phase);
        if (payload.kind) setWorkKind(payload.kind);
      });

      const unlistenHandsFree = await listen<boolean>(
        "hands-free-changed",
        (event) => {
          setHandsFree(event.payload);
          if (!event.payload) setCapWarning(false);
        },
      );

      const unlistenHandsFreeWarning = await listen(
        "hands-free-warning",
        () => {
          setCapWarning(true);
        },
      );

      return () => {
        unlistenShow();
        unlistenHide();
        unlistenLevel();
        unlistenStream();
        unlistenPhase();
        unlistenHandsFree();
        unlistenHandsFreeWarning();
      };
    };

    setupEventListeners();
  }, []);

  // Elapsed timer while the Live overlay is visible.
  useEffect(() => {
    if (state !== "streaming" || !isVisible) return;
    const id = setInterval(() => setElapsed((e) => e + 1), 1000);
    return () => clearInterval(id);
  }, [state, isVisible]);

  // Stick to the bottom as text streams in, but only while pinned, so a user who
  // has scrolled up to read history isn't yanked back down by the next chunk.
  useLayoutEffect(() => {
    const el = capRef.current;
    if (!el) return;
    // Fade the top edge only once text actually overflows the cap.
    setOverflowing(el.scrollHeight > el.clientHeight + 1);
    if (pinnedRef.current) el.scrollTop = el.scrollHeight;
  }, [streamText]);

  // Each fresh streaming session starts pinned to the bottom, fade cleared.
  useEffect(() => {
    pinnedRef.current = true;
    setOverflowing(false);
  }, [session]);

  // Whether the live panel currently has any text. Derived at the top level
  // (with the hooks) so the sticky-open effect below is UNCONDITIONAL: it must
  // never live inside the `state === "streaming"` branch or the hook count
  // changes when the overlay enters streaming and React tears down the tree.
  const hasText =
    streamText.committed.length > 0 || streamText.tentative.length > 0;
  useEffect(() => {
    if (hasText) setHasShownText(true);
  }, [hasText]);

  // Re-pin when the user is within ~a line of the bottom; unpin otherwise.
  const handleStreamScroll = () => {
    const el = capRef.current;
    if (!el) return;
    pinnedRef.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 16;
  };

  const fmtTime = (s: number) =>
    `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;

  // ---- Shared building blocks (one visual language for every overlay form) ----
  const waveform = (
    <div className="swave">
      {levels.map((v, i) => (
        <i
          key={i}
          style={{
            height: `${Math.max(3, Math.min(32, 3 + Math.pow(v, 0.5) * 29))}px`,
          }}
        />
      ))}
    </div>
  );

  const cancelBtn = (
    <button
      className="sx"
      aria-label={t("overlay.cancel")}
      onClick={() => commands.cancelOperation()}
    >
      <svg viewBox="0 0 16 16" aria-hidden="true">
        <path
          d="M4 4 L12 12 M12 4 L4 12"
          stroke="currentColor"
          strokeWidth="1.6"
          strokeLinecap="round"
        />
      </svg>
    </button>
  );

  // dot (left) | waveform (center) | timer + cancel (right), same structure for
  // pill & panel, so the Live morph is a pure width change.
  const listeningRow = (showTimer: boolean, showCancel: boolean) => (
    <div className="sbase">
      <div className="sbase-l">
        {handsFree ? (
          <svg
            className="slock"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2.5"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-label={t("overlay.handsFree")}
          >
            <rect x="4" y="11" width="16" height="10" rx="2" />
            <path d="M8 11V7a4 4 0 0 1 8 0v4" />
          </svg>
        ) : (
          <span className="sdot" />
        )}
      </div>
      {waveform}
      <div className="sbase-r">
        {capWarning && (
          <span className="scap-warning">{t("overlay.handsFreeEnding")}</span>
        )}
        {showTimer && <span className="stimer">{fmtTime(elapsed)}</span>}
        {showCancel && cancelBtn}
      </div>
    </div>
  );

  // spinner (left) | label (center) | cancel (right), same 3-zone grid as the
  // listening row, so the label is centered.
  const workingRow = (label: string, showCancel: boolean) => (
    <div className="sbase">
      <div className="sbase-l">
        <span className="sspinner" />
      </div>
      <span className="swork-label">{label}</span>
      <div className="sbase-r">{showCancel && cancelBtn}</div>
    </div>
  );

  // result icon (left) | label (center), the brief post-paste flash. No cancel:
  // the pipeline is already over.
  const resultRow = (label: string, kind: "ok" | "err") => (
    <div className="sbase">
      <div className="sbase-l">
        {kind === "ok" ? (
          // Beat 5 (the success beat): a sakura mist blooms behind the check, and the
          // check draws itself on (see .scheck path / draw-check in the CSS).
          <span className="sresult-icon">
            <span className="sresult-bloom" />
            <svg className="scheck" viewBox="0 0 16 16" aria-hidden="true">
              <path
                d="M3.5 8.5 L6.5 11.5 L12.5 5"
                stroke="currentColor"
                strokeWidth="1.8"
                fill="none"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            </svg>
          </span>
        ) : (
          <svg className="serr" viewBox="0 0 16 16" aria-hidden="true">
            <circle
              cx="8"
              cy="8"
              r="6.2"
              stroke="currentColor"
              strokeWidth="1.4"
              fill="none"
            />
            <path
              d="M8 5 v3.6 M8 11.2 v.2"
              stroke="currentColor"
              strokeWidth="1.6"
              strokeLinecap="round"
            />
          </svg>
        )}
      </div>
      {/* Beat 5: the success label rises in on the house entrance. */}
      <span className="swork-label motion-rise">{label}</span>
      <div className="sbase-r" />
    </div>
  );

  // ---- Live overlay: a pill that sculpts open into a panel ----
  if (state === "streaming") {
    const working = phase === "working";
    // Keep the panel open whenever there's text, even while finalizing, so the
    // transcript stays put under a working spinner instead of collapsing and
    // squishing the text mid-stream. Only fall back to the small working pill
    // when there was no text to preserve.
    const open = hasText || hasShownText;
    const collapsed = working && !hasText && !hasShownText;

    return (
      <div className={`ov-stage ${position}`}>
        <div
          key={session}
          className={`scard ${open ? "open" : ""} ${collapsed ? "working" : ""} ${
            isVisible ? "" : "leaving"
          }`}
        >
          <div className="stext">
            <div className="stext-clip">
              <div
                className={`stext-cap ${overflowing ? "overflowing" : ""}`}
                ref={capRef}
                onScroll={handleStreamScroll}
              >
                <p>
                  {/* Beat 2: the already-clear text rests; only the freshly
                      committed delta re-mounts (keyed by committed length) so it
                      condenses from vapor once and then rests. The trailing space
                      rides on the delta span so words never run together. */}
                  <span className="committed">
                    {streamText.committed.slice(0, deltaStart)}
                  </span>
                  {streamText.committed.length > deltaStart && (
                    <span
                      className="committed condensing"
                      key={streamText.committed.length}
                    >
                      {streamText.committed.slice(deltaStart) + " "}
                    </span>
                  )}
                  <span className="tentative">{streamText.tentative}</span>
                  {/* Beat 4: the caret catches in from the waveform edge the
                      first time text shows this session, then breathes. Dropped
                      once finalizing, a static spinner conveys the work. */}
                  {!working && (
                    <span
                      className={hasShownText ? "scaret catch" : "scaret"}
                    />
                  )}
                </p>
              </div>
            </div>
          </div>
          {working
            ? workingRow(
                workKind === "polishing"
                  ? t("overlay.processing")
                  : t("overlay.transcribing"),
                true,
              )
            : listeningRow(open, true)}
        </div>
      </div>
    );
  }

  // ---- Minimal overlay: exactly one row at a time, waveform (recording), a
  // spinner + label (transcribing / processing), or a brief result flash
  // (inserted / error). The pill animates its width between them.
  const working = state === "transcribing" || state === "processing";
  const result = state === "inserted" || state === "error";
  const workLabel =
    state === "processing"
      ? t("overlay.processing")
      : t("overlay.transcribing");

  return (
    <div className={`ov-stage ${position} ov-fade ${isVisible ? "show" : ""}`}>
      <div
        className={`scard compact ${(working || result) && isVisible ? "cworking" : ""} ${
          state === "inserted" ? "inserted" : ""
        }`}
      >
        {result
          ? resultRow(
              state === "inserted"
                ? t("overlay.inserted")
                : t("overlay.insertFailed"),
              state === "inserted" ? "ok" : "err",
            )
          : working
            ? workingRow(workLabel, true)
            : listeningRow(false, true)}
      </div>
    </div>
  );
};

export default RecordingOverlay;
