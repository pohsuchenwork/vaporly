//! Fixed internal defaults for behavior that Vaporly no longer exposes as
//! settings. Every const here was an AppSettings field in v1; the value is the
//! v1 default. Consumers read these instead of the store, so the code paths
//! stay intact (and testable) while the settings surface stays small.

use crate::audio_toolkit::{AgcParams, CaptureTuning, WhisperVetoes};
use crate::settings::{FeatureLevel, WhisperStrength};

/// How pasting is performed. Not user-configurable in v2: clipboard paste via
/// Ctrl/Cmd+V everywhere, direct typing on Linux (matches the v1 defaults).
/// The full v1 variant set is kept on purpose (paste code still matches it).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteMethod {
    CtrlV,
    Direct,
    None,
    ShiftInsert,
    CtrlShiftV,
    ExternalScript,
}

#[cfg(target_os = "linux")]
pub const PASTE_METHOD: PasteMethod = PasteMethod::Direct;
#[cfg(not(target_os = "linux"))]
pub const PASTE_METHOD: PasteMethod = PasteMethod::CtrlV;

/// Delay between writing the clipboard and sending the paste keystroke.
pub const PASTE_DELAY_MS: u64 = 60;

/// Linux typing-tool preference for PasteMethod::Direct (auto-detect chain).
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypingTool {
    Auto,
    Wtype,
    Kwtype,
    Dotool,
    Ydotool,
    Xdotool,
}

#[cfg(target_os = "linux")]
pub const TYPING_TOOL: TypingTool = TypingTool::Auto;

/// Voice activity detection is always on in v2.
pub const VAD_ENABLED: bool = true;

/// The base speech-probability gate for the Silero VAD. Whisper Mode relaxes
/// it per strength (see `whisper_vad_threshold`).
pub const VAD_BASE_THRESHOLD: f32 = 0.3;

/// Per-recording auto-gain parameters. The gentle up-only AGC is always on;
/// Whisper Mode raises the target level and the gain cap so quiet speech is
/// boosted hard enough to transcribe.
pub fn agc_params(whisper_mode: bool, strength: WhisperStrength) -> AgcParams {
    if !whisper_mode {
        return AgcParams::NORMAL;
    }
    let (target_rms, max_gain) = match strength {
        WhisperStrength::Light => (0.06, 8.0),
        WhisperStrength::Medium => (0.08, 12.0),
        WhisperStrength::High => (0.10, 20.0),
    };
    AgcParams {
        enabled: true,
        target_rms,
        max_gain,
    }
}

/// The VAD speech gate per Whisper Mode state: whispers score lower on the
/// speech-probability model, so the gate relaxes with the strength.
pub fn whisper_vad_threshold(whisper_mode: bool, strength: WhisperStrength) -> f32 {
    if !whisper_mode {
        return VAD_BASE_THRESHOLD;
    }
    match strength {
        WhisperStrength::Light | WhisperStrength::Medium => 0.22,
        WhisperStrength::High => 0.15,
    }
}

/// Whisper Mode loudness ceiling: the strength ladder. Each strength accepts
/// its loudness band AND everything quieter; anything louder is background
/// noise and produces no text. Light = normal speech and below (rejects only
/// loud sound); Medium = quiet speech and below (rejects a projected talking
/// voice); High = whisper/murmur only.
///
/// SEMANTICS: these compare against the gate's slow-attack/fast-release
/// loudness ENVELOPE (the level of the current sound source), not against
/// instantaneous 30ms frames - per-frame ceilings chopped the loud syllables
/// out of quiet sentences (measured 35% of frames gated mid-speech).
/// CALIBRATION: from the owner's measured sessions on this machine's virtual
/// mic (quiet talking ~0.010-0.022 session-mean raw RMS; the per-session
/// capture log prints measured rms vs the ceiling, so retuning is a log
/// read, not a guess).
pub fn whisper_loudness_ceiling(whisper_mode: bool, strength: WhisperStrength) -> Option<f32> {
    if !whisper_mode {
        return None;
    }
    Some(match strength {
        WhisperStrength::Light => 0.080,
        WhisperStrength::Medium => 0.030,
        WhisperStrength::High => 0.012,
    })
}

/// Whisper Mode vetoes beyond the loudness ceiling (round 20).
///
/// PROXIMITY (dryness): active at EVERY strength but deliberately
/// conservative - only clearly reverberant, washy sound (far from the mic)
/// rejects. Single-mic distance sensing is the weakest of the three signals
/// (the robust literature methods are two-mic), so the threshold sits well
/// below the lab default of 0.35 until real-clip tuning or per-mic
/// calibration raises it.
///
/// VOICING: High only. High means "a true whisper and nothing else", and a
/// whisper is physically unvoiced (no pitch), so sustained PITCHED speech
/// rejects no matter how quietly it reaches the mic - the case a loudness
/// ceiling cannot catch. Light and Medium accept voiced speech by design.
pub fn whisper_vetoes(whisper_mode: bool, strength: WhisperStrength) -> WhisperVetoes {
    if !whisper_mode {
        return WhisperVetoes::OFF;
    }
    WhisperVetoes {
        voicing_enabled: strength == WhisperStrength::High,
        voiced_block_threshold: 0.65,
        dryness_enabled: true,
        dryness_block_threshold: 0.20,
        energy_floor: 0.004,
    }
}

