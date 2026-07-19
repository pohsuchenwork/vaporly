use crate::actions::ACTION_MAP;
use crate::managers::audio::AudioRecordingManager;
use log::{debug, error, warn};
use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// Same-binding press debounce (key repeat). Per binding, so a fast fn then
/// fn+space chord is never swallowed by the fn press a few ms earlier.
const DEBOUNCE: Duration = Duration::from_millis(30);
/// A press held shorter than this is a "tap": tap 1 of a double-tap or an
/// accidental graze, never a deliberate dictation.
const TAP_MAX: Duration = Duration::from_millis(300);
/// After a tap release, how long a second press may take to latch hands-free.
const TAP_GAP: Duration = Duration::from_millis(350);
/// Dequeue-latency slack on the TAP_GAP deadline.
const TAP_GRACE: Duration = Duration::from_millis(50);
/// Hands-free sessions warn at 19 minutes and stop at 20 (a 20 minute cap).
const HANDS_FREE_WARN: Duration = Duration::from_secs(19 * 60);
const HANDS_FREE_MAX: Duration = Duration::from_secs(20 * 60);

/// Commands processed sequentially by the coordinator thread.
enum Command {
    Input {
        binding_id: String,
        hotkey_string: String,
        is_pressed: bool,
        push_to_talk: bool,
    },
    Cancel {
        recording_was_active: bool,
    },
    ProcessingFinished,
}

/// How the active recording is being driven.
#[derive(Clone, Debug, PartialEq)]
enum RecMode {
    /// Push-to-talk key currently held down.
    Hold { pressed_at: Instant },
    /// A sub-TAP_MAX press was released; recording continues while we wait
    /// for the second tap of a double-tap. No audio is lost either way.
    TapPending { deadline: Instant },
    /// Hands-free: press the key again to stop. Capped at HANDS_FREE_MAX.
    Latched { warned: bool },
}

/// Pipeline lifecycle, owned exclusively by the coordinator thread.
#[derive(Clone, Debug, PartialEq)]
enum Stage {
    Idle,
    Recording {
        binding_id: String,
        hotkey: String,
        mode: RecMode,
        started_at: Instant,
    },
    Processing,
}

/// What the machine wants done; applied by the thread that owns the app
/// handle so the state logic stays pure and unit-testable.
#[derive(Clone, Debug, PartialEq)]
enum Effect {
    Start {
        binding_id: String,
        hotkey: String,
    },
    Stop {
        binding_id: String,
        hotkey: String,
    },
    /// Abandon the current recording without transcribing (chord intercept).
    CancelSession,
    HandsFreeChanged(bool),
    HandsFreeWarning,
}

/// Pure state machine. All `Instant`s are injected, so the transition table
/// is testable without threads or timers.
struct Machine {
    stage: Stage,
    last_press: HashMap<String, Instant>,
}

impl Machine {
    fn new() -> Self {
        Self {
            stage: Stage::Idle,
            last_press: HashMap::new(),
        }
    }

    /// Deadline the loop must wake at even if no input arrives.
    fn next_deadline(&self) -> Option<Instant> {
        match &self.stage {
            Stage::Recording {
                mode, started_at, ..
            } => match mode {
                RecMode::Hold { .. } => None,
                RecMode::TapPending { deadline } => Some(*deadline),
                RecMode::Latched { warned } => {
                    if *warned {
                        Some(*started_at + HANDS_FREE_MAX)
                    } else {
                        Some(*started_at + HANDS_FREE_WARN)
                    }
                }
            },
            _ => None,
        }
    }

    fn debounced(&mut self, now: Instant, binding_id: &str) -> bool {
        if let Some(prev) = self.last_press.get(binding_id) {
            if now.duration_since(*prev) < DEBOUNCE {
                return true;
            }
        }
        self.last_press.insert(binding_id.to_string(), now);
        false
    }

