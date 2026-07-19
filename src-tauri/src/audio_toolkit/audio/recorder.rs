use std::{
    io::Error,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, Sample, SizedSample,
};

use crate::audio_toolkit::{
    audio::{AudioVisualiser, FrameResampler},
    constants,
    vad::{self, VadFrame},
    VoiceActivityDetector,
};

/// Per-recording auto-gain parameters. `NORMAL` is the always-on gentle boost
/// every dictation gets; Whisper Mode passes hotter targets and caps
/// (see `defaults::agc_params`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgcParams {
    pub enabled: bool,
    pub target_rms: f32,
    pub max_gain: f32,
}

impl AgcParams {
    pub const NORMAL: AgcParams = AgcParams {
        enabled: true,
        target_rms: 0.05,
        max_gain: 4.0,
    };
}

/// Whisper Mode vetoes beyond the loudness ceiling (round 20). Each veto is a
/// second, independent reason to reject sound as "not the whisperer at the
/// keyboard": a PITCHED (voiced) source - whispers have no pitch, so a voiced
/// talker rejects no matter how quietly it reaches the mic - and a clearly
/// REVERBERANT (far-from-mic) source, the single-mic proximity proxy.
/// Disabled vetoes never step their analyzers, so they cost nothing; in
/// normal mode (no ceiling) none of this runs at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WhisperVetoes {
    pub voicing_enabled: bool,
    /// Smoothed voiced probability above this (with signal present) rejects.
    pub voiced_block_threshold: f32,
    pub dryness_enabled: bool,
    /// Dryness score below this (with signal present) rejects as too far.
    pub dryness_block_threshold: f32,
    /// Neither veto fires below this raw frame rms (never veto silence).
    pub energy_floor: f32,
}

impl WhisperVetoes {
    /// Normal mode / whisper off: both vetoes disabled (thresholds are the
    /// tuned defaults so enabling one flag is enough in tests).
    pub const OFF: WhisperVetoes = WhisperVetoes {
        voicing_enabled: false,
        voiced_block_threshold: 0.65,
        dryness_enabled: false,
        dryness_block_threshold: 0.20,
        energy_floor: 0.004,
    };
}

/// Everything a recording session tunes on the capture path: the auto-gain
/// params, the VAD speech gate, and (Whisper Mode) the loudness ceiling above
/// which raw input is ignored entirely plus the voicing/dryness vetoes. Built
/// per session by `defaults::capture_tuning` and carried on `Cmd::Start`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptureTuning {
    pub agc: AgcParams,
    pub vad_threshold: f32,
    pub loudness_ceiling: Option<f32>,
    pub vetoes: WhisperVetoes,
}

enum Cmd {
    Start(VadPolicy, CaptureTuning),
    Stop(mpsc::Sender<Vec<f32>>),
    Shutdown,
}

enum AudioChunk {
    Samples(Vec<f32>),
    EndOfStream,
}

/// How 16 kHz mono frames should be filtered for one recording session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VadPolicy {
    /// Bypass VAD and forward every frame.
    Disabled,
    /// Current offline-tuned VAD profile.
    Offline,
    /// VAD profile with a longer post-speech tail for streaming-capable models.
    Streaming,
}

/// A single VAD engine plus the two hangover-tail lengths its smoothing wrapper
/// should use. The offline and streaming policies are never active
/// concurrently, so one detector is reconfigured per session (see `Cmd::Start`)
/// rather than kept as two resident engines.
#[derive(Clone)]
struct VadConfig {
    detector: Arc<Mutex<Box<dyn vad::VoiceActivityDetector>>>,
    offline_hangover_frames: usize,
    streaming_hangover_frames: usize,
}

impl VadConfig {
    /// Post-speech hangover tail (in 30 ms frames) for the given policy.
    /// `Disabled` never reaches the detector, so it maps to the offline value.
    fn hangover_for(&self, policy: VadPolicy) -> usize {
        match policy {
            VadPolicy::Streaming => self.streaming_hangover_frames,
            VadPolicy::Offline | VadPolicy::Disabled => self.offline_hangover_frames,
        }
    }
}

/// Callback invoked with each 16 kHz mono frame that passes the active capture
/// policy while recording. Used to feed a live streaming transcription as audio arrives.
pub type AudioFrameCallback = Arc<dyn Fn(&[f32]) + Send + Sync + 'static>;

pub struct AudioRecorder {
    device: Option<Device>,
    cmd_tx: Option<mpsc::Sender<Cmd>>,
    worker_handle: Option<std::thread::JoinHandle<()>>,
    vad: Option<VadConfig>,
    level_cb: Option<Arc<dyn Fn(Vec<f32>) + Send + Sync + 'static>>,
    audio_cb: Option<AudioFrameCallback>,
}