/// The full per-session capture tuning from the whisper settings: gain params,
/// VAD speech gate, the loudness ceiling, and the voicing/dryness vetoes, in
/// one bundle for AudioRecorder::start.
pub fn capture_tuning(whisper_mode: bool, strength: WhisperStrength) -> CaptureTuning {
    CaptureTuning {
        agc: agc_params(whisper_mode, strength),
        vad_threshold: whisper_vad_threshold(whisper_mode, strength),
        loudness_ceiling: whisper_loudness_ceiling(whisper_mode, strength),
        vetoes: whisper_vetoes(whisper_mode, strength),
    }
}

/// Extra tail capture after the stop keypress. v1 defaulted to none.
pub const EXTRA_RECORDING_BUFFER_MS: u64 = 0;

/// The microphone opens on demand per dictation (never held open).
pub const ALWAYS_ON_MICROPHONE: bool = false;

/// System output is never muted while recording.
pub const MUTE_WHILE_RECORDING: bool = false;

/// The on-demand microphone stream closes immediately after each dictation.
pub const LAZY_STREAM_CLOSE: bool = false;

/// How long a loaded STT model may sit idle before it is unloaded. The full
/// variant set is kept on purpose (the unload logic matches on it).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelUnloadTimeout {
    Never,
    Immediately,
    Min2,
    #[default]
    Min5,
    Min10,
    Min15,
    Hour1,
    Sec15,
}

impl ModelUnloadTimeout {
    pub fn to_minutes(self) -> Option<u64> {
        match self {
            ModelUnloadTimeout::Never => None,
            ModelUnloadTimeout::Immediately => Some(0),
            ModelUnloadTimeout::Min2 => Some(2),
            ModelUnloadTimeout::Min5 => Some(5),
            ModelUnloadTimeout::Min10 => Some(10),
            ModelUnloadTimeout::Min15 => Some(15),
            ModelUnloadTimeout::Hour1 => Some(60),
            ModelUnloadTimeout::Sec15 => Some(0),
        }
    }

    pub fn to_seconds(self) -> Option<u64> {
        match self {
            ModelUnloadTimeout::Never => None,
            ModelUnloadTimeout::Immediately => Some(0),
            ModelUnloadTimeout::Sec15 => Some(15),
            _ => self.to_minutes().map(|m| m * 60),
        }
    }
}

pub const MODEL_UNLOAD_TIMEOUT: ModelUnloadTimeout = ModelUnloadTimeout::Min5;

/// Compute accelerator for STT decodes. Auto defers to the hardware probe
/// (which binds CPU in VMs where the paravirtual GPU is slower than CPU).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscribeAcceleratorSetting {
    Auto,
    Cpu,
    Gpu,
}

pub const TRANSCRIBE_ACCELERATOR: TranscribeAcceleratorSetting = TranscribeAcceleratorSetting::Auto;

/// GPU device registry index for STT loads. -1 = auto (first match).
pub const TRANSCRIBE_GPU_DEVICE: i32 = -1;

/// Accelerator for the bundled llama.cpp cleanup engine. Auto defers to the
/// hardware probe (Metal on real Apple Silicon, CPU in VMs and elsewhere).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmAcceleratorSetting {
    Auto,
    Cpu,
    Gpu,
}

pub const LLM_ENGINE_ACCELERATOR: LlmAcceleratorSetting = LlmAcceleratorSetting::Auto;

/// Fuzzy custom-word correction threshold per aggressiveness level. Medium is
/// byte-identical to the v1 default (0.18). Off never reaches the stage; 0.0
/// (match nothing) keeps the function total.
pub fn word_threshold(level: FeatureLevel) -> f64 {
    match level {
        FeatureLevel::Off => 0.0,
        FeatureLevel::Light => 0.10,
        FeatureLevel::Medium => 0.18,
        FeatureLevel::High => 0.28,
    }
}

/// Fuzzy custom-phrase trigger threshold per aggressiveness level. Medium is
/// byte-identical to the previously hardcoded PHRASE_MATCH_THRESHOLD (0.25).
/// Off never reaches the stage; 0.0 (match nothing) keeps the function total.
/// Single-word triggers stay exact-only at every level (the precision rule in
/// apply_custom_phrases).
pub fn phrase_threshold(level: FeatureLevel) -> f64 {
    match level {
        FeatureLevel::Off => 0.0,
        FeatureLevel::Light => 0.12,
        FeatureLevel::Medium => 0.25,
        FeatureLevel::High => 0.35,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn medium_threshold_matches_v1_default() {
        assert_eq!(word_threshold(FeatureLevel::Medium), 0.18);
        assert_eq!(word_threshold(FeatureLevel::Light), 0.10);
        assert_eq!(word_threshold(FeatureLevel::High), 0.28);
        assert_eq!(word_threshold(FeatureLevel::Off), 0.0);
    }

    #[test]
    fn unload_timeout_default_is_five_minutes() {
        assert_eq!(MODEL_UNLOAD_TIMEOUT.to_minutes(), Some(5));
        assert_eq!(ModelUnloadTimeout::Sec15.to_seconds(), Some(15));
        assert_eq!(ModelUnloadTimeout::Never.to_seconds(), None);
    }
}
