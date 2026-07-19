use crate::defaults::{ModelUnloadTimeout, TranscribeAcceleratorSetting};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::model::{EngineType, ModelManager};
use crate::settings::{get_settings, AppSettings};
use anyhow::Result;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Emitter, Manager};
use tauri_specta::Event;
use transcribe_cpp::{
    Backend, Feature, Model, ModelOptions, RunExtension, RunOptions, Session, StreamOptions, Task,
    TimestampKind, WhisperRunOptions,
};

const STREAM_PERF_LOG_INTERVAL: Duration = Duration::from_secs(5);
const STREAM_FINALIZE_REPLY_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Serialize)]
pub struct ModelStateEvent {
    pub event_type: String,
    pub model_id: Option<String>,
    pub model_name: Option<String>,
    pub error: Option<String>,
}

/// Live transcription snapshot emitted to the overlay during a streaming run.
/// `committed` is the append-only, flicker-free prefix; `tentative` is the
/// volatile suffix the model may still rewrite.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct StreamTextEvent {
    pub committed: String,
    pub tentative: String,
}

/// Phase of the streaming overlay card, emitted to drive its UI state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum StreamPhase {
    /// Receiving audio / live text (or waiting for the stream to begin). Rust
    /// does not emit this today; the frontend starts in this phase and Rust only
    /// emits transitions away from it.
    Listening,
    /// Finalizing or post-processing, show a spinner.
    Working,
}

/// Semantic kind of "working" phase, used to localize the spinner label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum StreamWorkKind {
    Transcribing,
    Polishing,
}

/// Emitted to switch the streaming overlay to a working spinner.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct StreamPhaseEvent {
    pub phase: StreamPhase,
    /// Present only when `phase` is `Working`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<StreamWorkKind>,
}

/// Commands sent to the streaming worker thread. Audio frames and the finalize
/// request travel the same channel so FIFO ordering guarantees every fed frame
/// is processed before finalize runs.
enum StreamCmd {
    // (variants below; Feed carries its SEND time: frames queue behind long
    // decodes, so only send-time stamps preserve real silence gaps)
    Feed(Vec<f32>, Instant),
    /// Flush the stream and reply with the final text, or `None` if no stream
    /// was ever active (caller should fall back to batch transcription).
    Finalize(mpsc::Sender<Option<String>>),
    Cancel,
}

/// Routes real-time audio frames to the active streaming worker. Shared between
/// the [`TranscriptionManager`] (opens/closes the route) and the audio recorder's
/// per-frame callback (feeds frames). The recorder holds an `Arc<StreamRouter>`
/// directly, so a frame with no stream pending costs a single relaxed atomic
/// load, no Tauri state lookup, no mutex lock.
pub struct StreamRouter {
    /// Command channel to the active streaming worker, present from
    /// `start_stream` until `finalize_stream`/`cancel_stream`.
    tx: Mutex<Option<mpsc::Sender<StreamCmd>>>,
    /// True while a stream is pending or active (channel is open). The audio
    /// callback checks this first to avoid the mutex lock when no stream runs.
    open: Arc<AtomicBool>,
}

impl StreamRouter {
    fn new() -> Self {
        Self {
            tx: Mutex::new(None),
            open: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Open a fresh command channel for a new streaming session, returning the
    /// receiver the worker should drain. Caller must ensure no prior channel is
    /// still open.
    fn open(&self) -> mpsc::Receiver<StreamCmd> {
        let (tx, rx) = mpsc::channel::<StreamCmd>();
        *self.tx.lock().unwrap() = Some(tx);
        self.open.store(true, Ordering::Relaxed);
        rx
    }

    /// Take the sender out (closing the channel to new feeds). Returns the
    /// sender so the caller can send the final `Finalize`/`Cancel` command.
    fn take(&self) -> Option<mpsc::Sender<StreamCmd>> {
        self.open.store(false, Ordering::Relaxed);
        self.tx.lock().unwrap().take()
    }

    /// Drop the channel and mark closed without sending a final command (used
    /// when the worker exits without a finalize/cancel handshake).
    fn clear(&self) {
        self.open.store(false, Ordering::Relaxed);
        *self.tx.lock().unwrap() = None;
    }

    /// Forward a 16 kHz frame to the active streaming worker. Cheap no-op (a
    /// single relaxed atomic load) when no stream is pending.
    pub fn feed(&self, frame: &[f32]) {
        if !self.open.load(Ordering::Relaxed) {
            return;
        }
        if let Some(tx) = self.tx.lock().unwrap().as_ref() {
            let _ = tx.send(StreamCmd::Feed(frame.to_vec(), Instant::now()));
        }
    }

    /// Whether a stream is pending or active.
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Relaxed)
    }
}

enum LoadedEngine {
    /// A GGUF model loaded through transcribe-cpp (the only engine). Holds the
    /// live `Session`, which keeps its `Model` alive internally, so repeated
    /// dictation reuses the session without reloading.
    TranscribeCpp(Session),
}

/// RAII guard that clears the streaming worker/lease flags on any worker exit -
/// normal return, early return, or a panic in an engine call that unwinds the
/// detached worker thread. Tokens prevent an older worker from clearing a newer
/// worker's state if a start/finalize race ever slips through.
struct StreamWorkerGuard {
    worker_id: u64,
    active_stream_worker: Arc<AtomicU64>,
    active_engine_lease: Arc<AtomicU64>,
    stream_active: Arc<AtomicBool>,
}

