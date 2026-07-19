use log::{debug, warn};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashMap;
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct ShortcutBinding {
    pub id: String,
    pub name: String,
    pub description: String,
    pub default_binding: String,
    pub current_binding: String,
}

/// Custom Phrases: spoken trigger -> written replacement ("btw" -> "by the
/// way", "write my email format" -> a saved template). Applied
/// deterministically after STT (audio_toolkit::apply_custom_phrases).
#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct CustomPhrase {
    pub say: String,
    pub write: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "lowercase")]
pub enum OverlayPosition {
    Top,
    Bottom,
}

/// Which recording overlay to display.
/// - `None`: no overlay at all.
/// - `Bar`: compact status pill (waveform, spinner, result flash).
/// - `BarLive`: the pill grows into a panel showing live transcription text.
/// - `TextboxRaw`: stream committed words straight into the target app, then
///   repair them to the cleaned final text at finish (stream_inject.rs). The
///   overlay stays a compact status pill.
/// - `TextboxClean`: stream cleaned sentences into the target app as each
///   completes (deterministic sentences, or LiveCleaner chunks when a model
///   plan is active). Status pill only.
/// - `Inline`: stream the deterministically filtered live text into the
///   target app UNDERLINED (combining low line per grapheme), polishing each
///   completed sentence in place while the user still speaks; on release the
///   underline goes away and the clean final text stands. Status pill only.
///   All on-textbox styles degrade to Bar behavior for a dictation whose
///   injection preflight fails.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum OverlayStyle {
    None,
    Bar,
    BarLive,
    TextboxRaw,
    TextboxClean,
    /// Alias `wispr` kept so stores written before the rename still load.
    #[serde(alias = "wispr")]
    Inline,
}

/// Theme mode for both webview windows: follow the OS, or force light/dark.
/// The frontend stamps data-theme from this; the backend only stores it.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

/// Curated accent preset. Each maps to a verified light/dark stop set in the
/// frontend (src/styles/applyAccent.ts); the backend only stores the choice.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum AccentPreset {
    #[default]
    Sakura,
    Rose,
    Amber,
    Green,
    Blue,
    Violet,
}

/// Whisper Mode boost strength (whisper_mode itself is the on/off; there is
/// no Off strength).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum WhisperStrength {
    Light,
    #[default]
    Medium,
    High,
}

/// Per-mic Whisper Mode calibration (round 20): measured levels from the
/// optional ~8 s wizard plus the ceilings/floor derived from them. Stored
/// with the microphone it was measured on and IGNORED when a different mic
/// is active (a headset calibration must never govern the built-in mic).
/// When absent (or the device differs) the gate uses the strength defaults.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Type)]
pub struct WhisperCalibration {
    /// The microphone this was measured on ("default" = the system default).
    pub device_name: String,
    /// Measured mean raw frame rms per wizard phase.
    pub ambient_rms: f32,
    pub normal_rms: f32,
    pub whisper_rms: f32,
    /// Recommended loudness ceilings per strength (envelope semantics).
    pub light_ceiling: f32,
    pub medium_ceiling: f32,
    pub high_ceiling: f32,
    /// Veto energy floor derived from the room's ambient level.
    pub energy_floor: f32,
    /// Whisper/normal separation verdict: "good", "workable", or "poor".
    pub separation: String,
}

/// Aggressiveness dial shared by the cleanup stages (custom words, filler
/// fix up, mind-changing check).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum FeatureLevel {
    Off,
    Light,
    #[default]
    Medium,
    High,
}

/// Per-stage engine choice: deterministic rules or the local cleanup model.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum StageEngine {
    #[default]
    Deterministic,
    Model,
}

/// How context awareness adapts output to the frontmost app: deterministic
/// per-category rules, a model prompt hint, or both.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContextMode {
    #[default]
    Deterministic,
    Model,
    Both,
}

/// Auto-learning of custom words (F4). WatchPostPaste is the macOS AX
/// observation mode, experimental by contract.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutoLearnMode {
    #[default]
    Off,
    HistoryEdits,
    RepeatedWords,
    Both,
    WatchPostPaste,
}