    fn on_input(
        &mut self,
        now: Instant,
        binding_id: &str,
        hotkey: &str,
        is_pressed: bool,
        toggle: bool,
    ) -> Vec<Effect> {
        if is_pressed && self.debounced(now, binding_id) {
            debug!("Debounced press for '{binding_id}'");
            return vec![];
        }

        match self.stage.clone() {
            Stage::Idle => {
                if !is_pressed {
                    return vec![];
                }
                let mode = if toggle {
                    RecMode::Latched { warned: false }
                } else {
                    RecMode::Hold { pressed_at: now }
                };
                let mut fx = vec![Effect::Start {
                    binding_id: binding_id.to_string(),
                    hotkey: hotkey.to_string(),
                }];
                if toggle {
                    fx.push(Effect::HandsFreeChanged(true));
                }
                // Stage is set on confirm_started (Start may fail).
                self.stage = Stage::Recording {
                    binding_id: binding_id.to_string(),
                    hotkey: hotkey.to_string(),
                    mode,
                    started_at: now,
                };
                fx
            }
            Stage::Recording {
                binding_id: rec_id,
                hotkey: rec_hotkey,
                mode,
                started_at: _,
            } => {
                if is_pressed {
                    if binding_id == rec_id {
                        return match mode {
                            RecMode::Hold { .. } => {
                                debug!("Ignoring repeat press for '{binding_id}' while held");
                                vec![]
                            }
                            RecMode::TapPending { deadline } => {
                                if now <= deadline + TAP_GRACE {
                                    // Double-tap: latch hands-free.
                                    self.set_mode(RecMode::Latched { warned: false });
                                    vec![Effect::HandsFreeChanged(true)]
                                } else {
                                    // Late press: consumed as the stop.
                                    self.stage = Stage::Processing;
                                    vec![Effect::Stop {
                                        binding_id: rec_id,
                                        hotkey: rec_hotkey,
                                    }]
                                }
                            }
                            RecMode::Latched { .. } => {
                                self.stage = Stage::Processing;
                                vec![
                                    Effect::Stop {
                                        binding_id: rec_id,
                                        hotkey: rec_hotkey,
                                    },
                                    Effect::HandsFreeChanged(false),
                                ]
                            }
                        };
                    }
                    debug!("Ignoring press for '{binding_id}': pipeline busy");
                    vec![]
                } else {
                    // Release.
                    if binding_id != rec_id {
                        return vec![];
                    }
                    match mode {
                        RecMode::Hold { pressed_at } => {
                            if now.duration_since(pressed_at) >= TAP_MAX {
                                self.stage = Stage::Processing;
                                vec![Effect::Stop {
                                    binding_id: rec_id,
                                    hotkey: rec_hotkey,
                                }]
                            } else {
                                // A tap: keep recording, await the second tap.
                                self.set_mode(RecMode::TapPending {
                                    deadline: now + TAP_GAP,
                                });
                                vec![]
                            }
                        }
                        // TapPending absorbs the tap's own release; Latched
                        // absorbs both the latching tap's release and the
                        // stop-press release (which lands in Processing).
                        RecMode::TapPending { .. } | RecMode::Latched { .. } => vec![],
                    }
                }
            }
            Stage::Processing => {
                debug!("Ignoring input for '{binding_id}': pipeline busy");
                vec![]
            }
        }
    }

    fn on_timeout(&mut self, now: Instant) -> Vec<Effect> {
        let Stage::Recording {
            binding_id,
            hotkey,
            mode,
            started_at,
        } = self.stage.clone()
        else {
            return vec![];
        };
        match mode {
            RecMode::TapPending { deadline } if now >= deadline => {
                // No second tap. By construction the session is at most
                // tap(300ms) + gap(350ms) old, i.e. an accidental graze, not
                // a dictation: cancel quietly instead of running the full
                // stop/process/paste cycle (which read as the overlay
                // flickering open and closed).
                let _ = (binding_id, hotkey);
                self.stage = Stage::Idle;
                vec![Effect::CancelSession]
            }
            RecMode::Latched { warned } => {
                if now >= started_at + HANDS_FREE_MAX {
                    self.stage = Stage::Processing;
                    vec![
                        Effect::Stop { binding_id, hotkey },
                        Effect::HandsFreeChanged(false),
                    ]
                } else if !warned && now >= started_at + HANDS_FREE_WARN {
                    self.set_mode(RecMode::Latched { warned: true });
                    vec![Effect::HandsFreeWarning]
                } else {
                    vec![]
                }
            }
            _ => vec![],
        }
    }

    fn on_cancel(&mut self, recording_was_active: bool) -> Vec<Effect> {
        if !matches!(self.stage, Stage::Processing)
            && (recording_was_active || matches!(self.stage, Stage::Recording { .. }))
        {
            let was_latched = matches!(
                &self.stage,
                Stage::Recording {
                    mode: RecMode::Latched { .. },
                    ..
                }
            );
            self.stage = Stage::Idle;
            if was_latched {
                return vec![Effect::HandsFreeChanged(false)];
            }
        }
        vec![]
    }