impl Drop for StreamWorkerGuard {
    fn drop(&mut self) {
        if self.active_stream_worker.load(Ordering::Acquire) == self.worker_id {
            self.stream_active.store(false, Ordering::Release);
        }
        let _ = self.active_engine_lease.compare_exchange(
            self.worker_id,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        let _ = self.active_stream_worker.compare_exchange(
            self.worker_id,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

#[derive(Clone)]
pub struct TranscriptionManager {
    engine: Arc<Mutex<Option<LoadedEngine>>>,
    model_manager: Arc<ModelManager>,
    app_handle: AppHandle,
    current_model_id: Arc<Mutex<Option<String>>>,
    last_activity: Arc<AtomicU64>,
    shutdown_signal: Arc<AtomicBool>,
    watcher_handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
    is_loading: Arc<Mutex<bool>>,
    loading_condvar: Arc<Condvar>,
    reload_model_on_next_use: Arc<AtomicBool>,
    /// Routes real-time audio frames to the active streaming worker; see
    /// [`StreamRouter`]. Shared with the audio recorder so per-frame feeds skip
    /// Tauri state and the manager lock.
    router: Arc<StreamRouter>,
    /// True only while a transcribe-cpp `Stream` is actually in flight (set by
    /// the worker once `stream()` succeeds). Used for overlay/UI decisions.
    stream_active: Arc<AtomicBool>,
    /// Streaming uses four independent flags: router open = frames should route,
    /// worker active = no second worker may start, engine lease = engine is out
    /// of the mutex, stream active = UI should show a live session.
    ///
    /// Monotonic id source for stream workers; zero means "no worker".
    next_stream_worker_id: Arc<AtomicU64>,
    /// Nonzero while a stream worker exists, even if it has not leased the engine
    /// yet. This prevents a second worker from starting after finalize/cancel
    /// closes the router but before the first worker has fully exited.
    active_stream_worker: Arc<AtomicU64>,
    /// Nonzero while the streaming worker has taken the engine out of `engine`.
    /// `is_model_loaded()` consults this so the model still reports "loaded"
    /// while the worker holds it.
    active_engine_lease: Arc<AtomicU64>,
}

impl TranscriptionManager {
    pub fn new(app_handle: &AppHandle, model_manager: Arc<ModelManager>) -> Result<Self> {
        let manager = Self {
            engine: Arc::new(Mutex::new(None)),
            model_manager,
            app_handle: app_handle.clone(),
            current_model_id: Arc::new(Mutex::new(None)),
            last_activity: Arc::new(AtomicU64::new(Self::now_ms())),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            watcher_handle: Arc::new(Mutex::new(None)),
            is_loading: Arc::new(Mutex::new(false)),
            loading_condvar: Arc::new(Condvar::new()),
            reload_model_on_next_use: Arc::new(AtomicBool::new(false)),
            router: Arc::new(StreamRouter::new()),
            stream_active: Arc::new(AtomicBool::new(false)),
            next_stream_worker_id: Arc::new(AtomicU64::new(1)),
            active_stream_worker: Arc::new(AtomicU64::new(0)),
            active_engine_lease: Arc::new(AtomicU64::new(0)),
        };

        // Start the idle watcher
        {
            let app_handle_cloned = app_handle.clone();
            let manager_cloned = manager.clone();
            let shutdown_signal = manager.shutdown_signal.clone();
            let handle = thread::spawn(move || {
                debug!("Idle watcher thread started");
                while !shutdown_signal.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_secs(10)); // Check every 10 seconds

                    // Check shutdown signal again after sleep
                    if shutdown_signal.load(Ordering::Relaxed) {
                        break;
                    }

                    let timeout = crate::defaults::MODEL_UNLOAD_TIMEOUT;

                    // Skip Immediately, that variant is handled by
                    // maybe_unload_immediately() after each transcription.
                    // Treating it as 0s here would unload the model mid-recording.
                    if timeout == ModelUnloadTimeout::Immediately {
                        continue;
                    }

                    // While recording, keep the idle timer fresh so the
                    // model is never unloaded mid-session.
                    let is_recording = app_handle_cloned
                        .try_state::<Arc<AudioRecordingManager>>()
                        .is_some_and(|a| a.is_recording());
                    if is_recording {
                        manager_cloned.touch_activity();
                        continue;
                    }

                    if let Some(limit_seconds) = timeout.to_seconds() {
                        let last = manager_cloned.last_activity.load(Ordering::Relaxed);
                        let now_ms = TranscriptionManager::now_ms();
                        let idle_ms = now_ms.saturating_sub(last);
                        let limit_ms = limit_seconds * 1000;

                        if idle_ms > limit_ms {
                            // idle -> unload
                            if manager_cloned.is_model_loaded() {
                                let unload_start = std::time::Instant::now();
                                info!(
                                    "Model idle for {}s (limit: {}s), unloading",
                                    idle_ms / 1000,
                                    limit_seconds
                                );
                                match manager_cloned.unload_model() {
                                    Ok(()) => {
                                        let unload_duration = unload_start.elapsed();
                                        info!(
                                            "Model unloaded due to inactivity (took {}ms)",
                                            unload_duration.as_millis()
                                        );
                                    }
                                    Err(e) => {
                                        error!("Failed to unload idle model: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
                debug!("Idle watcher thread shutting down gracefully");
            });
            *manager.watcher_handle.lock().unwrap() = Some(handle);
        }

        Ok(manager)
    }

    /// Lock the engine mutex, recovering from poison if a previous transcription panicked.
    fn lock_engine(&self) -> MutexGuard<'_, Option<LoadedEngine>> {
        self.engine.lock().unwrap_or_else(|poisoned| {
            warn!("Engine mutex was poisoned by a previous panic, recovering");
            poisoned.into_inner()
        })
    }

    pub fn is_model_loaded(&self) -> bool {
        // The engine may be leased out to the streaming worker (taken out of
        // the mutex). It's still loaded, just in use, so report true.
        self.lock_engine().is_some() || self.active_engine_lease.load(Ordering::Acquire) != 0
    }

    pub fn unload_model(&self) -> Result<()> {
        let unload_start = std::time::Instant::now();
        debug!("Starting to unload model");

        {
            let mut engine = self.lock_engine();
            // Dropping the engine frees all resources
            *engine = None;
        }
        {
            let mut current_model = self.current_model_id.lock().unwrap();
            *current_model = None;
        }

        // Emit unloaded event
        let _ = self.app_handle.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "unloaded".to_string(),
                model_id: None,
                model_name: None,
                error: None,
            },
        );

        let unload_duration = unload_start.elapsed();
        debug!(
            "Model unloaded manually (took {}ms)",
            unload_duration.as_millis()
        );
        Ok(())
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    /// Reset the idle timer to now.
    fn touch_activity(&self) {
        self.last_activity.store(Self::now_ms(), Ordering::Relaxed);
    }

    /// Unloads the model immediately if the fixed unload policy says so and
    /// the model is loaded.
    pub fn maybe_unload_immediately(&self, context: &str) {
        if crate::defaults::MODEL_UNLOAD_TIMEOUT == ModelUnloadTimeout::Immediately
            && self.is_model_loaded()
        {
            info!("Immediately unloading model after {}", context);
            if let Err(e) = self.unload_model() {
                warn!("Failed to immediately unload model: {}", e);
            }
        }
    }

    pub fn load_model(&self, model_id: &str) -> Result<()> {
        self.load_model_with_device(model_id, None)
    }

    /// Like [`load_model`](Self::load_model), but lets a caller hard-select the
    /// compute device for this one load by its `transcribe_cpp::devices()`
    /// registry index (the index shown by `--list-devices`). `None` keeps the
    /// persisted accelerator setting (which may be Auto). Only affects
    /// transcribe-cpp (whisper-family) models; the selection is not persisted.
    pub fn load_model_with_device(
        &self,
        model_id: &str,
        device_index: Option<usize>,
    ) -> Result<()> {
        apply_accelerator_settings(&self.app_handle);

        let load_start = std::time::Instant::now();
        debug!("Starting to load model: {}", model_id);

        // Emit loading started event
        let _ = self.app_handle.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "loading_started".to_string(),
                model_id: Some(model_id.to_string()),
                model_name: None,
                error: None,
            },
        );

        let model_info = self
            .model_manager
            .get_model_info(model_id)
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

        if !model_info.is_downloaded {
            let error_msg = "Model not downloaded";
            let _ = self.app_handle.emit(
                "model-state-changed",
                ModelStateEvent {
                    event_type: "loading_failed".to_string(),
                    model_id: Some(model_id.to_string()),
                    model_name: Some(model_info.name.clone()),
                    error: Some(error_msg.to_string()),
                },
            );
            return Err(anyhow::anyhow!(error_msg));
        }

        let model_path = self.model_manager.get_model_path(model_id)?;

        // Drop the current engine BEFORE building the new one so transcribe-cpp
        // frees the previous native context first, avoids holding two models at
        // once (peak memory on large GGUFs). Clear the id too: if the new load
        // fails, status should read "no loaded model", not the dropped engine.
        {
            let mut engine = self.lock_engine();
            *engine = None;
        }
        {
            let mut current_model = self.current_model_id.lock().unwrap();
            *current_model = None;
        }

        // Create appropriate engine based on model type
        let emit_loading_failed = |error_msg: &str| {
            let _ = self.app_handle.emit(
                "model-state-changed",
                ModelStateEvent {
                    event_type: "loading_failed".to_string(),
                    model_id: Some(model_id.to_string()),
                    model_name: Some(model_info.name.clone()),
                    error: Some(error_msg.to_string()),
                },
            );
        };

        let loaded_engine = match model_info.engine_type {
            EngineType::TranscribeCpp => {
                // The whisper backend is chosen at load time (transcribe-cpp has
                // no runtime global). With an explicit `device_index` (the
                // --device-index flag) hard-select that registered device;
                // otherwise re-read the persisted accelerator preference (so an
                // accelerator change marked for reload takes effect here).
                let (backend, gpu_device) = match device_index {
                    Some(index) => resolve_device_index(index).inspect_err(|e| {
                        emit_loading_failed(&e.to_string());
                    })?,
                    None => {
                        let accelerator = crate::defaults::TRANSCRIBE_ACCELERATOR;
                        (
                            select_transcribe_backend(accelerator),
                            resolve_gpu_device(accelerator, crate::defaults::TRANSCRIBE_GPU_DEVICE),
                        )
                    }
                };
                let model_options = ModelOptions {
                    backend,
                    gpu_device,
                };
                let model = Model::load_with(&model_path, &model_options).map_err(|e| {
                    let error_msg = format!("Failed to load whisper model {}: {}", model_id, e);
                    emit_loading_failed(&error_msg);
                    anyhow::anyhow!(error_msg)
                })?;
                // The bound backend may differ from the request (e.g. CPU
                // fallback under Auto); log what actually loaded.
                let bound_backend = model.backend();
                let session = model.session().map_err(|e| {
                    let error_msg = format!(
                        "Failed to create session for whisper model {}: {}",
                        model_id, e
                    );
                    emit_loading_failed(&error_msg);
                    anyhow::anyhow!(error_msg)
                })?;
                // Reconcile the registry's advertised capabilities with the
                // loaded model's real ones (GGUF metadata) so badges/gating
                // reflect runtime truth, not the pre-download probe. The
                // load-completed event below triggers the frontend refresh.
                let caps = session.model().capabilities();
                self.model_manager.set_runtime_capabilities(
                    model_id,
                    caps.supports_streaming,
                    caps.supports_translate,
                    caps.supports_language_detect,
                    caps.languages.clone(),
                );
                info!(
                    "Loaded whisper model '{}' (requested {:?}, gpu_device {}, bound backend '{}', \
                     supports_streaming={}, supports_translate={}, supports_language_detect={})",
                    model_id,
                    backend,
                    gpu_device,
                    bound_backend,
                    caps.supports_streaming,
                    caps.supports_translate,
                    caps.supports_language_detect
                );
                LoadedEngine::TranscribeCpp(session)
            }
        };

        // Update the current engine and model ID
        {
            let mut engine = self.lock_engine();
            *engine = Some(loaded_engine);
        }
        {
            let mut current_model = self.current_model_id.lock().unwrap();
            *current_model = Some(model_id.to_string());
        }

        // Reset idle timer so the watcher doesn't immediately unload a just-loaded model
        self.touch_activity();

        // Emit loading completed event
        let _ = self.app_handle.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "loading_completed".to_string(),
                model_id: Some(model_id.to_string()),
                model_name: Some(model_info.name.clone()),
                error: None,
            },
        );

        let load_duration = load_start.elapsed();
        debug!(
            "Successfully loaded transcription model: {} (took {}ms)",
            model_id,
            load_duration.as_millis()
        );
        Ok(())
    }

    /// Kicks off the model loading in a background thread if it's not already loaded
    pub fn initiate_model_load(&self) {
        let mut is_loading = self.is_loading.lock().unwrap();
        if *is_loading {
            return;
        }

        let reload_pending = self.reload_model_on_next_use.load(Ordering::Acquire);
        if !reload_pending && self.is_model_loaded() {
            return;
        }

        *is_loading = true;
        let self_clone = self.clone();
        thread::spawn(move || {
            if reload_pending {
                self_clone
                    .reload_model_on_next_use
                    .store(false, Ordering::Release);
            }
            if let Err(e) = self_clone.load_model(crate::managers::model::FIXED_STT_MODEL_ID) {
                error!("Failed to load model: {}", e);
            }
            let mut is_loading = self_clone.is_loading.lock().unwrap();
            *is_loading = false;
            self_clone.loading_condvar.notify_all();
        });
    }

    pub fn get_current_model(&self) -> Option<String> {
        let current_model = self.current_model_id.lock().unwrap();
        current_model.clone()
    }

    /// The compute backend the currently-loaded engine is bound to, for
    /// diagnostics (e.g. confirming `--device-index` actually bound a GPU rather
    /// than falling back to CPU/auto). `None` when no model is loaded.
    pub fn current_backend(&self) -> Option<String> {
        match self.lock_engine().as_ref() {
            Some(LoadedEngine::TranscribeCpp(session)) => {
                Some(session.model().backend().to_string())
            }
            None => None,
        }
    }

    /// Whether a live streaming run is currently in flight.
    pub fn is_streaming(&self) -> bool {
        self.stream_active.load(Ordering::Acquire)
    }

    /// Shared handle to the stream router, used by the audio recorder to feed
    /// real-time frames without going through Tauri state on every frame.
    pub fn stream_router(&self) -> Arc<StreamRouter> {
        Arc::clone(&self.router)
    }

    /// Begin a live streaming transcription on the held engine's session.
    /// Audio frames pushed via [`StreamRouter::feed`] (captured directly by the
    /// audio recorder) are decoded incrementally and emitted to the overlay as
    /// [`StreamTextEvent`].
    ///
    /// Non-blocking: spawns a worker that waits for any in-progress model load,
    /// verifies the model supports streaming, then begins the stream. If the
    /// model can't stream, the worker idles until finalize/cancel and reports
    /// `None` so the caller falls back to batch transcription. Frames sent
    /// before the stream begins queue on the channel and are not lost.
    pub fn start_stream(&self, allow_pseudo: bool) {
        if self.router.is_open() || self.active_stream_worker.load(Ordering::Acquire) != 0 {
            warn!("start_stream called while a stream worker is already active");
            return;
        }
        let worker_id = self.next_stream_worker_id.fetch_add(1, Ordering::Relaxed);
        if self
            .active_stream_worker
            .compare_exchange(0, worker_id, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            warn!("start_stream lost a race with another stream worker");
            return;
        }
        let rx = self.router.open();
        self.stream_active.store(false, Ordering::Release);

        let manager = self.clone();
        thread::spawn(move || manager.run_stream_worker(rx, worker_id, allow_pseudo));
    }

    fn run_stream_worker(&self, rx: mpsc::Receiver<StreamCmd>, worker_id: u64, allow_pseudo: bool) {
        let _worker = StreamWorkerGuard {
            worker_id,
            active_stream_worker: Arc::clone(&self.active_stream_worker),
            active_engine_lease: Arc::clone(&self.active_engine_lease),
            stream_active: Arc::clone(&self.stream_active),
        };

        // Wait for any in-progress model load to finish (start_stream races the
        // background load kicked off when recording starts).
        {
            let mut is_loading = self.is_loading.lock().unwrap();
            while *is_loading {
                is_loading = self.loading_condvar.wait(is_loading).unwrap();
            }
        }

        let model_id = self.get_current_model().unwrap_or_default();

        // Take the engine out of the mutex so we own it during streaming,
        // structurally excluding any concurrent batch transcription (which
        // transcribe-cpp's compute_lock would refuse anyway). Returned when the
        // worker exits, or dropped if the model was switched/unloaded mid-stream.
        if self
            .active_engine_lease
            .compare_exchange(0, worker_id, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            warn!("Live preview: another worker already holds the transcription engine");
            self.router.clear();
            drain_until_finalize(rx);
            return;
        }
        let mut engine = match self.lock_engine().take() {
            Some(e) => e,
            None => {
                info!(
                    "Live preview: model '{}' was unloaded before streaming could begin; \
                     falling back to batch transcription",
                    model_id
                );
                let _ = self.active_engine_lease.compare_exchange(
                    worker_id,
                    0,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                self.router.clear();
                drain_until_finalize(rx);
                return;
            }
        };

        // The loaded session (not the ModelManager copy) is the source of
        // truth for run-path capabilities.
        let (supports_streaming, supports_translate, languages) = match &engine {
            LoadedEngine::TranscribeCpp(session) => {
                let model = session.model();
                let caps = model.capabilities();
                info!(
                    "Live preview: model '{}' arch='{}' variant='{}' supports_streaming={} \
                     supports_translate={} languages={:?}",
                    model_id,
                    model.arch(),
                    model.variant(),
                    caps.supports_streaming,
                    caps.supports_translate,
                    caps.languages,
                );
                (
                    caps.supports_streaming,
                    caps.supports_translate,
                    caps.languages,
                )
            }
        };

        if !supports_streaming {
            if allow_pseudo {
                // Live overlay wants text even from batch-only models:
                // periodic re-decode of the accumulated window (display-only;
                // the authoritative final text is still the caller's batch
                // decode after Finalize replies None).
                self.run_pseudo_stream(engine, rx, &model_id);
                return;
            }
            self.return_engine(engine, &model_id);
            self.router.clear();
            drain_until_finalize(rx);
            return;
        }

        // Build run options mirroring the offline transcribe-cpp path: task +
        // language gated against what the model actually advertises.
        let effective_language =
            effective_language_for_model(self.model_manager.as_ref(), &model_id);
        let run_plan =
            transcribe_cpp_run_plan(false, &effective_language, &languages, supports_translate);
        let run_options = RunOptions {
            task: run_plan.task,
            language: run_plan.language,
            target_language: run_plan.target_language,
            ..Default::default()
        };

        // Run the stream on the held session. The Stream borrows the session
        // (and thus the engine) for its lifetime, so the feed/finalize loop
        // lives in a labeled block, when it exits, the borrow is released and
        // the engine can be moved into return_engine().
        let mut finalize_reply: Option<mpsc::Sender<Option<String>>> = None;
        let mut finalize_result: Option<Option<String>> = None;
        let stream_started = 'stream: {
            let LoadedEngine::TranscribeCpp(session) = &mut engine;

            // Read the backend string before beginning the stream, the
            // `Stream` borrows `session` mutably for its lifetime, so we can't
            // call `session.model()` once it exists.
            let backend = session.model().backend();

            // StreamOptions::default() uses CommitPolicy::Auto and lets the
            // family pick its own streaming strategy (no family-specific ext).
            let mut stream = match session.stream(&run_options, &StreamOptions::default()) {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to begin stream: {}", e);
                    break 'stream false;
                }
            };

            self.stream_active.store(true, Ordering::Release);
            self.touch_activity();
            info!(
                "Live streaming transcription started (model '{}', backend '{}')",
                model_id, backend
            );

            let mut perf = StreamPerf::new();
            while let Ok(cmd) = rx.recv() {
                match cmd {
                    StreamCmd::Feed(pcm, _) => {
                        self.touch_activity();
                        perf.record_feed(pcm.len());
                        let feed_start = Instant::now();
                        match stream.feed(&pcm) {
                            Ok(update) => {
                                perf.record_compute(feed_start.elapsed());
                                perf.record_update(
                                    update.revision,
                                    update.input_received_ms,
                                    update.audio_committed_ms,
                                    update.buffered_ms,
                                );
                                if update.committed_changed || update.tentative_changed {
                                    let text = stream.text();
                                    perf.record_emit();
                                    self.emit_stream_text(&text.committed, &text.tentative);
                                }
                                perf.maybe_log();
                            }
                            Err(e) => {
                                perf.record_compute(feed_start.elapsed());
                                warn!("stream feed failed: {}", e);
                            }
                        }
                    }
                    StreamCmd::Finalize(reply) => {
                        let finalize_start = Instant::now();
                        let result = match stream.finalize() {
                            // After finalize the committed prefix holds the full
                            // text; display() = committed + tentative is the safe read.
                            Ok(update) => {
                                perf.record_compute(finalize_start.elapsed());
                                perf.record_update(
                                    update.revision,
                                    update.input_received_ms,
                                    update.audio_committed_ms,
                                    update.buffered_ms,
                                );
                                Some(stream.text().display())
                            }
                            Err(e) => {
                                perf.record_compute(finalize_start.elapsed());
                                error!(
                                    "stream finalize failed: {}; falling back to batch transcription",
                                    e
                                );
                                None
                            }
                        };
                        let chars = match &result {
                            Some(text) => text.len(),
                            _ => 0,
                        };
                        perf.log_finalized(chars);
                        finalize_reply = Some(reply);
                        finalize_result = Some(result);
                        break;
                    }
                    StreamCmd::Cancel => {
                        stream.reset();
                        break;
                    }
                }
            }

            true
        };
        // `stream` + the `&mut engine` borrow are released here.

        if !stream_started {
            // Stream never began (model doesn't support streaming or begin
            // failed); drain so the finalize handshake still completes and the
            // caller falls back to batch transcription. Return the engine first
            // so the fallback can immediately use it.
            self.return_engine(engine, &model_id);
            drain_until_finalize(rx);
            return;
        }

        self.return_engine(engine, &model_id);
        if let (Some(reply), Some(result)) = (finalize_reply, finalize_result) {
            let _ = reply.send(result);
        }
        // `_worker` drops here, clearing this worker's active/lease flags after
        // the engine has been returned to the pool.
    }

    /// Pseudo-streaming worker path for batch-only models: accumulate PCM,
    /// re-decode the window on a decode-time-aware cadence, diff against the
    /// previous decode (word-boundary LCP) into committed/tentative, and emit
    /// through the same StreamTextEvent the native path uses. Finalize always
    /// replies None after returning the engine, so the caller runs the exact
    /// batch decode it runs today, partials are display-only.
    fn run_pseudo_stream(
        &self,
        mut engine: LoadedEngine,
        rx: mpsc::Receiver<StreamCmd>,
        model_id: &str,
    ) {
        let settings = get_settings(&self.app_handle);
        let effective_language =
            effective_language_for_model(self.model_manager.as_ref(), model_id);
        info!(
            "Live preview (pseudo): model '{}' lang '{}'",
            model_id, effective_language
        );

        self.stream_active.store(true, Ordering::Release);
        self.emit_stream_text("", "");

        let mut st = PseudoStreamState::new();
        let mut probed = false;
        // Capability seed: models that advertise their timestamp ceiling skip
        // the probe ladder's failed ticks (canary would burn two).
        if let LoadedEngine::TranscribeCpp(session) = &engine {
            match session.model().capabilities().max_timestamp_kind {
                TimestampKind::None => {
                    st.ts_mode = PseudoTsMode::Degraded;
                    st.healthy = false;
                    probed = true;
                }
                TimestampKind::Segment => st.ts_mode = PseudoTsMode::Segment,
                _ => {} // Word/Token/Auto: start at Word; the ladder still guards
            }
        }
        let mut ticks: u32 = 0;
        let mut tick_ms_max: u128 = 0;
        // Wall-clock feed tracking: silence boundaries (Degraded flush) and
        // the settle decode (render the last words once feeds stop).
        let mut last_feed_at: Option<Instant> = None;
        let mut settled = false;

        'outer: loop {
            // recv_timeout keeps the worker breathing when feeds stop, so the
            // settle decode can render the trailing words of a pause.
            let first = match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(cmd) => Some(cmd),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break 'outer,
            };
            let mut pending_finalize: Option<mpsc::Sender<Option<String>>> = None;
            let mut cancelled = false;
            for cmd in first.into_iter().chain(rx.try_iter()) {
                match cmd {
                    StreamCmd::Feed(pcm, at) => {
                        self.touch_activity();
                        let gap =
                            last_feed_at.is_some_and(|t| at.duration_since(t) >= FEED_GAP_SILENCE);
                        last_feed_at = Some(at);
                        settled = false;
                        // Degraded window bounding: freeze the standing window
                        // BEFORE pushing post-gap speech, so the flush decode
                        // ends exactly at the silence boundary.
                        if st.degraded_flush_due(gap) {
                            let flush = catch_unwind(AssertUnwindSafe(|| {
                                pseudo_decode(
                                    &mut engine,
                                    &st.window,
                                    &settings,
                                    &effective_language,
                                    TimestampKind::None,
                                )
                            }));
                            match flush {
                                Ok(Ok(t)) => {
                                    if let Some((committed, tentative)) = st.flush_degraded(&t.text)
                                    {
                                        self.emit_stream_text(&committed, &tentative);
                                    }
                                }
                                Ok(Err(e)) => {
                                    // Keeping the window would reinstate the
                                    // generation-cap death spiral; drop the
                                    // span from the live view only (the batch
                                    // final still covers it).
                                    warn!("degraded flush decode failed: {e}; clearing window");
                                    let _ = st.flush_degraded("");
                                }
                                Err(payload) => {
                                    self.note_engine_panic(payload);
                                    drop(engine);
                                    drain_until_finalize(rx);
                                    return;
                                }
                            }
                        }
                        st.push(&pcm);
                    }
                    StreamCmd::Finalize(reply) => {
                        pending_finalize = Some(reply);
                        break;
                    }
                    StreamCmd::Cancel => {
                        cancelled = true;
                        break;
                    }
                }
            }
            if let Some(reply) = pending_finalize {
                // Healthy path: one bounded tail decode; what the user watched
                // IS the final text. Otherwise None and the caller batch-decodes
                // all audio, exactly the historical behavior.
                let result = if st.healthy && st.total_samples > 0 && !st.window.is_empty() {
                    let final_ts = if st.ts_mode == PseudoTsMode::Segment {
                        TimestampKind::Segment
                    } else {
                        TimestampKind::Word
                    };
                    let final_t = catch_unwind(AssertUnwindSafe(|| {
                        pseudo_decode(
                            &mut engine,
                            &st.window,
                            &settings,
                            &effective_language,
                            final_ts,
                        )
                    }));
                    match final_t {
                        Ok(Ok(t)) => {
                            // Append the final tail through the same dedup gate
                            // the live commits use (midpoint filter, then
                            // boundary-duplicate suppression inside append).
                            let tail: Vec<LiveWord> = st
                                .words_from_transcript(&t)
                                .into_iter()
                                .filter(|w| (w.t0_ms + w.t1_ms) / 2 > st.committed_end_ms)
                                .collect();
                            for (i, w) in tail.iter().enumerate() {
                                st.append_committed(w, i == 0);
                            }
                            info!(
                                "Live preview (pseudo) finalized: {} ticks, max tick {}ms, {} chars",
                                ticks,
                                tick_ms_max,
                                st.committed.len()
                            );
                            Some(st.committed.clone())
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                self.return_engine(engine, model_id);
                let _ = reply.send(result);
                return;
            }
            if cancelled {
                self.return_engine(engine, model_id);
                return;
            }

            let now = Instant::now();
            // Settle decode: feeds stopped but undecoded audio remains (the
            // "last sentence" fix). One per gap; new audio re-arms it.
            let settle_due = !settled
                && last_feed_at.is_some_and(|t| now.duration_since(t) >= SETTLE_GAP)
                && st.should_settle(now);
            if st.should_decode(now) || settle_due {
                if settle_due {
                    settled = true;
                }
                let ts = match st.ts_mode {
                    PseudoTsMode::Word => TimestampKind::Word,
                    PseudoTsMode::Segment => TimestampKind::Segment,
                    PseudoTsMode::Degraded => TimestampKind::None,
                };
                let decode = catch_unwind(AssertUnwindSafe(|| {
                    pseudo_decode(&mut engine, &st.window, &settings, &effective_language, ts)
                }));
                let took = now.elapsed();
                ticks += 1;
                tick_ms_max = tick_ms_max.max(took.as_millis());
                match decode {
                    Ok(Ok(t)) => {
                        if !probed {
                            probed = true;
                            if st.ts_mode == PseudoTsMode::Word {
                                if t.words.is_empty() && !t.segments.is_empty() {
                                    st.ts_mode = PseudoTsMode::Segment;
                                } else if t.words.is_empty()
                                    && t.segments.is_empty()
                                    && !t.text.trim().is_empty()
                                {
                                    st.ts_mode = PseudoTsMode::Degraded;
                                    st.healthy = false;
                                }
                            }
                        }
                        if st.ts_mode == PseudoTsMode::Degraded {
                            // Text-agreement commits: two consecutive decodes
                            // agreeing on a word prefix commit it (minus a
                            // guard), so the overlay gets stable committed
                            // text and the LiveCleaner can work on canary.
                            if let Some((committed, tentative)) =
                                st.note_decode_degraded(&t.text, now, took)
                            {
                                self.emit_stream_text(&committed, &tentative);
                            }
                        } else {
                            let words = st.words_from_transcript(&t);
                            if let Some((committed, tentative)) = st.note_decode(words, now, took) {
                                self.emit_stream_text(&committed, &tentative);
                            }
                            st.trim_after_commit();
                            st.enforce_tail_cap();
                        }
                    }
                    Ok(Err(e)) => {
                        st.last_decode_start = Some(now);
                        st.last_decode_dur = took;
                        if !probed {
                            // Capability ladder: some engines reject finer
                            // timestamp kinds with a hard error rather than
                            // empty rows (canary: "unsupported timestamp
                            // granularity"). Step down and retry next tick
                            // instead of abandoning live text.
                            match st.ts_mode {
                                PseudoTsMode::Word => {
                                    info!(
                                        "live decode: word timestamps rejected, \
                                         trying segment ({e})"
                                    );
                                    st.ts_mode = PseudoTsMode::Segment;
                                }
                                PseudoTsMode::Segment => {
                                    info!(
                                        "live decode: segment timestamps rejected, \
                                         degrading to text-only live view ({e})"
                                    );
                                    st.ts_mode = PseudoTsMode::Degraded;
                                    st.healthy = false;
                                    probed = true;
                                }
                                PseudoTsMode::Degraded => {
                                    warn!("pseudo-stream decode failed: {e}");
                                    st.healthy = false;
                                    probed = true;
                                }
                            }
                        } else {
                            warn!("pseudo-stream decode failed: {}", e);
                            st.healthy = false;
                        }
                    }
                    Err(panic_payload) => {
                        self.note_engine_panic(panic_payload);
                        drop(engine);
                        drain_until_finalize(rx);
                        return;
                    }
                }
            }
        }
        self.return_engine(engine, model_id);
    }

    /// Shared cleanup when the STT engine panics mid-stream: log, clear the
    /// current-model slot, and tell the frontend the engine is gone.
    fn note_engine_panic(&self, panic_payload: Box<dyn std::any::Any + Send>) {
        let msg = if let Some(m) = panic_payload.downcast_ref::<&str>() {
            m.to_string()
        } else if let Some(m) = panic_payload.downcast_ref::<String>() {
            m.clone()
        } else {
            "unknown panic".to_string()
        };
        error!("pseudo-stream engine panicked: {msg}; dropping engine");
        {
            let mut current_model = self
                .current_model_id
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *current_model = None;
        }
        let _ = self.app_handle.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "unloaded".to_string(),
                model_id: None,
                model_name: None,
                error: Some(format!("Engine panicked: {msg}")),
            },
        );
    }

    /// Return the leased engine to the mutex, unless the model was switched or
    /// unloaded during transcription (in which case the stale engine is dropped).
    fn return_engine(&self, engine: LoadedEngine, expected_model_id: &str) {
        let still_current =
            self.current_model_id.lock().unwrap().as_deref() == Some(expected_model_id);
        if still_current {
            *self.lock_engine() = Some(engine);
        } else {
            info!(
                "Model changed/unloaded during transcription; dropping stale engine (was '{}')",
                expected_model_id
            );
            // `engine` drops here, freeing its resources.
        }
    }

    /// Flush the active stream and return its final, post-filtered text.
    ///
    /// `Ok(None)` means no usable stream was active and the caller may fall back
    /// to batch transcription. `Err` means finalize itself failed or timed out.
    /// A timeout may still leave the worker holding the engine, so callers
    /// should surface it instead of immediately starting a batch fallback.
    pub fn finalize_stream(&self) -> Result<Option<String>> {
        let Some(tx) = self.router.take() else {
            return Ok(None);
        };
        let (reply_tx, reply_rx) = mpsc::channel();
        if tx.send(StreamCmd::Finalize(reply_tx)).is_err() {
            return Ok(None);
        }
        let raw = match reply_rx.recv_timeout(STREAM_FINALIZE_REPLY_TIMEOUT) {
            Ok(Some(text)) => text,
            Ok(None) => return Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.stream_active.store(false, Ordering::Release);
                return Err(anyhow::anyhow!(
                    "Timed out waiting {:?} for live transcription to finalize",
                    STREAM_FINALIZE_REPLY_TIMEOUT
                ));
            }
        };

        let settings = get_settings(&self.app_handle);
        // Streaming models do not receive a decode prompt, so custom words
        // always go through the shared fuzzy post-correction path. The stage
        // config is the dictation's start-time snapshot (final variant), so
        // this agrees byte-for-byte with what the LiveCleaner ticks saw.
        let cfg = crate::pipeline::current_final_cfg(&self.app_handle, &settings, false);
        let filtered = crate::pipeline::run_deterministic(&raw, &cfg);

        self.maybe_unload_immediately("streaming transcription");
        Ok(Some(filtered))
    }