impl AudioRecorder {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(AudioRecorder {
            device: None,
            cmd_tx: None,
            worker_handle: None,
            vad: None,
            level_cb: None,
            audio_cb: None,
        })
    }

    /// Attach a single VAD engine, reconfigured per session for the offline vs
    /// streaming hangover tail. The two policies are mutually exclusive within a
    /// recording, so one engine covers both instead of two resident instances.
    pub fn with_vad(
        mut self,
        detector: Box<dyn VoiceActivityDetector>,
        offline_hangover_frames: usize,
        streaming_hangover_frames: usize,
    ) -> Self {
        self.vad = Some(VadConfig {
            detector: Arc::new(Mutex::new(detector)),
            offline_hangover_frames,
            streaming_hangover_frames,
        });
        self
    }

    pub fn with_level_callback<F>(mut self, cb: F) -> Self
    where
        F: Fn(Vec<f32>) + Send + Sync + 'static,
    {
        self.level_cb = Some(Arc::new(cb));
        self
    }

    /// Register a callback that receives real-time 16 kHz frames after the active
    /// VAD policy has been applied. Frames arrive in real time, in order, on the
    /// recorder's consumer thread, keep the callback cheap (e.g. forward to a
    /// channel) so it never stalls capture.
    pub fn with_audio_callback<F>(mut self, cb: F) -> Self
    where
        F: Fn(&[f32]) + Send + Sync + 'static,
    {
        self.audio_cb = Some(Arc::new(cb));
        self
    }

    pub fn open(&mut self, device: Option<Device>) -> Result<(), Box<dyn std::error::Error>> {
        if self.worker_handle.is_some() {
            return Ok(()); // already open
        }

        let (sample_tx, sample_rx) = mpsc::channel::<AudioChunk>();
        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<(), String>>(1);

        let host = crate::audio_toolkit::get_cpal_host();
        let device = match device {
            Some(dev) => dev,
            None => host
                .default_input_device()
                .ok_or_else(|| Error::new(std::io::ErrorKind::NotFound, "No input device found"))?,
        };

        let thread_device = device.clone();
        let vad = self.vad.clone();
        // Move the optional level callback into the worker thread
        let level_cb = self.level_cb.clone();
        // Move the optional real-time audio frame callback into the worker thread
        let audio_cb = self.audio_cb.clone();

        let worker = std::thread::spawn(move || {
            let stop_flag = Arc::new(AtomicBool::new(false));
            let stop_flag_for_stream = stop_flag.clone();
            let init_result = (|| -> Result<(cpal::Stream, u32), String> {
                let config = AudioRecorder::get_preferred_config(&thread_device)
                    .map_err(|e| format!("Failed to fetch preferred config: {e}"))?;

                let sample_rate = config.sample_rate().0;
                let channels = config.channels() as usize;

                log::info!(
                    "Using device: {:?}\nSample rate: {}\nChannels: {}\nFormat: {:?}",
                    thread_device.name(),
                    sample_rate,
                    channels,
                    config.sample_format()
                );

                let stream = match config.sample_format() {
                    cpal::SampleFormat::U8 => AudioRecorder::build_stream::<u8>(
                        &thread_device,
                        &config,
                        sample_tx,
                        channels,
                        stop_flag_for_stream,
                    )
                    .map_err(|e| format!("Failed to build input stream: {e}"))?,
                    cpal::SampleFormat::I8 => AudioRecorder::build_stream::<i8>(
                        &thread_device,
                        &config,
                        sample_tx,
                        channels,
                        stop_flag_for_stream,
                    )
                    .map_err(|e| format!("Failed to build input stream: {e}"))?,
                    cpal::SampleFormat::I16 => AudioRecorder::build_stream::<i16>(
                        &thread_device,
                        &config,
                        sample_tx,
                        channels,
                        stop_flag_for_stream,
                    )
                    .map_err(|e| format!("Failed to build input stream: {e}"))?,
                    cpal::SampleFormat::I32 => AudioRecorder::build_stream::<i32>(
                        &thread_device,
                        &config,
                        sample_tx,
                        channels,
                        stop_flag_for_stream,
                    )
                    .map_err(|e| format!("Failed to build input stream: {e}"))?,
                    cpal::SampleFormat::F32 => AudioRecorder::build_stream::<f32>(
                        &thread_device,
                        &config,
                        sample_tx,
                        channels,
                        stop_flag_for_stream,
                    )
                    .map_err(|e| format!("Failed to build input stream: {e}"))?,
                    sample_format => {
                        return Err(format!("Unsupported sample format: {sample_format:?}"));
                    }
                };

                stream
                    .play()
                    .map_err(|e| format!("Failed to start microphone stream: {e}"))?;

                Ok((stream, sample_rate))
            })();

            match init_result {
                Ok((stream, sample_rate)) => {
                    let _ = init_tx.send(Ok(()));
                    // Keep the stream alive while we process samples.
                    run_consumer(
                        sample_rate,
                        vad,
                        sample_rx,
                        cmd_rx,
                        level_cb,
                        audio_cb,
                        stop_flag,
                    );
                    drop(stream);
                }
                Err(error_message) => {
                    log::error!("{error_message}");
                    let _ = init_tx.send(Err(error_message));
                }
            }
        });

        match init_rx.recv() {
            Ok(Ok(())) => {
                self.device = Some(device);
                self.cmd_tx = Some(cmd_tx);
                self.worker_handle = Some(worker);
                Ok(())
            }
            Ok(Err(error_message)) => {
                let _ = worker.join();
                let kind = if is_microphone_access_denied(&error_message) {
                    std::io::ErrorKind::PermissionDenied
                } else {
                    std::io::ErrorKind::Other
                };
                Err(Box::new(Error::new(kind, error_message)))
            }
            Err(recv_error) => {
                let _ = worker.join();
                Err(Box::new(Error::other(format!(
                    "Failed to initialize microphone worker: {recv_error}"
                ))))
            }
        }
    }

    pub fn start(
        &self,
        vad_policy: VadPolicy,
        tuning: CaptureTuning,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(tx) = &self.cmd_tx {
            tx.send(Cmd::Start(vad_policy, tuning))?;
        }
        Ok(())
    }

    pub fn stop(&self) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
        let (resp_tx, resp_rx) = mpsc::channel();
        if let Some(tx) = &self.cmd_tx {
            tx.send(Cmd::Stop(resp_tx))?;
        }
        Ok(resp_rx.recv()?) // wait for the samples
    }

    pub fn close(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(Cmd::Shutdown);
        }
        if let Some(h) = self.worker_handle.take() {
            let _ = h.join();
        }
        self.device = None;
        Ok(())
    }

    fn build_stream<T>(
        device: &cpal::Device,
        config: &cpal::SupportedStreamConfig,
        sample_tx: mpsc::Sender<AudioChunk>,
        channels: usize,
        stop_flag: Arc<AtomicBool>,
    ) -> Result<cpal::Stream, cpal::BuildStreamError>
    where
        T: Sample + SizedSample + Send + 'static,
        f32: cpal::FromSample<T>,
    {
        let mut output_buffer = Vec::new();
        let mut eos_sent = false;

        let stream_cb = move |data: &[T], _: &cpal::InputCallbackInfo| {
            if stop_flag.load(Ordering::Relaxed) {
                if !eos_sent {
                    let _ = sample_tx.send(AudioChunk::EndOfStream);
                    eos_sent = true;
                }
                return;
            }
            eos_sent = false;

            output_buffer.clear();

            if channels == 1 {
                output_buffer.extend(data.iter().map(|&sample| sample.to_sample::<f32>()));
            } else {
                let frame_count = data.len() / channels;
                output_buffer.reserve(frame_count);

                for frame in data.chunks_exact(channels) {
                    let mono_sample = frame
                        .iter()
                        .map(|&sample| sample.to_sample::<f32>())
                        .sum::<f32>()
                        / channels as f32;
                    output_buffer.push(mono_sample);
                }
            }

            if sample_tx
                .send(AudioChunk::Samples(output_buffer.clone()))
                .is_err()
            {
                log::error!("Failed to send samples");
            }
        };

        device.build_input_stream(
            &config.clone().into(),
            stream_cb,
            |err| log::error!("Stream error: {}", err),
            None,
        )
    }

    fn get_preferred_config(
        device: &cpal::Device,
    ) -> Result<cpal::SupportedStreamConfig, Box<dyn std::error::Error>> {
        // Use the device's native/default sample rate and let the FrameResampler
        // in run_consumer() downsample to 16kHz. This avoids forcing hardware into
        // a non-native rate which can cause issues on some devices (Bluetooth
        // codecs, certain ALSA drivers, etc.).
        let default_config = device.default_input_config()?;
        let target_rate = default_config.sample_rate();

        // Try to find the best sample format at the device's default rate
        let supported_configs = match device.supported_input_configs() {
            Ok(configs) => configs,
            Err(e) => {
                log::warn!("Could not enumerate input configs ({e}), using device default");
                return Ok(default_config);
            }
        };
        let mut best_config: Option<cpal::SupportedStreamConfigRange> = None;

        for config_range in supported_configs {
            if config_range.min_sample_rate() <= target_rate
                && config_range.max_sample_rate() >= target_rate
            {
                match best_config {
                    None => best_config = Some(config_range),
                    Some(ref current) => {
                        // Prioritize F32 > I16 > I32 > others
                        let score = |fmt: cpal::SampleFormat| match fmt {
                            cpal::SampleFormat::F32 => 4,
                            cpal::SampleFormat::I16 => 3,
                            cpal::SampleFormat::I32 => 2,
                            _ => 1,
                        };

                        if score(config_range.sample_format()) > score(current.sample_format()) {
                            best_config = Some(config_range);
                        }
                    }
                }
            }
        }

        if let Some(config) = best_config {
            return Ok(config.with_sample_rate(target_rate));
        }

        // Fall back to device default if no config matched (exotic/virtual devices)
        log::warn!(
            "No supported config matched device default rate {:?}, using default config",
            target_rate
        );
        Ok(default_config)
    }
}