    fn on_processing_finished(&mut self) {
        self.stage = Stage::Idle;
    }

    /// Result of applying a Start effect: recording actually began?
    fn confirm_started(&mut self, ok: bool) {
        if !ok && matches!(self.stage, Stage::Recording { .. }) {
            debug!("Start did not begin recording; staying idle");
            self.stage = Stage::Idle;
        }
    }

    fn set_mode(&mut self, new_mode: RecMode) {
        if let Stage::Recording { mode, .. } = &mut self.stage {
            *mode = new_mode;
        }
    }
}

/// Serialises all transcription lifecycle events through a single thread
/// to eliminate race conditions between keyboard shortcuts, signals, timers,
/// and the async transcribe-paste pipeline.
pub struct TranscriptionCoordinator {
    tx: Sender<Command>,
}

pub fn is_transcribe_binding(id: &str) -> bool {
    id == "transcribe"
}

impl TranscriptionCoordinator {
    pub fn new(app: AppHandle) -> Self {
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut machine = Machine::new();

                loop {
                    let cmd = match machine.next_deadline() {
                        Some(deadline) => {
                            let timeout = deadline.saturating_duration_since(Instant::now());
                            match rx.recv_timeout(timeout) {
                                Ok(cmd) => Some(cmd),
                                Err(RecvTimeoutError::Timeout) => None,
                                Err(RecvTimeoutError::Disconnected) => break,
                            }
                        }
                        None => match rx.recv() {
                            Ok(cmd) => Some(cmd),
                            Err(_) => break,
                        },
                    };

                    let now = Instant::now();
                    let effects = match cmd {
                        Some(Command::Input {
                            binding_id,
                            hotkey_string,
                            is_pressed,
                            push_to_talk,
                        }) => {
                            let toggle = !push_to_talk;
                            machine.on_input(now, &binding_id, &hotkey_string, is_pressed, toggle)
                        }
                        Some(Command::Cancel {
                            recording_was_active,
                        }) => machine.on_cancel(recording_was_active),
                        Some(Command::ProcessingFinished) => {
                            machine.on_processing_finished();
                            vec![]
                        }
                        None => machine.on_timeout(now),
                    };

                    apply_effects(&app, &mut machine, effects);
                }
                debug!("Transcription coordinator exited");
            }));
            if let Err(e) = result {
                error!("Transcription coordinator panicked: {e:?}");
            }
        });

        Self { tx }
    }

    /// Send a keyboard/signal input event for a transcribe binding.
    /// For signal-based toggles, use `is_pressed: true` and `push_to_talk: false`.
    pub fn send_input(
        &self,
        binding_id: &str,
        hotkey_string: &str,
        is_pressed: bool,
        push_to_talk: bool,
    ) {
        if self
            .tx
            .send(Command::Input {
                binding_id: binding_id.to_string(),
                hotkey_string: hotkey_string.to_string(),
                is_pressed,
                push_to_talk,
            })
            .is_err()
        {
            warn!("Transcription coordinator channel closed");
        }
    }

    pub fn notify_cancel(&self, recording_was_active: bool) {
        if self
            .tx
            .send(Command::Cancel {
                recording_was_active,
            })
            .is_err()
        {
            warn!("Transcription coordinator channel closed");
        }
    }

    pub fn notify_processing_finished(&self) {
        if self.tx.send(Command::ProcessingFinished).is_err() {
            warn!("Transcription coordinator channel closed");
        }
    }
}

fn apply_effects(app: &AppHandle, machine: &mut Machine, effects: Vec<Effect>) {
    for effect in effects {
        match effect {
            Effect::Start { binding_id, hotkey } => {
                let Some(action) = ACTION_MAP.get(binding_id.as_str()) else {
                    warn!("No action in ACTION_MAP for '{binding_id}'");
                    machine.confirm_started(false);
                    continue;
                };
                action.start(app, &binding_id, &hotkey);
                let ok = app
                    .try_state::<Arc<AudioRecordingManager>>()
                    .is_some_and(|a| a.is_recording());
                machine.confirm_started(ok);
            }
            Effect::Stop { binding_id, hotkey } => {
                let Some(action) = ACTION_MAP.get(binding_id.as_str()) else {
                    warn!("No action in ACTION_MAP for '{binding_id}'");
                    continue;
                };
                action.stop(app, &binding_id, &hotkey);
            }
            Effect::CancelSession => {
                crate::utils::cancel_current_operation(app);
            }
            Effect::HandsFreeChanged(latched) => {
                crate::overlay::set_overlay_hands_free(app, latched);
            }
            Effect::HandsFreeWarning => {
                crate::overlay::emit_hands_free_warning(app);
                let _ = app.emit("hands-free-warning", 60u64);
            }
        }
    }
}