    /// Abandon any active stream without producing text (e.g. on cancel).
    pub fn cancel_stream(&self) {
        if let Some(tx) = self.router.take() {
            let _ = tx.send(StreamCmd::Cancel);
        }
        self.stream_active.store(false, Ordering::Release);
    }

    /// Emit a working-phase event to the streaming overlay (spinner + label).
    pub fn emit_stream_working(&self, kind: StreamWorkKind) {
        let _ = StreamPhaseEvent {
            phase: StreamPhase::Working,
            kind: Some(kind),
        }
        .emit(&self.app_handle);
    }

    pub(crate) fn emit_stream_text(&self, committed: &str, tentative: &str) {
        let _ = StreamTextEvent {
            committed: committed.to_string(),
            tentative: tentative.to_string(),
        }
        .emit(&self.app_handle);
        // F3: hand the committed text to an active textbox injector DIRECTLY,
        // not through the JSON event (a listener could reorder; injection
        // order must match emission order exactly). Cheap when no injector is
        // active: one managed-state lookup and a mutex peek.
        if committed.is_empty() {
            return;
        }
        if let Some(slot) = self
            .app_handle
            .try_state::<crate::stream_inject::InjectorSlot>()
        {
            let injector = slot.0.lock().unwrap().clone();
            if let Some(injector) = injector.filter(|i| i.wants_committed()) {
                injector.on_committed(committed);
            }
        }
    }