pub fn is_microphone_access_denied(error_message: &str) -> bool {
    let normalized = error_message.to_lowercase();
    normalized.contains("access is denied")
        || normalized.contains("permission denied")
        || normalized.contains("0x80070005")
}

pub fn is_no_input_device_error(error_message: &str) -> bool {
    let normalized = error_message.to_lowercase();
    normalized.contains("no input device found")
        || (normalized.contains("failed to fetch preferred config")
            && normalized.contains("coreaudio"))
}

#[cfg(test)]
mod tests {
    use super::{
        is_microphone_access_denied, is_no_input_device_error, AgcParams, AutoGain, LoudnessGate,
        WhisperGate, WhisperVetoes,
    };

    #[test]
    fn whisper_gate_normal_mode_is_pure_passthrough() {
        // No ceiling = normal mode: nothing blocks and the veto analyzers
        // never step, even with the vetoes nominally enabled (the resource
        // contract: whisper off costs nothing).
        let vetoes = WhisperVetoes {
            voicing_enabled: true,
            dryness_enabled: true,
            ..WhisperVetoes::OFF
        };
        let mut gate = WhisperGate::new(None, vetoes);
        let loud_tone: Vec<f32> = (0..480)
            .map(|i| 0.5 * (std::f32::consts::TAU * 150.0 * i as f32 / 16_000.0).sin())
            .collect();
        for _ in 0..100 {
            assert!(!gate.blocks(&loud_tone));
        }
        let st = gate.stats();
        assert_eq!(st.blocked_loud + st.blocked_voiced + st.blocked_reverb, 0);
        assert_eq!(gate.voiced_n, 0, "voicing must not run in normal mode");
        assert_eq!(gate.dry_n, 0, "dryness must not run in normal mode");
    }

    #[test]
    fn voicing_veto_blocks_a_quiet_voiced_talker_that_loudness_misses() {
        // A voiced tone BELOW the loudness ceiling: the loudness gate passes
        // it, the voicing veto rejects it. This is the whole point of the
        // veto: a quiet TALKING voice is not a whisper.
        let vetoes = WhisperVetoes {
            voicing_enabled: true,
            ..WhisperVetoes::OFF
        };
        let mut gate = WhisperGate::new(Some(0.030), vetoes);
        let n = 16_000 * 2;
        let quiet_voiced: Vec<f32> = (0..n)
            .map(|i| 0.015 * (std::f32::consts::TAU * 140.0 * i as f32 / 16_000.0).sin())
            .collect();
        let (mut blocked, mut total) = (0u32, 0u32);
        for frame in quiet_voiced.chunks(480) {
            if gate.blocks(frame) {
                blocked += 1;
            }
            total += 1;
        }
        assert!(
            blocked * 100 / total > 60,
            "quiet voiced source should be vetoed: {blocked}/{total}"
        );
        let st = gate.stats();
        assert_eq!(st.blocked_loud, 0, "the loudness gate must not have fired");
        assert!(st.blocked_voiced > 0);
    }