/// Context awareness: which target-app categories adapt formatting, and via
/// which engine.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
pub struct ContextAwarenessSettings {
    pub email: bool,
    pub chat: bool,
    pub code: bool,
    pub browser: bool,
    pub notes: bool,
    pub general: bool,
    pub mode: ContextMode,
}

impl ContextAwarenessSettings {
    pub fn any_enabled(&self) -> bool {
        self.email || self.chat || self.code || self.browser || self.notes || self.general
    }
}

impl Default for ContextAwarenessSettings {
    fn default() -> Self {
        Self {
            email: true,
            chat: true,
            code: true,
            browser: true,
            notes: true,
            general: true,
            mode: ContextMode::Deterministic,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum RecordingRetentionPeriod {
    Never,
    PreserveLimit,
    Days3,
    Weeks2,
    Months3,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardImplementation {
    Tauri,
    #[serde(rename = "vaporly_native", alias = "handy_keys")]
    VaporlyNative,
}

impl Default for KeyboardImplementation {
    fn default() -> Self {
        #[cfg(target_os = "linux")]
        return KeyboardImplementation::Tauri;
        #[cfg(not(target_os = "linux"))]
        return KeyboardImplementation::VaporlyNative;
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum SoundTheme {
    Marimba,
    Pop,
    Chime,
    Bubble,
    Breeze,
    Custom,
}

impl SoundTheme {
    fn as_str(&self) -> &'static str {
        match self {
            SoundTheme::Marimba => "marimba",
            SoundTheme::Pop => "pop",
            SoundTheme::Chime => "chime",
            SoundTheme::Bubble => "bubble",
            SoundTheme::Breeze => "breeze",
            SoundTheme::Custom => "custom",
        }
    }

    pub fn to_start_path(self) -> String {
        format!("resources/{}_start.wav", self.as_str())
    }

    pub fn to_stop_path(self) -> String {
        format!("resources/{}_stop.wav", self.as_str())
    }
}

/// Vaporly settings, fresh schema (version 1). No serde aliases and no value
/// migrations: v2 starts with its own store. Unreadable stores are quarantined
/// (see below) and replaced with defaults.
#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct AppSettings {
    /// Settings schema marker for future one-time migrations.
    #[serde(default = "default_settings_schema_version")]
    pub settings_schema_version: u32,
    #[serde(default)]
    pub onboarding_completed: bool,
    pub bindings: HashMap<String, ShortcutBinding>,
    #[serde(default)]
    pub keyboard_implementation: KeyboardImplementation,
    /// One-time dismissal of the macOS globe-key tip shown when a binding
    /// uses fn (System Settings, Keyboard, "Press globe key to" = Do Nothing).
    #[serde(default)]
    pub globe_key_notice_dismissed: bool,
    #[serde(default)]
    pub selected_microphone: Option<String>,
    #[serde(default)]
    pub audio_feedback: bool,
    #[serde(default = "default_audio_feedback_volume")]
    pub audio_feedback_volume: f32,
    #[serde(default = "default_sound_theme")]
    pub sound_theme: SoundTheme,
    #[serde(default)]
    pub selected_output_device: Option<String>,
    /// Theme mode: follow the OS or force light/dark (both windows).
    #[serde(default)]
    pub theme_mode: ThemeMode,
    /// Accent color preset (both windows).
    #[serde(default)]
    pub accent_preset: AccentPreset,
    /// Whisper Mode: boost quiet speech (hotter auto-gain + a relaxed VAD
    /// gate) so whispering still transcribes. Applies from the next dictation.
    #[serde(default)]
    pub whisper_mode: bool,
    /// How hard Whisper Mode boosts (gain target/cap + VAD relax).
    #[serde(default)]
    pub whisper_strength: WhisperStrength,
    /// Optional per-mic calibration from the wizard; None = strength defaults.
    #[serde(default)]
    pub whisper_calibration: Option<WhisperCalibration>,
    #[serde(default = "default_overlay_style")]
    pub overlay_style: OverlayStyle,
    #[serde(default = "default_overlay_position")]
    pub overlay_position: OverlayPosition,
    /// Custom-word fuzzy correction aggressiveness (Off disables the stage).
    #[serde(default)]
    pub custom_words_level: FeatureLevel,
    #[serde(default)]
    pub custom_words: Vec<String>,
    #[serde(default)]
    pub custom_phrases: Vec<CustomPhrase>,
    /// Custom-phrase trigger matching aggressiveness (Off disables the stage;
    /// single-word triggers stay exact at every level).
    #[serde(default)]
    pub custom_phrases_level: FeatureLevel,
    /// Filler fix up dial (um/uh removal and friends).
    #[serde(default)]
    pub filler_level: FeatureLevel,
    #[serde(default)]
    pub filler_engine: StageEngine,
    /// Mind-changing check dial ("at eight, no wait, nine" -> "at nine").
    /// Default: High on the MODEL engine (the deterministic Medium pass alone
    /// misses too many spoken corrections; High+Model resolves explicit
    /// retractions with the LLM while the deterministic stages still do
    /// numbers/words/caps first).
    #[serde(default = "default_mind_change_level")]
    pub mind_change_level: FeatureLevel,
    #[serde(default = "default_mind_change_engine")]
    pub mind_change_engine: StageEngine,
    #[serde(default)]
    pub context_awareness: ContextAwarenessSettings,
    #[serde(default)]
    pub auto_learn_mode: AutoLearnMode,
    /// After pasting, keep the final text on the clipboard (true) or restore
    /// the previous clipboard contents (false).
    #[serde(default)]
    pub keep_result_on_clipboard: bool,
    #[serde(default = "default_true")]
    pub append_trailing_space: bool,
    #[serde(default)]
    pub autostart_enabled: bool,
    #[serde(default = "default_true")]
    pub update_checks_enabled: bool,
    #[serde(default = "default_history_limit")]
    pub history_limit: usize,
    #[serde(default = "default_recording_retention_period")]
    pub recording_retention_period: RecordingRetentionPeriod,
    /// Cleanup model for the bundled engine. Empty = the hardware ladder's
    /// recommendation (7B on >=14GB machines, 1.5B on 6-14GB, none below).
    #[serde(default)]
    pub llm_model_id: String,
}

impl AppSettings {
    /// Whether any cleanup stage needs the local LLM: gates the lazy engine
    /// and the per-dictation model pass. Delegates to the pipeline's free
    /// function (the canonical home since F1) so callers stay stable.
    pub fn model_pass_needed(&self) -> bool {
        crate::pipeline::config::model_pass_needed(self)
    }
}

pub const CURRENT_SETTINGS_SCHEMA_VERSION: u32 = 1;

fn default_settings_schema_version() -> u32 {
    CURRENT_SETTINGS_SCHEMA_VERSION
}

fn default_audio_feedback_volume() -> f32 {
    1.0
}

fn default_sound_theme() -> SoundTheme {
    SoundTheme::Marimba
}

fn default_overlay_style() -> OverlayStyle {
    OverlayStyle::BarLive
}

fn default_mind_change_level() -> FeatureLevel {
    FeatureLevel::High
}

fn default_mind_change_engine() -> StageEngine {
    StageEngine::Model
}

fn default_overlay_position() -> OverlayPosition {
    OverlayPosition::Bottom
}

fn default_true() -> bool {
    true
}

fn default_history_limit() -> usize {
    100
}

fn default_recording_retention_period() -> RecordingRetentionPeriod {
    RecordingRetentionPeriod::PreserveLimit
}

pub const SETTINGS_STORE_PATH: &str = "settings_store.json";

/// Registrable fallback for the Tauri keyboard backend, which cannot bind
/// the fn key. Used when the backend switches to Tauri (or native init
/// fails) and the stored binding contains fn.
pub fn tauri_safe_fallback(binding_id: &str) -> Option<&'static str> {
    match binding_id {
        "transcribe" => Some("option+space"),
        _ => None,
    }
}

/// Whether a hotkey string uses the fn modifier (any position).
pub fn binding_uses_fn(hotkey: &str) -> bool {
    hotkey.split('+').any(|part| {
        part.trim().eq_ignore_ascii_case("fn") || part.trim().eq_ignore_ascii_case("function")
    })
}

pub fn get_default_settings() -> AppSettings {
    // ONE dictation key: hold to talk, double-tap to lock
    // hands-free (20 minute cap). Only the VaporlyNative backend can register
    // fn; shortcut::tauri_safe_fallback rewrites it when the Tauri backend is
    // active. Windows/Linux default to a ctrl combo.
    #[cfg(target_os = "macos")]
    let default_shortcut = "fn";
    #[cfg(not(target_os = "macos"))]
    let default_shortcut = "ctrl+space";

    let mut bindings = HashMap::new();
    bindings.insert(
        "transcribe".to_string(),
        ShortcutBinding {
            id: "transcribe".to_string(),
            name: "Dictate".to_string(),
            description: "Hold and speak; double-tap to lock hands-free.".to_string(),
            default_binding: default_shortcut.to_string(),
            current_binding: default_shortcut.to_string(),
        },
    );
    bindings.insert(
        "cancel".to_string(),
        ShortcutBinding {
            id: "cancel".to_string(),
            name: "Cancel".to_string(),
            description: "Cancels the current recording.".to_string(),
            default_binding: "escape".to_string(),
            current_binding: "escape".to_string(),
        },
    );
    // Optional dedicated hands-free toggle: unbound out of the box (empty
    // bindings are skipped by every registrar). A single press toggles via the
    // coordinator's CLI path; the fn double-tap latch keeps working besides it.
    bindings.insert(
        "hands_free".to_string(),
        ShortcutBinding {
            id: "hands_free".to_string(),
            name: "Hands-free".to_string(),
            description: "Press once to start hands-free, again to stop.".to_string(),
            default_binding: String::new(),
            current_binding: String::new(),
        },
    );
    // Optional Whisper Mode toggle: unbound out of the box.
    bindings.insert(
        "whisper_toggle".to_string(),
        ShortcutBinding {
            id: "whisper_toggle".to_string(),
            name: "Whisper Mode".to_string(),
            description: "Toggles Whisper Mode on or off.".to_string(),
            default_binding: String::new(),
            current_binding: String::new(),
        },
    );

    AppSettings {
        settings_schema_version: default_settings_schema_version(),
        onboarding_completed: false,
        bindings,
        keyboard_implementation: KeyboardImplementation::default(),
        globe_key_notice_dismissed: false,
        selected_microphone: None,
        audio_feedback: true,
        audio_feedback_volume: default_audio_feedback_volume(),
        sound_theme: default_sound_theme(),
        selected_output_device: None,
        theme_mode: ThemeMode::default(),
        accent_preset: AccentPreset::default(),
        whisper_mode: false,
        whisper_strength: WhisperStrength::default(),
        whisper_calibration: None,
        overlay_style: default_overlay_style(),
        overlay_position: default_overlay_position(),
        custom_words_level: FeatureLevel::default(),
        custom_words: Vec::new(),
        custom_phrases: Vec::new(),
        custom_phrases_level: FeatureLevel::default(),
        filler_level: FeatureLevel::default(),
        filler_engine: StageEngine::default(),
        mind_change_level: default_mind_change_level(),
        mind_change_engine: default_mind_change_engine(),
        context_awareness: ContextAwarenessSettings::default(),
        auto_learn_mode: AutoLearnMode::default(),
        keep_result_on_clipboard: false,
        append_trailing_space: true,
        autostart_enabled: false,
        update_checks_enabled: true,
        history_limit: default_history_limit(),
        recording_retention_period: default_recording_retention_period(),
        llm_model_id: String::new(),
    }
}

/// Write an unreadable settings blob to `path`, pretty-printed. Pure (no
/// AppHandle) so it is unit-testable; returns whether the file was written.
fn write_settings_quarantine(path: &std::path::Path, settings_value: &serde_json::Value) -> bool {
    match serde_json::to_string_pretty(settings_value) {
        Ok(s) => match std::fs::write(path, s) {
            Ok(()) => true,
            Err(e) => {
                warn!("settings backup write failed at {}: {e}", path.display());
                false
            }
        },
        Err(e) => {
            warn!("settings backup serialize failed: {e}");
            false
        }
    }
}

/// Preserve an unreadable settings blob before it is replaced with defaults, so
/// a tester who cycles builds (a newer build writes an enum variant an older
/// build cannot parse) can recover their shortcuts and custom words instead of
/// silently losing them. Best-effort: a failure here never blocks startup, and
/// the quarantine is overwritten each time (last-known-bad).
fn quarantine_unreadable_settings(app: &AppHandle, settings_value: &serde_json::Value) {
    match crate::portable::resolve_app_data(app, "settings_store.corrupt.json") {
        Ok(path) => {
            if write_settings_quarantine(&path, settings_value) {
                warn!(
                    "settings failed to parse; backed up the unreadable settings to {} before resetting to defaults",
                    path.display()
                );
            }
        }
        Err(e) => warn!(
            "settings failed to parse and the data dir could not be resolved for backup ({e}); resetting to defaults"
        ),
    }
}

pub fn load_or_create_app_settings(app: &AppHandle) -> AppSettings {
    // Initialize store
    let store = app
        .store(crate::portable::store_path(SETTINGS_STORE_PATH))
        .expect("Failed to initialize store");

    if let Some(settings_value) = store.get("settings") {
        // Parse the entire settings object
        match serde_json::from_value::<AppSettings>(settings_value.clone()) {
            Ok(mut settings) => {
                debug!("Found existing settings: {:?}", settings);
                let default_settings = get_default_settings();
                let mut updated = false;

                // Merge default bindings into existing settings
                for (key, value) in default_settings.bindings {
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        settings.bindings.entry(key)
                    {
                        debug!("Adding missing binding: {}", entry.key());
                        entry.insert(value);
                        updated = true;
                    }
                }

                if updated {
                    debug!("Settings updated with defaults");
                    store.set("settings", serde_json::to_value(&settings).unwrap());
                }

                settings
            }
            Err(e) => {
                warn!("Failed to parse settings: {}", e);
                quarantine_unreadable_settings(app, &settings_value);
                // Fall back to default settings if parsing fails
                let default_settings = get_default_settings();
                store.set("settings", serde_json::to_value(&default_settings).unwrap());
                default_settings
            }
        }
    } else {
        let default_settings = get_default_settings();
        store.set("settings", serde_json::to_value(&default_settings).unwrap());
        default_settings
    }
}

pub fn get_settings(app: &AppHandle) -> AppSettings {
    let store = app
        .store(crate::portable::store_path(SETTINGS_STORE_PATH))
        .expect("Failed to initialize store");

    if let Some(settings_value) = store.get("settings") {
        match serde_json::from_value::<AppSettings>(settings_value.clone()) {
            Ok(settings) => settings,
            Err(e) => {
                warn!("Failed to parse settings: {}", e);
                quarantine_unreadable_settings(app, &settings_value);
                let default_settings = get_default_settings();
                store.set("settings", serde_json::to_value(&default_settings).unwrap());
                default_settings
            }
        }
    } else {
        let default_settings = get_default_settings();
        store.set("settings", serde_json::to_value(&default_settings).unwrap());
        default_settings
    }
}

pub fn write_settings(app: &AppHandle, settings: AppSettings) {
    let store = app
        .store(crate::portable::store_path(SETTINGS_STORE_PATH))
        .expect("Failed to initialize store");

    store.set("settings", serde_json::to_value(&settings).unwrap());
}

pub fn get_bindings(app: &AppHandle) -> HashMap<String, ShortcutBinding> {
    let settings = get_settings(app);

    settings.bindings
}

pub fn get_stored_binding(app: &AppHandle, id: &str) -> ShortcutBinding {
    let bindings = get_bindings(app);

    let binding = bindings.get(id).unwrap().clone();

    binding
}

pub fn get_history_limit(app: &AppHandle) -> usize {
    let settings = get_settings(app);
    settings.history_limit
}

pub fn get_recording_retention_period(app: &AppHandle) -> RecordingRetentionPeriod {
    let settings = get_settings(app);
    settings.recording_retention_period
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_defaults_match_the_v2_spec() {
        let s = get_default_settings();
        assert_eq!(s.settings_schema_version, CURRENT_SETTINGS_SCHEMA_VERSION);
        assert_eq!(s.settings_schema_version, 1);
        assert!(!s.onboarding_completed);
        assert!(s.bindings.contains_key("transcribe"));
        assert!(s.bindings.contains_key("cancel"));
        assert!(s.bindings.contains_key("hands_free"));
        assert!(s.bindings["hands_free"].current_binding.is_empty());
        assert!(s.bindings.contains_key("whisper_toggle"));
        assert!(s.bindings["whisper_toggle"].current_binding.is_empty());
        assert!(!s.globe_key_notice_dismissed);
        assert_eq!(s.selected_microphone, None);
        assert!(s.audio_feedback);
        assert_eq!(s.audio_feedback_volume, 1.0);
        assert_eq!(s.sound_theme, SoundTheme::Marimba);
        assert_eq!(s.selected_output_device, None);
        assert_eq!(s.theme_mode, ThemeMode::System);
        assert_eq!(s.accent_preset, AccentPreset::Sakura);
        assert!(!s.whisper_mode);
        assert_eq!(s.whisper_strength, WhisperStrength::Medium);
        assert_eq!(s.whisper_calibration, None);
        assert_eq!(s.overlay_style, OverlayStyle::BarLive);
        assert_eq!(s.overlay_position, OverlayPosition::Bottom);
        assert_eq!(s.custom_words_level, FeatureLevel::Medium);
        assert!(s.custom_words.is_empty());
        assert!(s.custom_phrases.is_empty());
        assert_eq!(s.custom_phrases_level, FeatureLevel::Medium);
        assert_eq!(s.filler_level, FeatureLevel::Medium);
        assert_eq!(s.filler_engine, StageEngine::Deterministic);
        // Defaults: mind-change runs High on the MODEL engine.
        assert_eq!(s.mind_change_level, FeatureLevel::High);
        assert_eq!(s.mind_change_engine, StageEngine::Model);
        assert!(s.context_awareness.any_enabled());
        assert!(
            s.context_awareness.email
                && s.context_awareness.chat
                && s.context_awareness.code
                && s.context_awareness.browser
                && s.context_awareness.notes
                && s.context_awareness.general
        );
        assert_eq!(s.context_awareness.mode, ContextMode::Deterministic);
        assert_eq!(s.auto_learn_mode, AutoLearnMode::Off);
        assert!(!s.keep_result_on_clipboard);
        assert!(s.append_trailing_space);
        assert!(!s.autostart_enabled);
        assert!(s.update_checks_enabled);
        assert_eq!(s.history_limit, 100);
        assert_eq!(
            s.recording_retention_period,
            RecordingRetentionPeriod::PreserveLimit
        );
        assert_eq!(s.llm_model_id, "");
    }

    #[test]
    fn defaults_want_the_model_pass_for_mind_change() {
        // Round 2: mind-change Light+Model is on out of the box, so a fresh
        // install DOES run the model pass (and the engine lazy-start hole is
        // closed by the dictation-start warm-up).
        assert!(get_default_settings().model_pass_needed());
        // Turning that one stage deterministic makes a fresh install fully
        // deterministic again: no engine, ever.
        let mut s = get_default_settings();
        s.mind_change_engine = StageEngine::Deterministic;
        assert!(!s.model_pass_needed());
    }

    /// Defaults with every stage forced Deterministic: the baseline for the
    /// matrix rows below.
    fn all_det_settings() -> AppSettings {
        let mut s = get_default_settings();
        s.mind_change_engine = StageEngine::Deterministic;
        s
    }

    #[test]
    fn model_pass_needed_matrix() {
        let mut s = all_det_settings();

        s.filler_engine = StageEngine::Model;
        assert!(s.model_pass_needed(), "filler on Model at Medium");
        s.filler_level = FeatureLevel::Off;
        assert!(!s.model_pass_needed(), "filler Model but Off level");

        s = all_det_settings();
        s.mind_change_engine = StageEngine::Model;
        assert!(
            s.model_pass_needed(),
            "mind-change engine set to Model needs the pass"
        );
        s.mind_change_level = FeatureLevel::Off;
        assert!(!s.model_pass_needed(), "mind-change Model but Off level");

        s = all_det_settings();
        s.context_awareness.mode = ContextMode::Model;
        assert!(s.model_pass_needed(), "context Model with categories on");
        s.context_awareness.mode = ContextMode::Both;
        assert!(s.model_pass_needed(), "context Both with categories on");
        s.context_awareness = ContextAwarenessSettings {
            email: false,
            chat: false,
            code: false,
            browser: false,
            notes: false,
            general: false,
            mode: ContextMode::Both,
        };
        assert!(!s.model_pass_needed(), "context Both but no category");
    }

    #[test]
    fn fresh_install_has_trailing_space_on() {
        assert!(get_default_settings().append_trailing_space);
    }

    #[test]
    fn serialized_defaults_carry_only_v2_keys() {
        // The store must never grow legacy keys back: serialize the defaults
        // and pin the exact key set.
        let value = serde_json::to_value(get_default_settings()).unwrap();
        let mut keys: Vec<String> = value
            .as_object()
            .expect("settings serialize to an object")
            .keys()
            .cloned()
            .collect();
        keys.sort();
        let expected = [
            "accent_preset",
            "append_trailing_space",
            "audio_feedback",
            "audio_feedback_volume",
            "auto_learn_mode",
            "autostart_enabled",
            "bindings",
            "context_awareness",
            "custom_phrases",
            "custom_phrases_level",
            "custom_words",
            "custom_words_level",
            "filler_engine",
            "filler_level",
            "globe_key_notice_dismissed",
            "history_limit",
            "keep_result_on_clipboard",
            "keyboard_implementation",
            "llm_model_id",
            "mind_change_engine",
            "mind_change_level",
            "onboarding_completed",
            "overlay_position",
            "overlay_style",
            "recording_retention_period",
            "selected_microphone",
            "selected_output_device",
            "settings_schema_version",
            "sound_theme",
            "theme_mode",
            "update_checks_enabled",
            "whisper_calibration",
            "whisper_mode",
            "whisper_strength",
        ];
        assert_eq!(keys, expected);
    }

    #[test]
    fn quarantine_preserves_unreadable_settings() {
        // A store blob that would fail to deserialize into AppSettings must be
        // written out verbatim before the reset, so nothing is silently lost.
        let dir =
            std::env::temp_dir().join(format!("vaporly-quarantine-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings_store.corrupt.json");
        let bad = serde_json::json!({
            "settings_schema_version": 999,
            "bogus_enum_field": "not_a_real_variant",
        });
        assert!(write_settings_quarantine(&path, &bad));
        let round = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&round).unwrap();
        assert_eq!(parsed["settings_schema_version"], 999);
        assert_eq!(parsed["bogus_enum_field"], "not_a_real_variant");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn binding_uses_fn_detects_all_positions() {
        assert!(binding_uses_fn("fn"));
        assert!(binding_uses_fn("fn+space"));
        assert!(binding_uses_fn("ctrl+FN+x"));
        assert!(!binding_uses_fn("ctrl+alt+h"));
        assert!(!binding_uses_fn("f5"));
    }

    #[test]
    fn overlay_style_round_trips_snake_case() {
        for (style, wire) in [
            (OverlayStyle::None, "\"none\""),
            (OverlayStyle::Bar, "\"bar\""),
            (OverlayStyle::BarLive, "\"bar_live\""),
            (OverlayStyle::TextboxRaw, "\"textbox_raw\""),
            (OverlayStyle::TextboxClean, "\"textbox_clean\""),
            (OverlayStyle::Inline, "\"inline\""),
        ] {
            assert_eq!(serde_json::to_string(&style).unwrap(), wire);
            let back: OverlayStyle = serde_json::from_str(wire).unwrap();
            assert_eq!(back, style);
        }
    }

    #[test]
    fn overlay_style_wispr_alias_still_loads() {
        // Stores written before the Wispr->Inline rename must still deserialize.
        let back: OverlayStyle = serde_json::from_str("\"wispr\"").unwrap();
        assert_eq!(back, OverlayStyle::Inline);
    }
}