    pub fn transcribe(&self, audio: Vec<f32>) -> Result<String> {
        #[cfg(debug_assertions)]
        if std::env::var("VAPORLY_FORCE_TRANSCRIPTION_FAILURE").is_ok()
            || std::env::var("HANDY_FORCE_TRANSCRIPTION_FAILURE").is_ok()
        {
            return Err(anyhow::anyhow!(
                "Simulated transcription failure (VAPORLY_FORCE_TRANSCRIPTION_FAILURE)"
            ));
        }

        // Update last activity timestamp
        self.touch_activity();

        let st = std::time::Instant::now();
        let audio_len = audio.len();

        debug!("Audio vector length: {}", audio_len);

        if audio.is_empty() {
            debug!("Empty audio vector");
            self.maybe_unload_immediately("empty audio");
            return Ok(String::new());
        }

        // Check if model is loaded, if not try to load it
        {
            // If the model is loading, wait for it to complete.
            let mut is_loading = self.is_loading.lock().unwrap();
            while *is_loading {
                is_loading = self.loading_condvar.wait(is_loading).unwrap();
            }

            let engine_guard = self.lock_engine();
            if engine_guard.is_none() {
                return Err(anyhow::anyhow!("Model is not loaded for transcription."));
            }
        }

        // Get current settings for configuration
        let settings = get_settings(&self.app_handle);

        // Validate the fixed English intent against the model that is actually
        // loaded (which can differ from the fixed default when a caller loaded
        // a specific model, e.g. the headless path).
        let active_model = self
            .get_current_model()
            .unwrap_or_else(|| crate::managers::model::FIXED_STT_MODEL_ID.to_string());
        // Resolve the language intent ("en", fixed in v2) into the language
        // this model will actually use. The coercion is capability-aware (a
        // must-pick model never receives "auto").
        let validated_language =
            effective_language_for_model(self.model_manager.as_ref(), &active_model);

        // Whether the loaded transcribe-cpp model accepts a decode prompt
        // (whisper family). Gates the whisper-only run extension below, and
        // whether fuzzy custom-word correction still runs afterwards.
        let mut model_takes_initial_prompt = false;

        // Perform transcription with the appropriate engine.
        // We use catch_unwind to prevent engine panics from poisoning the mutex,
        // which would make the app hang indefinitely on subsequent operations.
        let result = {
            let mut engine_guard = self.lock_engine();

            // Take the engine out so we own it during transcription.
            // If the engine panics, we simply don't put it back (effectively unloading it)
            // instead of poisoning the mutex.
            let mut engine = match engine_guard.take() {
                Some(e) => e,
                None => {
                    return Err(anyhow::anyhow!(
                        "Model failed to load after auto-load attempt. Please check your model settings."
                    ));
                }
            };

            // Release the lock before transcribing, no mutex held during the engine call
            drop(engine_guard);

            // Probe live transcribe-cpp capabilities once (cheap GGUF-metadata
            // reads); the loaded session is the source of truth, not the
            // ModelManager copy. The whisper run extension is kind-tagged, so
            // non-whisper archs (parakeet, voxtral, …) reject it with
            // INVALID_ARG; attach it, and translate, only where supported.
            let mut model_supports_translate = false;
            let mut model_languages: Vec<String> = Vec::new();
            if let LoadedEngine::TranscribeCpp(session) = &engine {
                let model = session.model();
                let caps = model.capabilities();
                model_takes_initial_prompt = model.supports(Feature::InitialPrompt);
                model_supports_translate = caps.supports_translate;
                model_languages = caps.languages;
                debug!(
                    "transcribe-cpp model '{}' on '{}': initial_prompt={}, translate={}, languages={:?}",
                    active_model,
                    model.backend(),
                    model_takes_initial_prompt,
                    model_supports_translate,
                    model_languages
                );
            }

            let transcribe_result = catch_unwind(AssertUnwindSafe(|| -> Result<String> {
                match &mut engine {
                    LoadedEngine::TranscribeCpp(session) => {
                        // Custom words become the initial prompt ONLY for models
                        // that accept one (whisper family). Attaching the
                        // whisper run extension to a non-whisper arch is rejected
                        // with INVALID_ARG, so skip it there and let the fuzzy
                        // post-correction handle custom words instead.
                        let family =
                            if settings.custom_words.is_empty() || !model_takes_initial_prompt {
                                None
                            } else {
                                Some(RunExtension::Whisper(WhisperRunOptions {
                                    initial_prompt: Some(settings.custom_words.join(", ")),
                                    ..Default::default()
                                }))
                            };

                        let run_plan = transcribe_cpp_run_plan(
                            false,
                            &validated_language,
                            &model_languages,
                            model_supports_translate,
                        );

                        let run_options = RunOptions {
                            task: run_plan.task,
                            language: run_plan.language,
                            target_language: run_plan.target_language,
                            // Whisper-family long-form (>30s) decode degenerates into a
                            // repetition loop when an initial prompt is set AND timestamps
                            // are off, a shared whisper.cpp behavior (verified: whisper.cpp
                            // collapses in the same prompt + no-timestamps cell). Vaporly runs
                            // whisper.cpp with timestamps on, so request segment timestamps
                            // here too for parity, which keeps multi-window decode stable.
                            // Only whisper advertises InitialPrompt; other arches keep None.
                            timestamps: if model_takes_initial_prompt {
                                TimestampKind::Segment
                            } else {
                                TimestampKind::None
                            },
                            family,
                            ..Default::default()
                        };

                        debug!(
                            "transcribe-cpp run: task={:?}, language={:?}, initial_prompt={}",
                            run_options.task,
                            run_options.language,
                            run_options.family.is_some()
                        );

                        session
                            .run(&audio, &run_options)
                            .map(|t| t.text)
                            .map_err(|e| {
                                anyhow::anyhow!("transcribe-cpp transcription failed: {}", e)
                            })
                    }
                }
            }));

            match transcribe_result {
                Ok(inner_result) => {
                    // Success or normal error: return the engine unless a model
                    // switch/unload invalidated it while it was in use.
                    self.return_engine(engine, &active_model);
                    inner_result?
                }
                Err(panic_payload) => {
                    // Engine panicked, do NOT put it back (it's in an unknown state).
                    // The engine is dropped here, effectively unloading it.
                    let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    error!(
                        "Transcription engine panicked: {}. Model has been unloaded.",
                        panic_msg
                    );

                    // Clear the model ID so it will be reloaded on next attempt
                    {
                        let mut current_model = self
                            .current_model_id
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        *current_model = None;
                    }

                    let _ = self.app_handle.emit(
                        "model-state-changed",
                        ModelStateEvent {
                            event_type: "unloaded".to_string(),
                            model_id: None,
                            model_name: None,
                            error: Some(format!("Engine panicked: {}", panic_msg)),
                        },
                    );

                    return Err(anyhow::anyhow!(
                        "Transcription engine panicked: {}. The model has been unloaded and will reload on next attempt.",
                        panic_msg
                    ));
                }
            }
        };

        // Apply fuzzy word correction if custom words are configured, UNLESS the
        // words were already handed to the model as an initial prompt (whisper
        // family). Non-whisper transcribe-cpp models can't take a prompt, so they
        // still get fuzzy correction here, same as the ONNX engines. During a
        // dictation this reads the start-time snapshot; headless/re-transcribe
        // callers get a fresh context-less config.
        let filtered_result = {
            let cfg = crate::pipeline::current_final_cfg(
                &self.app_handle,
                &settings,
                model_takes_initial_prompt,
            );
            crate::pipeline::run_deterministic(&result, &cfg)
        };

        let et = std::time::Instant::now();
        // Real-time factor. Input PCM is 16 kHz mono, so audio length in seconds
        // is samples / 16000. `speedup` is audio_secs / elapsed_secs, e.g. 4.00x
        // means transcribed 4x faster than real time
        let elapsed_secs = (et - st).as_secs_f64();
        let audio_secs = audio_len as f64 / 16_000.0;
        let speedup = real_time_factor(audio_secs, elapsed_secs);
        info!(
            "Transcription completed in {:.2}s for {:.2}s of audio ({:.2}x real-time)",
            elapsed_secs, audio_secs, speedup
        );

        let final_result = filtered_result;

        if final_result.is_empty() {
            info!("Transcription result is empty");
        } else {
            // Never write the spoken content to the persistent log by default:
            // log only its length at debug. The full text is available at trace
            // (off by default) for local debugging.
            debug!("Transcription result: {} chars", final_result.len());
            log::trace!("Transcription full text: {final_result}");
        }

        self.maybe_unload_immediately("transcription");

        Ok(final_result)
    }
}

struct StreamPerf {
    feed_count: u64,
    emit_count: u64,
    streamed_samples: u64,
    stream_compute_elapsed: Duration,
    last_log: Instant,
    latest_revision: i32,
    latest_input_received_ms: i64,
    latest_audio_committed_ms: i64,
    latest_buffered_ms: i64,
}

impl StreamPerf {
    fn new() -> Self {
        Self {
            feed_count: 0,
            emit_count: 0,
            streamed_samples: 0,
            stream_compute_elapsed: Duration::ZERO,
            last_log: Instant::now(),
            latest_revision: 0,
            latest_input_received_ms: 0,
            latest_audio_committed_ms: 0,
            latest_buffered_ms: 0,
        }
    }

    fn record_feed(&mut self, samples: usize) {
        self.feed_count += 1;
        self.streamed_samples += samples as u64;
    }

    fn record_compute(&mut self, elapsed: Duration) {
        self.stream_compute_elapsed += elapsed;
    }

    fn record_update(
        &mut self,
        revision: i32,
        input_received_ms: i64,
        audio_committed_ms: i64,
        buffered_ms: i64,
    ) {
        self.latest_revision = revision;
        self.latest_input_received_ms = input_received_ms;
        self.latest_audio_committed_ms = audio_committed_ms;
        self.latest_buffered_ms = buffered_ms;
    }

    fn record_emit(&mut self) {
        self.emit_count += 1;
    }

    fn maybe_log(&mut self) {
        if self.last_log.elapsed() < STREAM_PERF_LOG_INTERVAL {
            return;
        }

        let audio_secs = self.audio_secs();
        let compute_secs = self.compute_secs();
        debug!(
            "Live preview perf: {:.2}s streamed audio, {:.2}s model compute ({:.2}x real-time), \
             input_received={:.2}s, committed_audio={:.2}s, buffered={}ms, revision={}, \
             {} frames fed, {} updates emitted",
            audio_secs,
            compute_secs,
            real_time_factor(audio_secs, compute_secs),
            self.latest_input_received_ms as f64 / 1000.0,
            self.latest_audio_committed_ms as f64 / 1000.0,
            self.latest_buffered_ms,
            self.latest_revision,
            self.feed_count,
            self.emit_count,
        );
        self.last_log = Instant::now();
    }

    fn log_finalized(&self, chars: usize) {
        let audio_secs = self.audio_secs();
        let compute_secs = self.compute_secs();
        info!(
            "Live preview finalized in {:.2}s model compute for {:.2}s streamed audio ({:.2}x real-time): \
             input_received={:.2}s, committed_audio={:.2}s, buffered={}ms, revision={}, \
             {} frames fed, {} updates emitted, {} chars",
            compute_secs,
            audio_secs,
            real_time_factor(audio_secs, compute_secs),
            self.latest_input_received_ms as f64 / 1000.0,
            self.latest_audio_committed_ms as f64 / 1000.0,
            self.latest_buffered_ms,
            self.latest_revision,
            self.feed_count,
            self.emit_count,
            chars
        );
    }

    fn audio_secs(&self) -> f64 {
        self.streamed_samples as f64 / 16_000.0
    }

    fn compute_secs(&self) -> f64 {
        self.stream_compute_elapsed.as_secs_f64()
    }
}

fn real_time_factor(audio_secs: f64, compute_secs: f64) -> f64 {
    if compute_secs > 0.0 {
        audio_secs / compute_secs
    } else {
        0.0
    }
}

fn normalize_cjk_language(language: &str) -> &str {
    match language {
        "zh-Hans" | "zh-Hant" => "zh",
        other => other,
    }
}

/// Resolve the fixed "en" language intent into the language a specific model
/// can use (v2 is English-only; the coercion keeps working for models that
/// spell it as a locale like en-US).
fn effective_language_for_model(model_manager: &ModelManager, model_id: &str) -> String {
    match model_manager.get_model_info(model_id) {
        Some(info) => crate::managers::model::effective_language(
            "en",
            &info.supported_languages,
            info.supports_language_detection,
        ),
        None => "en".to_string(),
    }
}