    #[test]
    fn whisper_noise_passes_the_voicing_veto() {
        // Whisper-like unvoiced noise below the ceiling must flow through.
        let vetoes = WhisperVetoes {
            voicing_enabled: true,
            ..WhisperVetoes::OFF
        };
        let mut gate = WhisperGate::new(Some(0.030), vetoes);
        let mut state = 99u64;
        let mut prev = 0.0f32;
        let n = 16_000 * 2;
        let whisper: Vec<f32> = (0..n)
            .map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                let w = ((state >> 33) as f32 / u32::MAX as f32) * 2.0 - 1.0;
                let hp = w - prev;
                prev = w;
                0.010 * hp
            })
            .collect();
        let (mut blocked, mut total) = (0u32, 0u32);
        for frame in whisper.chunks(480) {
            if gate.blocks(frame) {
                blocked += 1;
            }
            total += 1;
        }
        assert!(
            blocked * 100 / total < 10,
            "whisper-like noise must pass: {blocked}/{total} blocked"
        );
    }

    #[test]
    fn loudness_gate_passes_quiet_speech_and_blocks_sustained_loud() {
        let quiet: Vec<f32> = vec![0.015; 480]; // the owner's quiet talking
        let loud: Vec<f32> = vec![0.08; 480]; // projected voice / TV

        // No ceiling (normal mode): nothing ever blocks.
        let mut open_gate = LoudnessGate::new(None);
        assert!(!open_gate.blocks(&loud));
        assert!(!open_gate.blocks(&quiet));

        // Medium ceiling: an entire quiet sentence passes uninterrupted.
        let mut gate = LoudnessGate::new(Some(0.030));
        for _ in 0..100 {
            assert!(!gate.blocks(&quiet));
        }

        // A sustained loud source closes the gate within ~1s...
        let mut closed_at = None;
        for i in 0..60 {
            if gate.blocks(&loud) {
                closed_at = Some(i);
                break;
            }
        }
        let closed_at = closed_at.expect("gate must close on sustained loud sound");
        assert!(closed_at <= 30, "closed after {closed_at} frames");
        // ...and stays closed while the loud source continues.
        for _ in 0..50 {
            assert!(gate.blocks(&loud));
        }
        // Sustained quiet reopens it and quiet speech flows again.
        let mut reopened = false;
        for _ in 0..200 {
            if !gate.blocks(&quiet) {
                reopened = true;
                break;
            }
        }
        assert!(reopened, "gate must reopen after quiet returns");
        for _ in 0..20 {
            assert!(!gate.blocks(&quiet));
        }
    }

    #[test]
    fn loudness_gate_does_not_chop_words_on_brief_peaks() {
        let quiet: Vec<f32> = vec![0.015; 480];
        let peak: Vec<f32> = vec![0.06; 480]; // a loud vowel (~4x the mean)
        let mut gate = LoudnessGate::new(Some(0.030));
        for _ in 0..50 {
            assert!(!gate.blocks(&quiet));
        }
        // A 4-frame (~120ms) loud vowel inside a quiet sentence must NOT close
        // the gate: the slow ambient envelope barely lifts over one syllable,
        // never reaching the close-confirm count. This is the word-chopping
        // regression row (real-speech peaks are shorter/softer than this).
        for _ in 0..4 {
            assert!(!gate.blocks(&peak));
        }
        for _ in 0..30 {
            assert!(!gate.blocks(&quiet));
        }
        let (seen, blocked, mean, max) = gate.stats();
        assert_eq!(seen, 84);
        assert_eq!(blocked, 0);
        assert!((max - 0.06).abs() < 1e-4);
        assert!(mean > 0.01 && mean < 0.03);
    }

    #[test]
    fn loudness_gate_closes_on_a_loud_voice_despite_word_gaps() {
        // A loud talking voice: loud word-bursts separated by short gaps. A
        // single-envelope gate leaked here (each gap reset the close counter,
        // measured 11% rejected); the slow ambient envelope rides over the
        // gaps and keeps the gate closed.
        let word: Vec<f32> = vec![0.10; 480];
        let gap: Vec<f32> = vec![0.004; 480];
        let mut gate = LoudnessGate::new(Some(0.030));
        let (mut blocked, mut total) = (0u32, 0u32);
        for _ in 0..25 {
            for _ in 0..15 {
                if gate.blocks(&word) {
                    blocked += 1;
                }
                total += 1;
            }
            for _ in 0..8 {
                if gate.blocks(&gap) {
                    blocked += 1;
                }
                total += 1;
            }
        }
        assert!(
            blocked * 100 / total > 50,
            "a loud voice must be mostly rejected: {blocked}/{total}"
        );
        // A real, sustained pause (not a word-gap) reopens the gate so quiet
        // speech is captured again.
        let quiet: Vec<f32> = vec![0.012; 480];
        let mut reopened = false;
        for _ in 0..60 {
            if !gate.blocks(&quiet) {
                reopened = true;
                break;
            }
        }
        assert!(reopened, "gate must reopen after a sustained pause");
    }

    #[test]
    fn auto_gain_boosts_to_the_params_cap() {
        // A steady whisper-level frame (~0.005 RMS at 16 kHz / 30 ms).
        let quiet: Vec<f32> = vec![0.005; 480];
        let mut normal = AutoGain::new(AgcParams::NORMAL);
        let mut whisper = AutoGain::new(AgcParams {
            enabled: true,
            target_rms: 0.10,
            max_gain: 20.0,
        });
        for _ in 0..400 {
            normal.process(&quiet);
            whisper.process(&quiet);
        }
        // Normal stays at its gentle 4x cap; whisper approaches its hot cap.
        assert!((normal.gain - AgcParams::NORMAL.max_gain).abs() < 0.2);
        assert!(whisper.gain > 15.0);
        // A loud frame makes the gain yield instantly (no clipping pump).
        let loud: Vec<f32> = vec![0.5; 480];
        whisper.process(&loud);
        assert!(whisper.gain < 2.0);
    }

    #[test]
    fn detects_access_is_denied() {
        assert!(is_microphone_access_denied("Access is denied"));
    }

    #[test]
    fn detects_permission_denied() {
        assert!(is_microphone_access_denied("permission denied"));
    }

    #[test]
    fn detects_windows_error_code() {
        assert!(is_microphone_access_denied("WASAPI error: 0x80070005"));
    }

    #[test]
    fn does_not_match_unrelated_errors() {
        assert!(!is_microphone_access_denied("device not found"));
    }

    #[test]
    fn detects_no_input_device() {
        assert!(is_no_input_device_error("No input device found"));
    }

    #[test]
    fn detects_coreaudio_config_error() {
        assert!(is_no_input_device_error(
            "Failed to fetch preferred config: A backend-specific error has occurred: An unknown error unknown to the coreaudio-rs API occurred"
        ));
    }

    #[test]
    fn does_not_match_other_errors_for_no_device() {
        assert!(!is_no_input_device_error("permission denied"));
        assert!(!is_no_input_device_error("device not found"));
    }
}