#[cfg(test)]
mod machine_tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    fn press(m: &mut Machine, t: Instant, id: &str, key: &str, toggle: bool) -> Vec<Effect> {
        m.on_input(t, id, key, true, toggle)
    }
    fn release(m: &mut Machine, t: Instant, id: &str, key: &str, toggle: bool) -> Vec<Effect> {
        m.on_input(t, id, key, false, toggle)
    }
    fn starts(fx: &[Effect]) -> usize {
        fx.iter()
            .filter(|e| matches!(e, Effect::Start { .. }))
            .count()
    }
    fn stops(fx: &[Effect]) -> usize {
        fx.iter()
            .filter(|e| matches!(e, Effect::Stop { .. }))
            .count()
    }

    #[test]
    fn hold_release_stops_immediately() {
        let mut m = Machine::new();
        let t0 = Instant::now();
        let fx = press(&mut m, t0, "transcribe", "fn", false);
        assert_eq!(starts(&fx), 1);
        let fx = release(&mut m, t0 + ms(800), "transcribe", "fn", false);
        assert_eq!(stops(&fx), 1, "long hold stops on release, zero latency");
        assert_eq!(m.stage, Stage::Processing);
    }

    #[test]
    fn short_tap_waits_then_stops_at_deadline() {
        let mut m = Machine::new();
        let t0 = Instant::now();
        press(&mut m, t0, "transcribe", "fn", false);
        let fx = release(&mut m, t0 + ms(120), "transcribe", "fn", false);
        assert!(fx.is_empty(), "tap release keeps recording");
        assert!(m.next_deadline().is_some());
        let fx = m.on_timeout(t0 + ms(120) + TAP_GAP);
        assert_eq!(
            fx,
            vec![Effect::CancelSession],
            "no second tap: a graze cancels quietly (no processing flicker)"
        );
        assert_eq!(m.stage, Stage::Idle);
    }

    #[test]
    fn double_tap_latches_hands_free() {
        let mut m = Machine::new();
        let t0 = Instant::now();
        press(&mut m, t0, "transcribe", "fn", false);
        release(&mut m, t0 + ms(120), "transcribe", "fn", false);
        let fx = press(&mut m, t0 + ms(250), "transcribe", "fn", false);
        assert_eq!(fx, vec![Effect::HandsFreeChanged(true)]);
        // Its release is absorbed; a later press stops.
        assert!(release(&mut m, t0 + ms(350), "transcribe", "fn", false).is_empty());
        let fx = press(&mut m, t0 + ms(5000), "transcribe", "fn", false);
        assert_eq!(stops(&fx), 1);
        assert!(fx.contains(&Effect::HandsFreeChanged(false)));
    }

    #[test]
    fn late_second_press_stops_instead_of_latching() {
        let mut m = Machine::new();
        let t0 = Instant::now();
        press(&mut m, t0, "transcribe", "fn", false);
        release(&mut m, t0 + ms(100), "transcribe", "fn", false);
        // Past deadline+grace: consumed as the stop.
        let fx = press(
            &mut m,
            t0 + ms(100) + TAP_GAP + TAP_GRACE + ms(10),
            "transcribe",
            "fn",
            false,
        );
        assert_eq!(stops(&fx), 1);
    }

    #[test]
    fn toggle_press_latches_from_idle() {
        let mut m = Machine::new();
        let t0 = Instant::now();
        let fx = press(&mut m, t0, "transcribe", "fn", true);
        assert_eq!(starts(&fx), 1);
        assert!(fx.contains(&Effect::HandsFreeChanged(true)));
        assert!(m.next_deadline().is_some(), "latched sessions are capped");
        let fx = press(&mut m, t0 + ms(2000), "transcribe", "fn", true);
        assert_eq!(stops(&fx), 1);
    }

    #[test]
    fn warn_fires_once_then_cap_stops() {
        let mut m = Machine::new();
        let t0 = Instant::now();
        press(&mut m, t0, "transcribe", "fn", true);
        let fx = m.on_timeout(t0 + HANDS_FREE_WARN);
        assert_eq!(fx, vec![Effect::HandsFreeWarning]);
        assert!(m.on_timeout(t0 + HANDS_FREE_WARN + ms(10)).is_empty());
        let fx = m.on_timeout(t0 + HANDS_FREE_MAX);
        assert_eq!(stops(&fx), 1);
        assert_eq!(m.stage, Stage::Processing);
        assert!(m.next_deadline().is_none(), "no deadline after Processing");
    }

    #[test]
    fn debounce_swallows_bounce_presses() {
        let mut m = Machine::new();
        let t0 = Instant::now();
        // Tap, then a switch-bounce press within DEBOUNCE of the first press:
        // swallowed, so it neither latches nor stops.
        press(&mut m, t0, "transcribe", "fn", false);
        release(&mut m, t0 + ms(10), "transcribe", "fn", false);
        let fx = press(&mut m, t0 + ms(20), "transcribe", "fn", false);
        assert!(fx.is_empty(), "bounce press is debounced");
        // A deliberate second tap after the debounce window latches.
        let fx = press(&mut m, t0 + ms(120), "transcribe", "fn", false);
        assert_eq!(fx, vec![Effect::HandsFreeChanged(true)]);
    }

    #[test]
    fn other_binding_press_while_recording_is_busy() {
        let mut m = Machine::new();
        let t0 = Instant::now();
        press(&mut m, t0, "transcribe", "fn", false);
        let fx = press(&mut m, t0 + ms(800), "some_other_binding", "fn+d", false);
        assert!(fx.is_empty(), "recording sessions are not hijacked");
    }

    #[test]
    fn latched_ignores_releases_and_other_presses() {
        let mut m = Machine::new();
        let t0 = Instant::now();
        press(&mut m, t0, "transcribe", "fn", true);
        assert!(release(&mut m, t0 + ms(100), "transcribe", "fn", true).is_empty());
        let fx = press(&mut m, t0 + ms(1000), "some_other_binding", "fn+d", false);
        assert!(fx.is_empty(), "latched sessions ignore other bindings");
    }

    #[test]
    fn cancel_during_latch_returns_idle_and_clears_deadline() {
        let mut m = Machine::new();
        let t0 = Instant::now();
        press(&mut m, t0, "transcribe", "fn", true);
        let fx = m.on_cancel(true);
        assert_eq!(fx, vec![Effect::HandsFreeChanged(false)]);
        assert_eq!(m.stage, Stage::Idle);
        assert!(m.next_deadline().is_none());
    }

    #[test]
    fn processing_ignores_everything_until_finished() {
        let mut m = Machine::new();
        let t0 = Instant::now();
        press(&mut m, t0, "transcribe", "fn", false);
        release(&mut m, t0 + ms(500), "transcribe", "fn", false);
        assert_eq!(m.stage, Stage::Processing);
        assert!(press(&mut m, t0 + ms(600), "transcribe", "fn", false).is_empty());
        assert!(m.on_timeout(t0 + ms(700)).is_empty());
        m.on_processing_finished();
        assert_eq!(m.stage, Stage::Idle);
    }

    #[test]
    fn failed_start_returns_to_idle() {
        let mut m = Machine::new();
        let t0 = Instant::now();
        press(&mut m, t0, "transcribe", "fn", false);
        m.confirm_started(false);
        assert_eq!(m.stage, Stage::Idle);
        assert!(release(&mut m, t0 + ms(500), "transcribe", "fn", false).is_empty());
    }

    #[test]
    fn plain_toggle_sessions_get_the_cap() {
        let mut m = Machine::new();
        let t0 = Instant::now();
        // push_to_talk=false: CLI/SIGUSR toggles and the toggle setting.
        press(&mut m, t0, "transcribe", "fn", true);
        assert!(m.next_deadline().is_some());
        let fx = m.on_timeout(t0 + HANDS_FREE_MAX);
        assert_eq!(stops(&fx), 1);
    }

    #[test]
    fn hold_has_no_deadline_and_never_caps() {
        let mut m = Machine::new();
        let t0 = Instant::now();
        press(&mut m, t0, "transcribe", "fn", false);
        assert!(m.next_deadline().is_none(), "holds are never capped");
        let fx = release(
            &mut m,
            t0 + Duration::from_secs(25 * 60),
            "transcribe",
            "fn",
            false,
        );
        assert_eq!(stops(&fx), 1);
    }
}