struct TranscribeCppRunPlan {
    task: Task,
    language: Option<String>,
    target_language: Option<String>,
}

/// Build the transcribe-cpp language/task options shared by batch and live
/// streaming paths.
fn transcribe_cpp_run_plan(
    translate_to_english: bool,
    effective_language: &str,
    model_languages: &[String],
    model_supports_translate: bool,
) -> TranscribeCppRunPlan {
    let requested_language = match effective_language {
        "auto" => None,
        other => Some(normalize_cjk_language(other).to_string()),
    };
    // Only pass a language the loaded model actually advertises (per
    // capabilities().languages); otherwise auto-detect rather than failing with
    // UNSUPPORTED_LANGUAGE. Language-agnostic models report an empty list, so
    // they always stay on auto.
    let language = requested_language.filter(|lang| model_languages.iter().any(|l| l == lang));
    let (task, target_language) = cpp_translation_task(
        translate_to_english,
        model_supports_translate,
        language.as_deref(),
    );

    TranscribeCppRunPlan {
        task,
        language,
        target_language,
    }
}

/// Thin shim over [`crate::pipeline::run_deterministic`] for callers that
/// hold settings but no per-dictation snapshot (tests, future one-off
/// consumers). Builds a context-less [`crate::pipeline::StageConfig`] on the
/// spot; dictation paths use the snapshot in `pipeline::DictationContextSlot`
/// instead so live and final agree byte-for-byte.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn post_process_transcription_text(
    raw: String,
    settings: &AppSettings,
    custom_words_already_prompted: bool,
    live: bool,
) -> String {
    let cfg = crate::pipeline::StageConfig::from_settings(
        settings,
        live,
        custom_words_already_prompted,
        None,
    );
    crate::pipeline::run_deterministic(&raw, &cfg)
}

/// Decide a transcribe-cpp run's task + translation target from settings.
///
/// "Translate to English" only fires where the model advertises translation.
/// transcribe-cpp requires an explicit `target_language`: a null target
/// defaults to the *source*, so a non-English source silently becomes e.g.
/// es→es and the engine rejects the unadvertised pair.
/// An English source is skipped entirely, en→en is not a real translation, and
/// it's reachable by default since auto-detect-less models coerce intent to "en".
///
/// Returns `(task, target_language)` ready to drop into `RunOptions`.
fn cpp_translation_task(
    translate_to_english: bool,
    model_supports_translate: bool,
    source_language: Option<&str>,
) -> (Task, Option<String>) {
    let translate_to_en =
        translate_to_english && model_supports_translate && source_language != Some("en");
    if translate_to_en {
        (Task::Translate, Some("en".to_string()))
    } else {
        (Task::Transcribe, None)
    }
}

/// Drain a stream command channel, ignoring fed audio, until the caller
/// finalizes or cancels. Used when streaming can't actually run (model not
/// loaded / not streaming-capable) so the finalize handshake still completes
/// and the caller falls back to batch transcription.
fn drain_until_finalize(rx: mpsc::Receiver<StreamCmd>) {
    while let Ok(cmd) = rx.recv() {
        match cmd {
            StreamCmd::Feed(..) => {}
            StreamCmd::Finalize(reply) => {
                let _ = reply.send(None);
                break;
            }
            StreamCmd::Cancel => break,
        }
    }
}