/// Whisper Mode auto-gain: up-only AGC applied after VAD, right where frames
/// fan out to BOTH the batch buffer and the streaming router, so quiet speech
/// is boosted identically for live partials and final decode. Never attenuates
/// (gain >= 1), capped at `params.max_gain` (4x normally, hotter in Whisper
/// Mode), rises slowly between words and yields instantly when the speaker
/// gets loud (no pumping, no clip).
struct AutoGain {
    params: AgcParams,
    rms_sq_ema: f32,
    gain: f32,
    scratch: Vec<f32>,
}
const AGC_RMS_ALPHA: f32 = 0.15;
const AGC_RISE_ALPHA: f32 = 0.06;

impl AutoGain {
    fn new(params: AgcParams) -> Self {
        Self {
            params,
            rms_sq_ema: params.target_rms * params.target_rms,
            gain: 1.0,
            scratch: Vec::new(),
        }
    }
    fn reset(&mut self, params: AgcParams) {
        self.params = params;
        self.rms_sq_ema = params.target_rms * params.target_rms;
        self.gain = 1.0;
    }
    /// Process one frame; returns the (possibly boosted) samples.
    fn process<'a>(&'a mut self, frame: &'a [f32]) -> &'a [f32] {
        if frame.is_empty() {
            return frame;
        }
        let sum_sq: f32 = frame.iter().map(|s| s * s).sum();
        let frame_rms_sq = sum_sq / frame.len() as f32;
        self.rms_sq_ema += (frame_rms_sq - self.rms_sq_ema) * AGC_RMS_ALPHA;
        let rms = self.rms_sq_ema.sqrt().max(1e-6);
        let desired = (self.params.target_rms / rms).clamp(1.0, self.params.max_gain);
        if desired < self.gain {
            self.gain = desired; // yield fast when speech gets loud
        } else {
            self.gain += (desired - self.gain) * AGC_RISE_ALPHA;
        }
        if self.gain <= 1.001 {
            return frame;
        }
        self.scratch.clear();
        self.scratch
            .extend(frame.iter().map(|s| (s * self.gain).clamp(-1.0, 1.0)));
        &self.scratch
    }
}

/// Whisper Mode loudness gate: the strength ladder's ceiling. Each strength
/// accepts its loudness band AND everything quieter, and rejects anything
/// louder as background noise (Light: normal speech and below; Medium: quiet
/// speech and below; High: whisper and below).
///
/// Decisions are made per SPEECH BURST, not per 30ms frame: speech loudness
/// swings 10-20dB within a word (vowels vs gaps), so a per-frame ceiling
/// chops the loud syllables out of a quiet sentence and hands the STT
/// swiss-cheese audio (measured: 35% of frames gated MID-SPEECH). The gate
/// tracks the RAW frame RMS through TWO envelopes because the two directions
/// need opposite time constants:
/// - `slow_ema` (~750ms) = the AMBIENT SOURCE level. It rides over the
///   word-gaps in speech, so a loud TALKING voice (which pauses every ~1-2s)
///   still reads as sustained-loud instead of resetting on every gap. Drives
///   CLOSING: slow_ema > ceiling for GATE_CLOSE_CONFIRM_FRAMES closes the gate
///   (the ~200ms onset leak errs toward capture; the session log shows it).
/// - `fast_ema` (responsive) drives REOPENING: fast_ema < ceiling *
///   GATE_REOPEN_RATIO (hysteresis, so the boundary never flaps) for
///   GATE_OPEN_CONFIRM_FRAMES (~360ms of SUSTAINED quiet, so a 150-300ms
///   word-gap in loud speech does NOT reopen mid-burst, but a real pause
///   does). A single fast envelope let loud speech leak (each word-gap reset
///   the close counter); splitting the directions fixed it (validated in sim
///   over the owner's real speech: loud-voice rejection 11% -> 68%, quiet
///   capture stays 0% gated).
/// Both envelopes read RAW pre-boost RMS - the booster erases loudness.
/// `None` = gate open forever (normal mode). Also tracks per-session
/// raw-level stats for the capture log, ceiling or not.
struct LoudnessGate {
    slow_ema: f32,
    fast_ema: f32,
    over_count: u8,
    under_count: u8,
    closed: bool,
    ceiling: Option<f32>,
    frames_seen: u32,
    frames_blocked: u32,
    rms_sum: f64,
    rms_max: f32,
}
const GATE_SLOW_ALPHA: f32 = 0.04; // ~750ms: ambient source level, gap-tolerant
const GATE_FAST_ALPHA: f32 = 0.20; // responsive: for the reopen decision
const GATE_CLOSE_CONFIRM_FRAMES: u8 = 6; // sustained-loud (slow ema) to close
const GATE_OPEN_CONFIRM_FRAMES: u8 = 12; // ~360ms sustained-quiet (fast) to reopen
const GATE_REOPEN_RATIO: f32 = 0.8;

impl LoudnessGate {
    fn new(ceiling: Option<f32>) -> Self {
        Self {
            slow_ema: 0.0,
            fast_ema: 0.0,
            over_count: 0,
            under_count: 0,
            closed: false,
            ceiling,
            frames_seen: 0,
            frames_blocked: 0,
            rms_sum: 0.0,
            rms_max: 0.0,
        }
    }
    fn reset(&mut self, ceiling: Option<f32>) {
        *self = Self::new(ceiling);
    }
    /// (frames seen, frames blocked, mean raw rms, max raw rms) this session.
    fn stats(&self) -> (u32, u32, f32, f32) {
        let mean = if self.frames_seen > 0 {
            (self.rms_sum / self.frames_seen as f64) as f32
        } else {
            0.0
        };
        (self.frames_seen, self.frames_blocked, mean, self.rms_max)
    }
    /// Feed one raw frame; true = drop it (a louder source is active).
    #[cfg(test)]
    fn blocks(&mut self, frame: &[f32]) -> bool {
        if frame.is_empty() {
            return false;
        }
        let frame_rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
        self.blocks_rms(frame_rms)
    }
    /// Same decision from a precomputed raw frame rms (the WhisperGate
    /// computes the rms once and shares it across analyzers).
    fn blocks_rms(&mut self, frame_rms: f32) -> bool {
        self.frames_seen += 1;
        self.rms_sum += frame_rms as f64;
        self.rms_max = self.rms_max.max(frame_rms);
        let Some(ceiling) = self.ceiling else {
            return false;
        };
        self.slow_ema += (frame_rms - self.slow_ema) * GATE_SLOW_ALPHA;
        self.fast_ema += (frame_rms - self.fast_ema) * GATE_FAST_ALPHA;

        if self.closed {
            // Reopen on the responsive envelope, but only after sustained quiet.
            if self.fast_ema < ceiling * GATE_REOPEN_RATIO {
                self.under_count = self.under_count.saturating_add(1);
                if self.under_count >= GATE_OPEN_CONFIRM_FRAMES {
                    self.closed = false;
                    self.over_count = 0;
                    self.under_count = 0;
                    // Snap the slow envelope down to the (now-quiet) fast one so
                    // its lingering tail from the loud stretch does not
                    // immediately re-trip the close and flap the gate.
                    self.slow_ema = self.fast_ema;
                    return false; // the quiet stretch resumes with this frame
                }
            } else {
                self.under_count = 0;
            }
            self.frames_blocked += 1;
            return true;
        }

        // Close on the slow ambient level so word-gaps in loud speech do not
        // reset the accumulation.
        if self.slow_ema > ceiling {
            self.over_count = self.over_count.saturating_add(1);
            if self.over_count >= GATE_CLOSE_CONFIRM_FRAMES {
                self.closed = true;
                self.over_count = 0;
                self.under_count = 0;
                self.frames_blocked += 1;
                return true;
            }
        } else {
            self.over_count = 0;
        }
        false
    }
}

/// Frames of sustained voicing before the veto engages (anti-flutter), and
/// frames of hangover after voicing stops, so word boundaries do not chop.
const VOICED_CONFIRM_FRAMES: u8 = 3;
const VOICED_HANGOVER_FRAMES: u8 = 10;

/// Per-session Whisper Mode gate stats for the capture log (tuning data).
struct WhisperGateStats {
    frames_seen: u32,
    blocked_loud: u32,
    blocked_voiced: u32,
    blocked_reverb: u32,
    mean_rms: f32,
    max_rms: f32,
    /// Mean smoothed voiced probability over analyzed frames (0 when off).
    mean_voiced: f32,
    /// Mean dryness score over analyzed frames (0 when off).
    mean_dryness: f32,
}

/// The full Whisper Mode gate (round 20): the validated dual-envelope
/// loudness ceiling FUSED with the voicing veto (whispers have no pitch, so
/// sustained PITCHED speech rejects regardless of level) and the dryness veto
/// (clearly reverberant sound is far from the mic; the proximity proxy).
/// Priority when several fire at once: loudness, then voiced, then far.
///
/// Resource contract: in normal mode (no ceiling) `blocks` is a pure
/// passthrough after the loudness stats update - the analyzers never step. A
/// disabled veto never steps its analyzer either. When everything is on the
/// per-frame cost is ~180k multiply-adds (voicing autocorrelation) plus
/// microseconds of envelope stats: well under 0.1% of one core.
struct WhisperGate {
    loudness: LoudnessGate,
    voicing: super::voicing::VoicingDetector,
    dryness: super::dryness::DrynessAnalyzer,
    vetoes: WhisperVetoes,
    voiced_run: u8,
    voiced_hang: u8,
    blocked_voiced: u32,
    blocked_reverb: u32,
    voiced_sum: f64,
    voiced_n: u32,
    dry_sum: f64,
    dry_n: u32,
}

impl WhisperGate {
    fn new(ceiling: Option<f32>, vetoes: WhisperVetoes) -> Self {
        Self {
            loudness: LoudnessGate::new(ceiling),
            voicing: super::voicing::VoicingDetector::new(),
            dryness: super::dryness::DrynessAnalyzer::new(),
            vetoes,
            voiced_run: 0,
            voiced_hang: 0,
            blocked_voiced: 0,
            blocked_reverb: 0,
            voiced_sum: 0.0,
            voiced_n: 0,
            dry_sum: 0.0,
            dry_n: 0,
        }
    }