/// Initialize the transcribe-cpp native backend once at startup: route native +
/// ggml diagnostics into the `log` facade and register compute backend modules.
/// In a static build (macOS Metal) `init_backends_default` is a harmless no-op;
/// in a `dynamic-backends` build it loads the per-ISA CPU / GPU modules. Must run
/// before the first model load.
pub fn init_transcribe_backend() {
    transcribe_cpp::init_logging();
    match transcribe_cpp::init_backends_default() {
        Ok(()) => {
            let devices = transcribe_cpp::devices();
            info!(
                "transcribe-cpp initialized with {} compute device(s): [{}]",
                devices.len(),
                devices
                    .iter()
                    .map(|d| format!("{} ({})", d.name, d.kind))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Err(e) => warn!("Failed to initialize transcribe-cpp backends: {}", e),
    }
}

/// Human-readable list of the transcribe-cpp compute devices registered at
/// startup, for the `--list-devices` flag. The reported `index` is the
/// value to pass to `--device-index`. Backends must be initialized first
/// (see [`init_transcribe_backend`]).
pub fn describe_compute_devices() -> Vec<String> {
    transcribe_cpp::devices()
        .into_iter()
        .map(|d| {
            let idx = d
                .index
                .map(|i| i.to_string())
                .unwrap_or_else(|| "-".to_string());
            let name = if d.description.is_empty() {
                d.name
            } else {
                d.description
            };
            let vram_mb = d.memory_total / (1024 * 1024);
            format!(
                "index={} kind={} name={} vram={}MB",
                idx, d.kind, name, vram_mb
            )
        })
        .collect()
}

/// Resolve a `--list-devices` registry index to the (backend, gpu_device) pair
/// for a transcribe-cpp model load (the `--device-index` flag). The
/// backend is set explicitly from the device's kind, so there's no "index 0 =
/// auto" ambiguity. Errors if the index isn't a registered, loadable device.
fn resolve_device_index(index: usize) -> Result<(Backend, i32)> {
    let device = transcribe_cpp::devices()
        .into_iter()
        .find(|d| d.index == Some(index))
        .ok_or_else(|| {
            anyhow::anyhow!("No compute device with index {index} (see --list-devices)")
        })?;
    let backend = match device.kind.as_str() {
        "cpu" => Backend::Cpu,
        "metal" => Backend::Metal,
        "cuda" => Backend::Cuda,
        "vulkan" => Backend::Vulkan,
        other => {
            return Err(anyhow::anyhow!(
                "Device index {index} has kind '{other}', which cannot host a model"
            ))
        }
    };
    // gpu_device is a registry index used only by GPU backends; CPU ignores it.
    let gpu_device = if matches!(backend, Backend::Cpu) {
        0
    } else {
        index as i32
    };
    Ok((backend, gpu_device))
}

/// Map Vaporly's whisper accelerator setting to a transcribe-cpp [`Backend`].
///
/// `Auto` lets the library pick the best device (with CPU fallback). `Cpu` forces
/// strict CPU. `Gpu` requests the platform GPU backend, but only if a device for
/// it is actually registered, otherwise it falls back to `Auto` so the load
/// never fails outright on a machine without that GPU backend.
fn select_transcribe_backend(setting: TranscribeAcceleratorSetting) -> Backend {
    match setting {
        TranscribeAcceleratorSetting::Cpu => Backend::Cpu,
        TranscribeAcceleratorSetting::Auto => {
            // VMs expose a paravirtual Metal device that transcribe-cpp's Auto
            // happily binds, and it decodes at ~1.4x realtime, which makes the
            // live transcript trail seconds behind speech (measured on the
            // reference VM; its CPU runs the same models 5-15x faster). Same
            // lesson the bundled LLM engine already encodes: in a VM, CPU wins.
            if crate::managers::hardware::profile().is_vm {
                info!("transcribe.cpp Auto: VM detected, binding CPU (paravirtual GPU is slower than CPU)");
                Backend::Cpu
            } else {
                Backend::Auto
            }
        }
        TranscribeAcceleratorSetting::Gpu => {
            #[cfg(target_os = "macos")]
            let candidates = [Backend::Metal];
            #[cfg(not(target_os = "macos"))]
            let candidates = [Backend::Cuda, Backend::Vulkan];

            match candidates
                .into_iter()
                .find(|&b| transcribe_cpp::backend_available(b))
            {
                Some(b) => b,
                None => {
                    warn!("No GPU backend available for transcribe.cpp; falling back to Auto");
                    Backend::Auto
                }
            }
        }
    }
}

/// Resolve the user's stored GPU device choice into a [`ModelOptions::gpu_device`]
/// registry index for the next model load.
///
/// Settings store a registry index into [`transcribe_cpp::devices`] (`-1` is the
/// UI's auto/CPU sentinel); transcribe-cpp treats `0` as "auto / first match" and
/// rejects an out-of-range or non-GPU index. So an explicit selection is honored
/// only when the user chose the GPU accelerator and the stored index still
/// resolves to a registered GPU device, otherwise fall back to `0` so a stale
/// selection can never fail the load.
fn resolve_gpu_device(setting: TranscribeAcceleratorSetting, gpu_device: i32) -> i32 {
    if setting != TranscribeAcceleratorSetting::Gpu || gpu_device <= 0 {
        return 0;
    }
    let still_valid = transcribe_cpp::devices()
        .iter()
        .any(|d| d.index == Some(gpu_device as usize) && d.kind != "cpu" && d.kind != "accel");
    if still_valid {
        gpu_device
    } else {
        warn!(
            "Stored transcribe GPU device index {} is no longer available; using auto",
            gpu_device
        );
        0
    }
}

/// Log the accelerator preference that the next model load will apply.
///
/// The transcribe.cpp backend is not set here: it is chosen at model-load time
/// from [`select_transcribe_backend`], so changing the accelerator only needs a
/// model reload (see `reload_model_on_next_use`).
pub fn apply_accelerator_settings(_app: &tauri::AppHandle) {
    info!(
        "transcribe.cpp accelerator preference: {:?} (applied on next model load)",
        crate::defaults::TRANSCRIBE_ACCELERATOR
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn languages(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|code| (*code).to_string()).collect()
    }

    #[test]
    fn transcribe_cpp_run_plan_maps_chinese_variants() {
        let plan = transcribe_cpp_run_plan(false, "zh-Hant", &languages(&["zh"]), true);

        assert!(matches!(plan.task, Task::Transcribe));
        assert_eq!(plan.language.as_deref(), Some("zh"));
        assert_eq!(plan.target_language, None);
    }

    #[test]
    fn transcribe_cpp_run_plan_skips_english_translation() {
        let plan = transcribe_cpp_run_plan(true, "en", &languages(&["en", "es"]), true);

        assert!(matches!(plan.task, Task::Transcribe));
        assert_eq!(plan.language.as_deref(), Some("en"));
        assert_eq!(plan.target_language, None);
    }

    #[test]
    fn transcribe_cpp_run_plan_translates_supported_non_english() {
        let plan = transcribe_cpp_run_plan(true, "es", &languages(&["en", "es"]), true);

        assert!(matches!(plan.task, Task::Translate));
        assert_eq!(plan.language.as_deref(), Some("es"));
        assert_eq!(plan.target_language.as_deref(), Some("en"));
    }

    #[test]
    fn transcribe_cpp_run_plan_requires_model_translation_support() {
        let plan = transcribe_cpp_run_plan(true, "es", &languages(&["en", "es"]), false);

        assert!(matches!(plan.task, Task::Transcribe));
        assert_eq!(plan.language.as_deref(), Some("es"));
        assert_eq!(plan.target_language, None);
    }
}

impl Drop for TranscriptionManager {
    fn drop(&mut self) {
        // Skip shutdown unless this is the very last clone. TranscriptionManager
        // is cloned by initiate_model_load() and the watcher thread, those
        // clones dropping must not kill the watcher. The watcher thread holds
        // its own clone, so engine's strong_count is always >= 2 while the
        // watcher is alive. When it reaches 1, only this instance remains
        // and we can safely shut down.
        if Arc::strong_count(&self.engine) > 1 {
            return;
        }

        // Signal the watcher thread to shutdown
        self.shutdown_signal.store(true, Ordering::Relaxed);

        // Wait for the thread to finish gracefully
        if let Some(handle) = self.watcher_handle.lock().unwrap().take() {
            if let Err(e) = handle.join() {
                warn!("Failed to join idle watcher thread: {:?}", e);
            } else {
                debug!("Idle watcher thread joined successfully");
            }
        }
    }
}

/// Pseudo-streaming tuning: tail-window decode with LocalAgreement-2 commits
/// (whisper_streaming's policy), sized so per-tick cost is CONSTANT regardless
/// of dictation length on a CPU decoding at rtf 0.15-0.3.
const PSEUDO_MIN_INTERVAL: Duration = Duration::from_millis(500);
/// Faster floor once the engine has PROVEN it keeps up (smoothed rtf under
/// PSEUDO_FAST_RTF): live text updates 2.5x per second instead of 2x.
const PSEUDO_MIN_INTERVAL_FAST: Duration = Duration::from_millis(400);
const PSEUDO_FAST_RTF: f32 = 0.12;
const PSEUDO_INTERVAL_FACTOR: f32 = 1.15;
const PSEUDO_FIRST_DECODE_SECS: f32 = 0.6;
const PSEUDO_MIN_NEW_AUDIO_SECS: f32 = 0.3;
/// Target decode wall time per tick; the tail window is sized to hit this.
const PSEUDO_TICK_BUDGET_SECS: f32 = 0.55;
const PSEUDO_TAIL_MIN_SECS: f32 = 2.2;
const PSEUDO_TAIL_MAX_SECS: f32 = 6.0;
/// Committed audio kept in the window as acoustic left context after a trim.
const PSEUDO_LEFT_CTX_SECS: f32 = 0.8;
/// Words ending within this of the audio edge never commit (still unstable).
const PSEUDO_COMMIT_GUARD_SECS: f32 = 0.4;
/// Word start-time match tolerance across two decodes of shifted windows.
const PSEUDO_AGREE_TOL_MS: i64 = 160;
/// Two consecutive over-budget ticks at the minimum window pause live decode.
const PSEUDO_PAUSE_TICK_SECS: f32 = 2.5;
/// Cooldown before a paused live decode retries; healthy stays sticky-false.
const PSEUDO_PAUSE_COOLDOWN: Duration = Duration::from_secs(3);
/// A tick this slow means CPU contention; word-boundary timestamps jitter
/// enough to risk dropped words at commit seams, so the live text stops
/// being paste-worthy (display continues; the batch final pastes).
const PSEUDO_JITTER_UNHEALTHY_SECS: f32 = 1.5;
/// Degraded commits: trailing words held back from a text-agreement commit.
const DEGRADED_GUARD_WORDS: usize = 4;
/// Wall-clock gap between feeds marking a silence boundary. Under
/// VadPolicy::Offline the recorder emits ONLY VAD-passed speech frames, so
/// feed gaps ARE natural silences.
const FEED_GAP_SILENCE: Duration = Duration::from_millis(300);
/// Degraded window budget: flush at the next silence boundary past MAX...
const DEGRADED_WINDOW_MAX_SECS: f32 = 12.0;
/// ...and force a mid-speech flush past HARD (generation-cap guard).
const DEGRADED_WINDOW_HARD_SECS: f32 = 16.0;
/// After this long without a new frame, one settle decode renders whatever
/// audio arrived after the last tick (the "last sentence" fix, all modes).
const SETTLE_GAP: Duration = Duration::from_millis(400);
const PSEUDO_SAMPLE_RATE: f32 = 16_000.0;

/// Timestamp capability of the live decode path, probed on the first tick.
#[derive(Clone, Copy, PartialEq)]
enum PseudoTsMode {
    Word,
    Segment,
    /// No usable timestamps: tentative-only live view, batch final.
    Degraded,
}

/// One decoded word on the ABSOLUTE fed-audio timeline.
#[derive(Clone)]
struct LiveWord {
    /// Agreement key: lowercased, surrounding punctuation stripped.
    norm: String,
    /// Display form; the newest decode's spelling wins.
    display: String,
    t0_ms: i64,
    t1_ms: i64,
}

fn norm_word(w: &str) -> String {
    w.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

/// LocalAgreement-2: longest prefix where both decodes agree on the word and
/// (within tolerance) on WHEN it was said.
fn agree_prefix(prev: &[LiveWord], new: &[LiveWord]) -> usize {
    let mut n = 0;
    for (a, b) in prev.iter().zip(new.iter()) {
        if a.norm == b.norm && (a.t0_ms - b.t0_ms).abs() <= PSEUDO_AGREE_TOL_MS {
            n += 1;
        } else {
            break;
        }
    }
    n
}

/// Worker-local state for the tail-window pseudo-stream.
struct PseudoStreamState {
    // audio (absolute sample bookkeeping: window_start + window.len() == total)
    window: Vec<f32>,
    window_start_sample: u64,
    total_samples: u64,
    // text
    committed: String,
    committed_end_ms: i64,
    /// Norm of the last committed word, for boundary re-decode dedup.
    committed_last_norm: String,
    prev_words: Vec<LiveWord>,
    recent_commit_bounds: std::collections::VecDeque<i64>,
    last_emit_committed: String,
    last_emit_tentative: String,
    // cadence / health
    last_decode_start: Option<Instant>,
    last_decode_dur: Duration,
    rtf_ema: f32,
    /// Recoverable pause: decoding suspends until this deadline, then retries
    /// (a slow machine gets a slow heartbeat, never a dead overlay).
    pause_until: Option<Instant>,
    samples_at_last_decode: u64,
    ts_mode: PseudoTsMode,
    /// True while the live text can be trusted as the FINAL text.
    healthy: bool,
    slow_ticks: u32,
    /// Degraded mode: normalized words of the previous full-window decode
    /// (text-agreement source; separate from prev_words, whose fabricated
    /// times would poison the midpoint filter).
    prev_text_words: Vec<String>,
    /// Degraded mode: words of the current window's decode already committed
    /// (monotonic ratchet; reset when the window flushes).
    degraded_committed_words: usize,
}

impl PseudoStreamState {
    fn new() -> Self {
        Self {
            window: Vec::new(),
            window_start_sample: 0,
            total_samples: 0,
            committed: String::new(),
            committed_end_ms: 0,
            committed_last_norm: String::new(),
            prev_words: Vec::new(),
            recent_commit_bounds: std::collections::VecDeque::with_capacity(16),
            last_emit_committed: String::new(),
            last_emit_tentative: String::new(),
            last_decode_start: None,
            last_decode_dur: Duration::ZERO,
            rtf_ema: 0.0,
            pause_until: None,
            samples_at_last_decode: 0,
            ts_mode: PseudoTsMode::Word,
            healthy: true,
            slow_ticks: 0,
            prev_text_words: Vec::new(),
            degraded_committed_words: 0,
        }
    }

    fn push(&mut self, pcm: &[f32]) {
        self.window.extend_from_slice(pcm);
        self.total_samples += pcm.len() as u64;
    }

    fn window_secs(&self) -> f32 {
        self.window.len() as f32 / PSEUDO_SAMPLE_RATE
    }

    fn total_fed_ms(&self) -> i64 {
        (self.total_samples as f64 * 1000.0 / PSEUDO_SAMPLE_RATE as f64) as i64
    }

    fn window_start_ms(&self) -> i64 {
        (self.window_start_sample as f64 * 1000.0 / PSEUDO_SAMPLE_RATE as f64) as i64
    }

    fn should_decode(&self, now: Instant) -> bool {
        if let Some(until) = self.pause_until {
            if now < until {
                return false;
            }
        }
        if (self.total_samples as f32) < PSEUDO_FIRST_DECODE_SECS * PSEUDO_SAMPLE_RATE {
            return false;
        }
        let new_audio =
            (self.total_samples - self.samples_at_last_decode) as f32 / PSEUDO_SAMPLE_RATE;
        if new_audio < PSEUDO_MIN_NEW_AUDIO_SECS {
            return false;
        }
        match self.last_decode_start {
            None => true,
            Some(start) => {
                // The floor drops to 400ms once the smoothed decode rtf shows
                // real headroom; the adaptive backoff still rules when slow.
                let floor = if self.rtf_ema > 0.0 && self.rtf_ema < PSEUDO_FAST_RTF {
                    PSEUDO_MIN_INTERVAL_FAST
                } else {
                    PSEUDO_MIN_INTERVAL
                };
                let min_interval = floor.max(self.last_decode_dur.mul_f32(PSEUDO_INTERVAL_FACTOR));
                now.duration_since(start) >= min_interval
            }
        }
    }

    /// Words (absolute-time) from a decode of the current window.
    fn words_from_transcript(&self, t: &transcribe_cpp::Transcript) -> Vec<LiveWord> {
        let base = self.window_start_ms();
        let mk = |t0: i64, t1: i64, text: &str| LiveWord {
            norm: norm_word(text),
            display: text.trim().to_string(),
            t0_ms: base + t0,
            t1_ms: base + t1,
        };
        if !t.words.is_empty() {
            return t
                .words
                .iter()
                .filter(|w| !w.text.trim().is_empty())
                .map(|w| mk(w.t0_ms, w.t1_ms, &w.text))
                .collect();
        }
        // Segment fallback: each segment behaves as one "word" for agreement.
        t.segments
            .iter()
            .filter(|sg| !sg.text.trim().is_empty())
            .map(|sg| mk(sg.t0_ms, sg.t1_ms, &sg.text))
            .collect()
    }

    /// Append one word to the committed text, deduping boundary re-decodes.
    ///
    /// `at_boundary` marks the FIRST word a batch appends. Word timings jitter
    /// 50-200ms between decodes of shifted windows, so for short words no
    /// timestamp test can separate "the last committed word decoded again"
    /// from "the speaker said it twice across the boundary". A same-norm word
    /// at the boundary that starts at or before the committed edge is dropped
    /// unconditionally: the re-decode case is overwhelmingly more common, and
    /// the rare straddling stutter it eats is a disfluency downstream cleanup
    /// removes anyway. Words with clear air after the boundary always append.
    /// Skipped duplicates still advance the boundary to their fresher end.
    fn append_committed(&mut self, w: &LiveWord, at_boundary: bool) {
        let boundary_dup = at_boundary
            && w.norm == self.committed_last_norm
            && !w.norm.is_empty()
            && w.t0_ms <= self.committed_end_ms + 80;
        if !boundary_dup {
            if !self.committed.is_empty() && !self.committed.ends_with(char::is_whitespace) {
                self.committed.push(' ');
            }
            self.committed.push_str(&w.display);
            self.committed_last_norm = w.norm.clone();
        }
        self.committed_end_ms = self.committed_end_ms.max(w.t1_ms);
        if self.recent_commit_bounds.len() == 16 {
            self.recent_commit_bounds.pop_front();
        }
        self.recent_commit_bounds.push_back(w.t1_ms);
    }

    /// Cadence and pause bookkeeping shared by every decode flavor.
    fn note_decode_common(&mut self, started: Instant, took: Duration) {
        self.last_decode_start = Some(started);
        self.last_decode_dur = took;
        self.samples_at_last_decode = self.total_samples;
        let secs = self.window_secs().max(0.1);
        let rtf = took.as_secs_f32() / secs;
        self.rtf_ema = if self.rtf_ema == 0.0 {
            rtf
        } else {
            self.rtf_ema * 0.7 + rtf * 0.3
        };
        // A decode ran, so any prior cooldown is over; a re-trip needs two
        // fresh slow ticks.
        self.pause_until = None;
        if took.as_secs_f32() > PSEUDO_JITTER_UNHEALTHY_SECS {
            self.healthy = false;
        }
        if took.as_secs_f32() > PSEUDO_PAUSE_TICK_SECS
            && self.window_secs() <= PSEUDO_TAIL_MIN_SECS + 0.5
        {
            self.slow_ticks += 1;
            if self.slow_ticks >= 2 {
                self.pause_until = Some(started + took + PSEUDO_PAUSE_COOLDOWN);
                self.healthy = false; // sticky: the batch final stays authoritative
                self.slow_ticks = 0;
            }
        } else {
            self.slow_ticks = 0;
        }
    }

    /// Settle decode: when feeds stop (the user paused or finished a
    /// sentence), the new-audio gate would leave the last words undecoded
    /// forever. This relaxes ONLY that gate; pause and backoff still apply.
    fn should_settle(&self, now: Instant) -> bool {
        if let Some(until) = self.pause_until {
            if now < until {
                return false;
            }
        }
        if self.total_samples == self.samples_at_last_decode {
            return false;
        }
        if (self.total_samples as f32) < PSEUDO_MIN_NEW_AUDIO_SECS * PSEUDO_SAMPLE_RATE {
            return false;
        }
        match self.last_decode_start {
            None => true,
            Some(start) => {
                let min_interval =
                    PSEUDO_MIN_INTERVAL.max(self.last_decode_dur.mul_f32(PSEUDO_INTERVAL_FACTOR));
                now.duration_since(start) >= min_interval
            }
        }
    }

    /// Append plain display text to the committed string (Degraded mode:
    /// timestamp bookkeeping stays untouched, committed_end_ms remains 0).
    fn append_committed_text(&mut self, display: &str) {
        let display = display.trim();
        if display.is_empty() {
            return;
        }
        if !self.committed.is_empty() && !self.committed.ends_with(char::is_whitespace) {
            self.committed.push(' ');
        }
        self.committed.push_str(display);
        self.committed_last_norm = norm_word(display);
    }

    /// Degraded (no timestamps): LocalAgreement-2 on WORD SEQUENCES of two
    /// consecutive full-window decodes. Words agreed by both decodes commit
    /// minus a trailing guard; commits only ever grow (a later decode that
    /// flips an early word simply stops new commits that tick).
    fn note_decode_degraded(
        &mut self,
        text: &str,
        started: Instant,
        took: Duration,
    ) -> Option<(String, String)> {
        self.note_decode_common(started, took);
        let display: Vec<&str> = text.split_whitespace().collect();
        let norms: Vec<String> = display.iter().map(|w| norm_word(w)).collect();
        let lcp = self
            .prev_text_words
            .iter()
            .zip(norms.iter())
            .take_while(|(a, b)| a == b)
            .count();
        let commit_upto = lcp.saturating_sub(DEGRADED_GUARD_WORDS);
        if commit_upto > self.degraded_committed_words {
            let fresh = display[self.degraded_committed_words..commit_upto].join(" ");
            self.append_committed_text(&fresh);
            self.degraded_committed_words = commit_upto;
        }
        self.prev_text_words = norms;
        let tentative = display
            .get(self.degraded_committed_words..)
            .unwrap_or(&[])
            .join(" ");
        if self.committed == self.last_emit_committed && tentative == self.last_emit_tentative {
            return None;
        }
        self.last_emit_committed = self.committed.clone();
        self.last_emit_tentative = tentative.clone();
        Some((self.committed.clone(), tentative))
    }

    /// Whether the Degraded window should be flushed to a frozen prefix now.
    fn degraded_flush_due(&self, at_silence_gap: bool) -> bool {
        self.ts_mode == PseudoTsMode::Degraded
            && !self.window.is_empty()
            && ((at_silence_gap && self.window_secs() > DEGRADED_WINDOW_MAX_SECS)
                || self.window_secs() > DEGRADED_WINDOW_HARD_SECS)
    }

    /// Freeze the standing window's text into committed (single-decode
    /// trust; healthy is already false in Degraded) and clear the window so
    /// per-tick decode cost stays bounded and the decoder's generation cap
    /// is never hit again.
    fn flush_degraded(&mut self, text: &str) -> Option<(String, String)> {
        let display: Vec<&str> = text.split_whitespace().collect();
        let fresh = display
            .get(self.degraded_committed_words..)
            .unwrap_or(&[])
            .join(" ");
        self.append_committed_text(&fresh);
        self.window.clear();
        self.window_start_sample = self.total_samples;
        self.samples_at_last_decode = self.total_samples;
        self.prev_text_words.clear();
        self.degraded_committed_words = 0;
        if self.committed == self.last_emit_committed && self.last_emit_tentative.is_empty() {
            return None;
        }
        self.last_emit_committed = self.committed.clone();
        self.last_emit_tentative = String::new();
        Some((self.committed.clone(), String::new()))
    }

    /// Record a decode; commit agreed words; return (committed, tentative) when
    /// the emitted pair changed.
    fn note_decode(
        &mut self,
        words: Vec<LiveWord>,
        started: Instant,
        took: Duration,
    ) -> Option<(String, String)> {
        self.note_decode_common(started, took);

        // Drop left-context re-decodes of already-committed audio. Gate on the
        // word MIDPOINT: boundaries jitter 50-200ms between decodes of shifted
        // windows, so a t1-based cutoff re-admits (doubles) the last committed
        // word whenever its end lands a few ms past the recorded boundary.
        let fresh: Vec<LiveWord> = words
            .into_iter()
            .filter(|w| (w.t0_ms + w.t1_ms) / 2 > self.committed_end_ms)
            .collect();

        let agreed = agree_prefix(&self.prev_words, &fresh);
        // Commit only words safely behind the audio edge.
        let guard_ms = self.total_fed_ms() - (PSEUDO_COMMIT_GUARD_SECS * 1000.0) as i64;
        let commit_n = fresh[..agreed]
            .iter()
            .take_while(|w| w.t1_ms <= guard_ms)
            .count();
        for (i, w) in fresh[..commit_n].to_vec().iter().enumerate() {
            self.append_committed(w, i == 0);
        }
        let tentative = fresh[commit_n..]
            .iter()
            .map(|w| w.display.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        // Keep ONLY the uncommitted suffix: next tick's decode words are
        // filtered against the advanced committed_end_ms, so a stored
        // committed word would misalign agreement forever.
        let mut fresh = fresh;
        self.prev_words = fresh.split_off(commit_n);
        if self.committed == self.last_emit_committed && tentative == self.last_emit_tentative {
            return None;
        }
        self.last_emit_committed = self.committed.clone();
        self.last_emit_tentative = tentative.clone();
        Some((self.committed.clone(), tentative))
    }

    /// Trim committed audio out of the window, keeping acoustic left context.
    /// Per-tick decode cost becomes O(tail) forever.
    fn trim_after_commit(&mut self) {
        let target_ms = self.committed_end_ms - (PSEUDO_LEFT_CTX_SECS * 1000.0) as i64;
        if target_ms <= self.window_start_ms() {
            return;
        }
        let tail_target = (PSEUDO_TICK_BUDGET_SECS / self.rtf_ema.max(0.05))
            .clamp(PSEUDO_TAIL_MIN_SECS, PSEUDO_TAIL_MAX_SECS);
        let cut_ms = target_ms.min(self.total_fed_ms() - (tail_target * 1000.0) as i64);
        if cut_ms <= self.window_start_ms() {
            return;
        }
        let cut_sample = ((cut_ms as f64 / 1000.0) * PSEUDO_SAMPLE_RATE as f64) as u64;
        let drop = (cut_sample.saturating_sub(self.window_start_sample)) as usize;
        if drop == 0 || drop >= self.window.len() {
            return;
        }
        self.window.drain(..drop);
        self.window_start_sample += drop as u64;
    }

    /// Hard tail cap: on agreement stalls, force-commit older words (single-
    /// decode trust) and mark the stream unhealthy so batch stays authoritative.
    fn enforce_tail_cap(&mut self) {
        if self.window_secs() <= PSEUDO_TAIL_MAX_SECS {
            return;
        }
        let cap_start_ms = self.total_fed_ms() - (PSEUDO_TAIL_MAX_SECS * 1000.0) as i64;
        let n_stale = self
            .prev_words
            .iter()
            .take_while(|w| w.t1_ms <= cap_start_ms)
            .count();
        if n_stale > 0 {
            let stale: Vec<LiveWord> = self.prev_words.drain(..n_stale).collect();
            for (i, w) in stale.iter().enumerate() {
                self.append_committed(w, i == 0);
            }
            self.healthy = false;
        }
        let cut_sample = ((cap_start_ms.max(0) as f64 / 1000.0) * PSEUDO_SAMPLE_RATE as f64) as u64;
        let drop = (cut_sample.saturating_sub(self.window_start_sample)) as usize;
        if drop > 0 && drop < self.window.len() {
            self.window.drain(..drop);
            self.window_start_sample += drop as u64;
        }
    }
}

/// One live decode of the tail window. transcribe-cpp models return timed
/// output (words/segments) used for LocalAgreement commits.
fn pseudo_decode(
    engine: &mut LoadedEngine,
    audio: &[f32],
    _settings: &AppSettings,
    validated_language: &str,
    ts: TimestampKind,
) -> Result<transcribe_cpp::Transcript> {
    match engine {
        LoadedEngine::TranscribeCpp(session) => {
            let (supports_translate, languages) = {
                let model = session.model();
                let caps = model.capabilities();
                (caps.supports_translate, caps.languages)
            };
            let run_plan =
                transcribe_cpp_run_plan(false, validated_language, &languages, supports_translate);
            let run_options = RunOptions {
                task: run_plan.task,
                language: run_plan.language,
                target_language: run_plan.target_language,
                // Timestamps ON for live agreement; no prompt (the whisper
                // repetition-loop cell needs prompt + no-timestamps).
                timestamps: ts,
                family: None,
                ..Default::default()
            };
            session
                .run(audio, &run_options)
                .map_err(|e| anyhow::anyhow!("pseudo decode failed: {e}"))
        }
    }
}

#[cfg(test)]
mod phrase_wiring_tests {
    use super::*;
    use crate::settings::{FeatureLevel, StageEngine};

    fn settings_with_phrase() -> AppSettings {
        let mut s = crate::settings::get_default_settings();
        s.custom_phrases = vec![crate::settings::CustomPhrase {
            say: "btw".to_string(),
            write: "by the way".to_string(),
        }];
        s
    }

    #[test]
    fn phrases_expand_even_when_words_were_prompted() {
        let mut s = settings_with_phrase();
        s.custom_words = vec!["Kubernetes".to_string()];
        let out = post_process_transcription_text("btw hello".to_string(), &s, true, false);
        assert!(out.to_lowercase().contains("by the way"), "got: {out}");
    }

    #[test]
    fn phrases_expand_with_filler_stage_off() {
        // Filler Off: phrases still expand, and the always-on F1 shaping
        // (caps + terminal punctuation) applies regardless of the filler
        // stage (v1's early-return "verbatim mode" is gone by design).
        let mut s = settings_with_phrase();
        s.filler_level = FeatureLevel::Off;
        let out = post_process_transcription_text("btw hello".to_string(), &s, false, false);
        assert_eq!(out, "By the way hello.");
    }

    #[test]
    fn model_filler_engine_skips_deterministic_filtering() {
        // Filler stage on Model: the deterministic pass must not eat fillers
        // (the LLM pass owns them). Shaping still applies (always-on).
        let mut s = crate::settings::get_default_settings();
        s.filler_engine = StageEngine::Model;
        let out = post_process_transcription_text("um hello there".to_string(), &s, false, false);
        assert!(out.to_lowercase().starts_with("um"), "got: {out}");
    }

    #[test]
    fn custom_words_level_off_disables_correction() {
        let mut s = crate::settings::get_default_settings();
        s.custom_words = vec!["Kubernetes".to_string()];
        s.custom_words_level = FeatureLevel::Off;
        let out =
            post_process_transcription_text("coober netties hello".to_string(), &s, false, false);
        assert!(!out.contains("Kubernetes"), "got: {out}");
    }

    #[test]
    fn words_correct_before_phrases_match() {
        let mut s = crate::settings::get_default_settings();
        // The custom word fixes the spelling ("charge B" -> "ChargeBee", the
        // documented n-gram case); the phrase then matches the corrected text.
        s.custom_words = vec!["ChargeBee".to_string()];
        s.custom_phrases = vec![crate::settings::CustomPhrase {
            say: "ChargeBee".to_string(),
            write: "ChargeBee (billing)".to_string(),
        }];
        let out = post_process_transcription_text("charge B hello".to_string(), &s, false, false);
        assert!(out.contains("(billing)"), "got: {out}");
    }
}

#[cfg(test)]
mod pseudo_stream_tests {
    use super::*;

    fn lw(t0_ms: i64, t1_ms: i64, text: &str) -> LiveWord {
        LiveWord {
            norm: norm_word(text),
            display: text.to_string(),
            t0_ms,
            t1_ms,
        }
    }

    fn secs(s: f32) -> Vec<f32> {
        vec![0.0; (s * PSEUDO_SAMPLE_RATE) as usize]
    }

    #[test]
    fn norm_word_strips_punct_and_case() {
        assert_eq!(norm_word(" Hello, "), "hello");
        assert_eq!(norm_word("\"World!\""), "world");
        assert_eq!(norm_word("9:30"), "9:30"); // inner punctuation kept
    }

    #[test]
    fn agreement_needs_two_decodes() {
        let new = vec![lw(0, 400, "hello"), lw(500, 900, "world")];
        assert_eq!(agree_prefix(&[], &new), 0);
    }

    #[test]
    fn agreement_stops_at_word_change() {
        let prev = vec![
            lw(0, 400, "meet"),
            lw(500, 900, "at"),
            lw(1000, 1400, "eight"),
        ];
        let new = vec![
            lw(0, 400, "meet"),
            lw(500, 900, "at"),
            lw(1000, 1400, "nine"),
        ];
        assert_eq!(agree_prefix(&prev, &new), 2);
    }

    #[test]
    fn agreement_tolerates_small_time_drift_only() {
        let prev = vec![lw(0, 400, "hello"), lw(500, 900, "world")];
        let drift = vec![lw(100, 500, "hello"), lw(600, 1000, "world")];
        assert_eq!(agree_prefix(&prev, &drift), 2);
        let shifted = vec![lw(300, 700, "hello"), lw(800, 1200, "world")];
        assert_eq!(agree_prefix(&prev, &shifted), 0);
    }

    #[test]
    fn agreement_ignores_punct_and_case() {
        let prev = vec![lw(0, 400, "Hello,")];
        let new = vec![lw(0, 400, "hello")];
        assert_eq!(agree_prefix(&prev, &new), 1);
    }

    #[test]
    fn cadence_first_decode_and_min_interval() {
        let mut st = PseudoStreamState::new();
        let t0 = Instant::now();
        assert!(!st.should_decode(t0), "no audio yet");
        st.push(&secs(0.5));
        assert!(!st.should_decode(t0), "under the first-decode threshold");
        st.push(&secs(0.2)); // 0.7s total
        assert!(st.should_decode(t0), "first decode once 0.6s is fed");
        let _ = st.note_decode(vec![lw(0, 300, "one")], t0, Duration::from_millis(200));
        st.push(&secs(0.5));
        assert!(
            !st.should_decode(t0 + Duration::from_millis(499)),
            "500ms floor"
        );
        assert!(st.should_decode(t0 + Duration::from_millis(510)));
    }

    #[test]
    fn cadence_backs_off_when_decode_is_slow() {
        let mut st = PseudoStreamState::new();
        let t0 = Instant::now();
        st.push(&secs(2.0));
        let _ = st.note_decode(vec![], t0, Duration::from_millis(1000));
        st.push(&secs(1.0));
        // Next tick waits 1.15x the last decode duration: 1150ms.
        assert!(!st.should_decode(t0 + Duration::from_millis(1100)));
        assert!(st.should_decode(t0 + Duration::from_millis(1200)));
    }

    #[test]
    fn cadence_needs_new_audio() {
        let mut st = PseudoStreamState::new();
        let t0 = Instant::now();
        st.push(&secs(1.0));
        let _ = st.note_decode(vec![], t0, Duration::from_millis(100));
        assert!(
            !st.should_decode(t0 + Duration::from_secs(5)),
            "no new audio: never re-decode"
        );
        st.push(&secs(0.2)); // under the 0.3s minimum
        assert!(!st.should_decode(t0 + Duration::from_secs(5)));
        st.push(&secs(0.2));
        assert!(st.should_decode(t0 + Duration::from_secs(5)));
    }

    #[test]
    fn local_agreement_commits_and_continues() {
        let mut st = PseudoStreamState::new();
        let t0 = Instant::now();
        st.push(&secs(3.0)); // commit guard sits at 2600ms
        let words = || {
            vec![
                lw(0, 500, "Hello"),
                lw(600, 1000, "world"),
                lw(1100, 1500, "how"),
                lw(2600, 2900, "are"),
            ]
        };
        // First decode: no agreement yet, everything tentative.
        let (c, t) = st
            .note_decode(words(), t0, Duration::from_millis(200))
            .unwrap();
        assert_eq!(c, "");
        assert_eq!(t, "Hello world how are");
        // Second decode agrees: words safely behind the guard commit.
        let (c, t) = st
            .note_decode(
                words(),
                t0 + Duration::from_millis(600),
                Duration::from_millis(200),
            )
            .unwrap();
        assert_eq!(c, "Hello world how");
        assert_eq!(t, "are");
        // Third decode: committed words no longer appear in the decode input
        // (filtered by committed_end_ms). Agreement must keep working;
        // regression guard for prev_words holding only the uncommitted suffix.
        st.push(&secs(1.0)); // total 4s, guard 3600ms
        let tail = vec![lw(2600, 2900, "are"), lw(3000, 3400, "you")];
        let (c, t) = st
            .note_decode(
                tail,
                t0 + Duration::from_millis(1200),
                Duration::from_millis(200),
            )
            .unwrap();
        assert_eq!(c, "Hello world how are");
        assert_eq!(t, "you");
        assert!(st.healthy, "normal flow keeps live text final-worthy");
    }

    #[test]
    fn disagreeing_tail_never_commits() {
        let mut st = PseudoStreamState::new();
        let t0 = Instant::now();
        st.push(&secs(3.0));
        let first = vec![
            lw(0, 500, "meet"),
            lw(600, 1000, "at"),
            lw(1100, 1500, "eight"),
        ];
        let second = vec![
            lw(0, 500, "meet"),
            lw(600, 1000, "at"),
            lw(1100, 1500, "nine"),
        ];
        let _ = st.note_decode(first, t0, Duration::from_millis(200));
        let (c, t) = st
            .note_decode(
                second,
                t0 + Duration::from_millis(600),
                Duration::from_millis(200),
            )
            .unwrap();
        assert_eq!(c, "meet at");
        assert_eq!(t, "nine", "the newest decode's tail wins the display");
    }

    #[test]
    fn identical_timed_decode_is_deduped() {
        let mut st = PseudoStreamState::new();
        let t0 = Instant::now();
        st.push(&secs(1.0));
        let w = || vec![lw(0, 700, "hello")];
        assert!(st
            .note_decode(w(), t0, Duration::from_millis(100))
            .is_some());
        assert!(st
            .note_decode(
                w(),
                t0 + Duration::from_millis(600),
                Duration::from_millis(100)
            )
            .is_none());
    }

    #[test]
    fn boundary_redecodes_do_not_double_words() {
        let mut st = PseudoStreamState::new();
        let t0 = Instant::now();
        st.push(&secs(3.0));
        // Two agreeing decodes commit "running the".
        let w1 = vec![lw(0, 500, "running"), lw(600, 800, "the")];
        let _ = st.note_decode(w1.clone(), t0, Duration::from_millis(100));
        let _ = st.note_decode(
            w1,
            t0 + Duration::from_millis(600),
            Duration::from_millis(100),
        );
        assert_eq!(st.committed, "running the");
        // The next decodes re-supply "the" with jittered timings whose midpoint
        // leaks past the committed boundary. It must not double.
        st.push(&secs(1.0));
        let w2 = vec![lw(750, 1000, "the"), lw(1050, 1500, "numbers")];
        let _ = st.note_decode(
            w2.clone(),
            t0 + Duration::from_millis(1200),
            Duration::from_millis(100),
        );
        let _ = st.note_decode(
            w2,
            t0 + Duration::from_millis(1800),
            Duration::from_millis(100),
        );
        assert_eq!(st.committed, "running the numbers");
    }

    #[test]
    fn distant_same_word_is_not_deduped() {
        let mut st = PseudoStreamState::new();
        st.committed = "yes".into();
        st.committed_last_norm = "yes".into();
        st.committed_end_ms = 1000;
        st.push(&secs(3.0));
        // Same word again but with clear air after the boundary: a real repeat.
        st.append_committed(&lw(1600, 1900, "yes"), true);
        assert_eq!(st.committed, "yes yes");
    }

    #[test]
    fn trim_keeps_left_context_and_absolute_bookkeeping() {
        let mut st = PseudoStreamState::new();
        st.push(&secs(10.0));
        st.committed_end_ms = 5000;
        st.rtf_ema = 0.1; // tail target = 0.55 / 0.1 = 5.5s
        st.trim_after_commit();
        assert_eq!(
            st.window_start_sample as usize + st.window.len(),
            st.total_samples as usize
        );
        // cut at min(5000 - 800 left ctx, 10000 - 5500 tail) = 4200ms
        assert_eq!(st.window_start_ms(), 4200);
    }

    #[test]
    fn tail_cap_force_commits_and_taints_finality() {
        let mut st = PseudoStreamState::new();
        st.push(&secs(7.0)); // over the 6s hard cap
        st.prev_words = vec![lw(200, 800, "stale"), lw(6500, 6900, "fresh")];
        st.enforce_tail_cap();
        assert_eq!(st.committed, "stale");
        assert!(
            !st.healthy,
            "single-decode commits disqualify the live final"
        );
        assert_eq!(st.prev_words.len(), 1);
        assert_eq!(
            st.window_start_sample as usize + st.window.len(),
            st.total_samples as usize
        );
    }

    #[test]
    fn degraded_first_decode_tentative_then_agreement_commits() {
        let mut st = PseudoStreamState::new();
        st.ts_mode = PseudoTsMode::Degraded;
        st.healthy = false;
        let t0 = Instant::now();
        st.push(&secs(4.0));
        let text = "hello there this is a longer test sentence now";
        // Decode 1: nothing to agree with, everything tentative.
        let (c, t) = st
            .note_decode_degraded(text, t0, Duration::from_millis(200))
            .unwrap();
        assert_eq!(c, "");
        assert_eq!(t, text);
        // Decode 2 agrees: all but the last 4 guard words commit.
        let (c, t) = st
            .note_decode_degraded(
                text,
                t0 + Duration::from_millis(600),
                Duration::from_millis(200),
            )
            .unwrap();
        assert_eq!(c, "hello there this is a");
        assert_eq!(t, "longer test sentence now");
        // Unchanged decode: no re-emit.
        assert!(st
            .note_decode_degraded(
                text,
                t0 + Duration::from_millis(1200),
                Duration::from_millis(200)
            )
            .is_none());
    }

    #[test]
    fn degraded_lcp_regression_never_rewrites_committed() {
        let mut st = PseudoStreamState::new();
        st.ts_mode = PseudoTsMode::Degraded;
        st.healthy = false;
        let t0 = Instant::now();
        st.push(&secs(4.0));
        let a = "alpha beta gamma delta epsilon zeta eta theta";
        let _ = st.note_decode_degraded(a, t0, Duration::from_millis(200));
        let _ = st.note_decode_degraded(
            a,
            t0 + Duration::from_millis(600),
            Duration::from_millis(200),
        );
        assert_eq!(st.committed, "alpha beta gamma delta");
        // A later decode flips an EARLY word: no new commits, no rewrite.
        let flipped = "ALPHA WRONG gamma delta epsilon zeta eta theta";
        let _ = st.note_decode_degraded(
            flipped,
            t0 + Duration::from_millis(1200),
            Duration::from_millis(200),
        );
        assert_eq!(
            st.committed, "alpha beta gamma delta",
            "committed is a ratchet"
        );
    }

    #[test]
    fn degraded_flush_commits_all_and_clears_window() {
        let mut st = PseudoStreamState::new();
        st.ts_mode = PseudoTsMode::Degraded;
        st.healthy = false;
        let t0 = Instant::now();
        st.push(&secs(13.0));
        let text = "one two three four five six seven eight";
        let _ = st.note_decode_degraded(text, t0, Duration::from_millis(300));
        let _ = st.note_decode_degraded(
            text,
            t0 + Duration::from_millis(600),
            Duration::from_millis(300),
        );
        assert_eq!(st.committed, "one two three four");
        let (c, t) = st.flush_degraded(text).unwrap();
        assert_eq!(c, "one two three four five six seven eight");
        assert_eq!(t, "");
        assert!(st.window.is_empty());
        assert_eq!(
            st.window_start_sample, st.total_samples,
            "absolute invariant"
        );
        assert_eq!(st.degraded_committed_words, 0);
        assert!(st.prev_text_words.is_empty());
    }

    #[test]
    fn degraded_flush_due_gap_vs_hard() {
        let mut st = PseudoStreamState::new();
        st.ts_mode = PseudoTsMode::Degraded;
        st.push(&secs(11.0));
        assert!(
            !st.degraded_flush_due(true),
            "11s even at a gap: keep going"
        );
        let mut st = PseudoStreamState::new();
        st.ts_mode = PseudoTsMode::Degraded;
        st.push(&secs(13.0));
        assert!(st.degraded_flush_due(true), "13s at a gap: flush");
        assert!(
            !st.degraded_flush_due(false),
            "13s mid-speech: wait for the gap"
        );
        let mut st = PseudoStreamState::new();
        st.ts_mode = PseudoTsMode::Degraded;
        st.push(&secs(17.0));
        assert!(st.degraded_flush_due(false), "17s: hard flush regardless");
        // Word mode never flushes this way.
        let mut st = PseudoStreamState::new();
        st.push(&secs(17.0));
        assert!(!st.degraded_flush_due(true));
    }

    #[test]
    fn settle_relaxes_only_the_new_audio_gate() {
        let mut st = PseudoStreamState::new();
        let t0 = Instant::now();
        st.push(&secs(1.0));
        let _ = st.note_decode(vec![lw(0, 700, "hello")], t0, Duration::from_millis(100));
        // Tiny trailing audio arrives (under the 0.3s new-audio gate)...
        st.push(&secs(0.15));
        assert!(
            !st.should_decode(t0 + Duration::from_secs(2)),
            "regular cadence ignores sub-gate audio"
        );
        assert!(
            st.should_settle(t0 + Duration::from_secs(2)),
            "the settle decode renders it"
        );
        // ...but not when nothing is undecoded.
        let _ = st.note_decode(
            vec![],
            t0 + Duration::from_secs(2),
            Duration::from_millis(100),
        );
        assert!(!st.should_settle(t0 + Duration::from_secs(4)));
    }

    #[test]
    fn two_slow_ticks_pause_then_cooldown_resumes() {
        let mut st = PseudoStreamState::new();
        let t0 = Instant::now();
        st.push(&secs(2.0)); // at the minimum tail already
        let _ = st.note_decode(vec![], t0, Duration::from_secs(3));
        assert!(st.pause_until.is_none(), "one slow tick is forgiven");
        st.push(&secs(0.5));
        // Second slow tick: started t0+3s, took 3s -> cooldown ends t0+9s.
        let _ = st.note_decode(vec![], t0 + Duration::from_secs(3), Duration::from_secs(3));
        assert!(st.pause_until.is_some());
        assert!(!st.healthy);
        st.push(&secs(0.5)); // fresh audio for the new-audio gate
        assert!(
            !st.should_decode(t0 + Duration::from_millis(8900)),
            "cooldown still holds"
        );
        assert!(
            st.should_decode(t0 + Duration::from_millis(9100)),
            "decoding resumes after the cooldown"
        );
        assert!(!st.healthy, "healthy stays sticky");
    }

    #[test]
    fn pause_retrips_after_recovery() {
        let mut st = PseudoStreamState::new();
        let t0 = Instant::now();
        st.push(&secs(2.0));
        let _ = st.note_decode(vec![], t0, Duration::from_secs(3));
        st.push(&secs(0.3));
        let _ = st.note_decode(vec![], t0 + Duration::from_secs(3), Duration::from_secs(3));
        assert!(st.pause_until.is_some(), "first trip");
        // Post-cooldown: two MORE slow ticks re-trip (slow_ticks was reset).
        // Window must stay at/under the 2.7s trip gate (nothing trims here).
        st.push(&secs(0.2));
        let _ = st.note_decode(vec![], t0 + Duration::from_secs(10), Duration::from_secs(3));
        assert!(
            st.pause_until.is_none(),
            "single slow tick after reset is forgiven"
        );
        st.push(&secs(0.1));
        let _ = st.note_decode(vec![], t0 + Duration::from_secs(13), Duration::from_secs(3));
        assert!(
            st.pause_until.is_some(),
            "re-trips after two fresh slow ticks"
        );
    }

    #[test]
    fn transcript_words_map_to_absolute_time() {
        let mut st = PseudoStreamState::new();
        st.push(&secs(4.0));
        st.window.drain(..32_000);
        st.window_start_sample = 32_000; // 2s trimmed away
        let t = transcribe_cpp::Transcript {
            text: "hi there".into(),
            words: vec![
                transcribe_cpp::Word {
                    t0_ms: 100,
                    t1_ms: 400,
                    text: " hi".into(),
                    ..Default::default()
                },
                transcribe_cpp::Word {
                    t0_ms: 500,
                    t1_ms: 900,
                    text: " there".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let words = st.words_from_transcript(&t);
        assert_eq!(words[0].t0_ms, 2100);
        assert_eq!(words[1].display, "there");
    }

    #[test]
    fn transcript_segment_fallback() {
        let st = PseudoStreamState::new();
        let t = transcribe_cpp::Transcript {
            text: "one two. three four.".into(),
            segments: vec![
                transcribe_cpp::Segment {
                    t0_ms: 0,
                    t1_ms: 1500,
                    text: " one two.".into(),
                    ..Default::default()
                },
                transcribe_cpp::Segment {
                    t0_ms: 1600,
                    t1_ms: 3000,
                    text: " three four.".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let words = st.words_from_transcript(&t);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].display, "one two.");
    }
}