    fn reset(&mut self, ceiling: Option<f32>, vetoes: WhisperVetoes) {
        *self = Self::new(ceiling, vetoes);
    }

    /// Whisper Mode is on for this session (a loudness ceiling is set).
    fn whisper_active(&self) -> bool {
        self.loudness.ceiling.is_some()
    }

    fn stats(&self) -> WhisperGateStats {
        let (frames_seen, blocked_loud, mean_rms, max_rms) = self.loudness.stats();
        WhisperGateStats {
            frames_seen,
            blocked_loud,
            blocked_voiced: self.blocked_voiced,
            blocked_reverb: self.blocked_reverb,
            mean_rms,
            max_rms,
            mean_voiced: if self.voiced_n > 0 {
                (self.voiced_sum / self.voiced_n as f64) as f32
            } else {
                0.0
            },
            mean_dryness: if self.dry_n > 0 {
                (self.dry_sum / self.dry_n as f64) as f32
            } else {
                0.0
            },
        }
    }

    /// Feed one RAW (pre-boost) frame; true = drop it.
    fn blocks(&mut self, frame: &[f32]) -> bool {
        if frame.is_empty() {
            return false;
        }
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
        let loud = self.loudness.blocks_rms(rms);
        if !self.whisper_active() {
            // Normal mode: nothing beyond the (inert) loudness stats runs.
            return false;
        }

        // Analyzers step on every whisper-mode frame their veto is enabled
        // for (even while another gate is closed) so their state stays warm.
        let voiced_blocks = if self.vetoes.voicing_enabled {
            let p = self.voicing.step(frame);
            self.voiced_sum += p as f64;
            self.voiced_n += 1;
            let voiced_now =
                p > self.vetoes.voiced_block_threshold && rms > self.vetoes.energy_floor;
            if voiced_now {
                self.voiced_run = self.voiced_run.saturating_add(1);
                if self.voiced_run >= VOICED_CONFIRM_FRAMES {
                    self.voiced_hang = VOICED_HANGOVER_FRAMES;
                }
            } else {
                self.voiced_run = 0;
                self.voiced_hang = self.voiced_hang.saturating_sub(1);
            }
            self.voiced_hang > 0
        } else {
            false
        };

        let dry_blocks = if self.vetoes.dryness_enabled {
            let d = self.dryness.step(rms);
            self.dry_sum += d as f64;
            self.dry_n += 1;
            d < self.vetoes.dryness_block_threshold && rms > self.vetoes.energy_floor
        } else {
            false
        };

        if loud {
            return true; // LoudnessGate already counted this block
        }
        if voiced_blocks {
            self.blocked_voiced += 1;
            return true;
        }
        if dry_blocks {
            self.blocked_reverb += 1;
            return true;
        }
        false
    }
}

#[allow(clippy::too_many_arguments)]
fn run_consumer(
    in_sample_rate: u32,
    vad: Option<VadConfig>,
    sample_rx: mpsc::Receiver<AudioChunk>,
    cmd_rx: mpsc::Receiver<Cmd>,
    level_cb: Option<Arc<dyn Fn(Vec<f32>) + Send + Sync + 'static>>,
    audio_cb: Option<AudioFrameCallback>,
    stop_flag: Arc<AtomicBool>,
) {
    let mut frame_resampler = FrameResampler::new(
        in_sample_rate as usize,
        constants::WHISPER_SAMPLE_RATE as usize,
        Duration::from_millis(30),
    );

    let mut processed_samples = Vec::<f32>::new();
    let mut recording = false;
    let mut vad_policy = VadPolicy::Offline;
    let mut agc_params = AgcParams {
        enabled: false,
        ..AgcParams::NORMAL
    };
    let mut agc = AutoGain::new(agc_params);
    let mut gate = WhisperGate::new(None, WhisperVetoes::OFF);

    // ---------- spectrum visualisation setup ---------------------------- //
    const BUCKETS: usize = 16;
    // Scale the FFT window to the device sample rate so the analysis window
    // (~33 ms) and frequency resolution (~30 Hz/bin) stay roughly constant
    // across devices. A fixed 512-sample window collapses the low vocal
    // buckets onto a single bin at 48 kHz (e.g. built-in laptop mics), and
    // would stutter at ~4-8 updates/sec on an 8-16 kHz Bluetooth headset.
    // Targets: 48 kHz -> 2048, 16 kHz -> 512, 8 kHz -> 256.
    let target_window = (f64::from(in_sample_rate) / 30.0).round() as usize;
    let window_size = [256usize, 512, 1024, 2048]
        .into_iter()
        .min_by_key(|w| w.abs_diff(target_window))
        .unwrap();
    let mut visualizer = AudioVisualiser::new(
        in_sample_rate,
        window_size,
        BUCKETS,
        400.0,  // vocal_min_hz
        4000.0, // vocal_max_hz
    );

    #[allow(clippy::too_many_arguments)]
    fn handle_frame(
        samples: &[f32],
        recording: bool,
        vad_policy: VadPolicy,
        vad: &Option<VadConfig>,
        audio_cb: &Option<AudioFrameCallback>,
        out_buf: &mut Vec<f32>,
        agc: &mut AutoGain,
        agc_enabled: bool,
        gate: &mut WhisperGate,
    ) {
        if !recording {
            return;
        }

        // Whisper Mode: raw input above the loudness ceiling, or vetoed as
        // pitched/far, is ignored entirely (never reaches the VAD, the
        // booster, or the buffers).
        if gate.blocks(samples) {
            return;
        }

        fn deliver(buf: &[f32], out_buf: &mut Vec<f32>, audio_cb: &Option<AudioFrameCallback>) {
            out_buf.extend_from_slice(buf);
            if let Some(cb) = audio_cb {
                cb(buf);
            }
        }

        // Whisper Mode boosts BEFORE the speech detector: a whisper on a quiet
        // mic is too small for Silero raw, so it must score the amplified
        // frame. Normal mode keeps the proven raw-VAD -> boost order.
        if gate.whisper_active() {
            let boosted: &[f32] = if agc_enabled {
                agc.process(samples)
            } else {
                samples
            };
            if vad_policy == VadPolicy::Disabled {
                deliver(boosted, out_buf, audio_cb);
                return;
            }
            if let Some(cfg) = vad {
                let mut det = cfg.detector.lock().unwrap();
                match det.push_frame(boosted).unwrap_or(VadFrame::Speech(boosted)) {
                    VadFrame::Speech(buf) => deliver(buf, out_buf, audio_cb),
                    VadFrame::Noise => {}
                }
            } else {
                deliver(boosted, out_buf, audio_cb);
            }
            return;
        }

        let mut emit = |buf: &[f32]| {
            let buf = if agc_enabled { agc.process(buf) } else { buf };
            deliver(buf, out_buf, audio_cb);
        };

        if vad_policy == VadPolicy::Disabled {
            emit(samples);
            return;
        }

        if let Some(cfg) = vad {
            let mut det = cfg.detector.lock().unwrap();
            match det.push_frame(samples).unwrap_or(VadFrame::Speech(samples)) {
                VadFrame::Speech(buf) => emit(buf),
                VadFrame::Noise => {}
            }
        } else {
            emit(samples);
        }
    }

    // Runs until the stream closes and `recv` returns `Err`.
    while let Ok(chunk) = sample_rx.recv() {
        let raw = match chunk {
            AudioChunk::Samples(s) => s,
            AudioChunk::EndOfStream => continue,
        };

        // ---------- spectrum processing ---------------------------------- //
        if let Some(buckets) = visualizer.feed(&raw) {
            if let Some(cb) = &level_cb {
                cb(buckets);
            }
        }

        // ---------- existing pipeline ------------------------------------ //
        frame_resampler.push(&raw, &mut |frame: &[f32]| {
            handle_frame(
                frame,
                recording,
                vad_policy,
                &vad,
                &audio_cb,
                &mut processed_samples,
                &mut agc,
                agc_params.enabled,
                &mut gate,
            )
        });

        // non-blocking check for a command
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Cmd::Start(policy, tuning) => {
                    stop_flag.store(false, Ordering::Relaxed);
                    vad_policy = policy;
                    agc_params = tuning.agc;
                    agc.reset(agc_params);
                    gate.reset(tuning.loudness_ceiling, tuning.vetoes);
                    processed_samples.clear();
                    recording = true;
                    visualizer.reset();
                    // Reconfigure the single VAD engine for this session's policy
                    // and clear its smoothing + recurrent state before it sees
                    // any frames.
                    if vad_policy != VadPolicy::Disabled {
                        if let Some(cfg) = &vad {
                            let mut det = cfg.detector.lock().unwrap();
                            det.set_hangover_frames(cfg.hangover_for(vad_policy));
                            det.set_threshold(tuning.vad_threshold);
                            det.reset();
                        }
                    }
                }
                Cmd::Stop(reply_tx) => {
                    recording = false;
                    stop_flag.store(true, Ordering::Relaxed);

                    // Drain all remaining audio until the producer confirms end-of-stream.
                    // The cpal callback sees the stop flag, sends EndOfStream, and goes
                    // silent, guaranteeing every captured sample is in the channel
                    // ahead of the sentinel.
                    loop {
                        match sample_rx.recv_timeout(Duration::from_secs(2)) {
                            Ok(AudioChunk::Samples(remaining)) => {
                                frame_resampler.push(&remaining, &mut |frame: &[f32]| {
                                    handle_frame(
                                        frame,
                                        true,
                                        vad_policy,
                                        &vad,
                                        &audio_cb,
                                        &mut processed_samples,
                                        &mut agc,
                                        agc_params.enabled,
                                        &mut gate,
                                    )
                                });
                            }
                            Ok(AudioChunk::EndOfStream) => break,
                            Err(_) => {
                                log::warn!("Timed out waiting for EndOfStream from audio callback");
                                break;
                            }
                        }
                    }

                    frame_resampler.finish(&mut |frame: &[f32]| {
                        handle_frame(
                            frame,
                            true,
                            vad_policy,
                            &vad,
                            &audio_cb,
                            &mut processed_samples,
                            &mut agc,
                            agc_params.enabled,
                            &mut gate,
                        )
                    });

                    // One line per session so any "whisper mode is not
                    // working" report is a log read: what the mic actually
                    // measured vs the ceiling that gated it.
                    let st = gate.stats();
                    log::info!(
                        "capture session: raw rms mean {:.4} max {:.4} over {} frames, gated {} loud / {} voiced / {} far (ceiling {}); voiced mean {:.2}, dryness mean {:.2}",
                        st.mean_rms,
                        st.max_rms,
                        st.frames_seen,
                        st.blocked_loud,
                        st.blocked_voiced,
                        st.blocked_reverb,
                        match gate.loudness.ceiling {
                            Some(c) => format!("{:.4}", c),
                            None => "none".to_string(),
                        },
                        st.mean_voiced,
                        st.mean_dryness
                    );

                    let _ = reply_tx.send(std::mem::take(&mut processed_samples));

                    // Resume the audio callback so the consumer loop can continue
                    // receiving chunks (important for always-on microphone mode).
                    stop_flag.store(false, Ordering::Relaxed);
                }
                Cmd::Shutdown => {
                    stop_flag.store(true, Ordering::Relaxed);
                    return;
                }
            }
        }
    }
}
