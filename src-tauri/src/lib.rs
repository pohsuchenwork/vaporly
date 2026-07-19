mod actions;
mod audio_feedback;
pub mod audio_toolkit;
mod auto_learn;
mod catalog;
pub mod cli;
mod clipboard;
mod commands;
mod context;
mod defaults;
mod input;
mod llm_client;
mod managers;
mod overlay;
mod pipeline;
pub mod portable;
mod prompts;
mod settings;
mod shortcut;
mod signal_handle;
mod stream_inject;
#[cfg(test)]
mod stt_smoke;
mod transcription_coordinator;
mod tray;
mod tray_i18n;
mod utils;
mod whisper_calibrate;

pub use cli::CliArgs;
#[cfg(debug_assertions)]
use specta_typescript::{BigIntExportBehavior, Typescript};
use tauri_specta::{collect_commands, collect_events, Builder};

use env_filter::Builder as EnvFilterBuilder;
use managers::audio::AudioRecordingManager;
use managers::history::HistoryManager;
use managers::model::ModelManager;
use managers::transcription::TranscriptionManager;
#[cfg(unix)]
use signal_hook::consts::{SIGINT, SIGTERM, SIGUSR1, SIGUSR2};
#[cfg(unix)]
use signal_hook::iterator::Signals;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tauri::image::Image;
pub use transcription_coordinator::TranscriptionCoordinator;

use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_log::{Builder as LogBuilder, RotationStrategy, Target, TargetKind};

use crate::settings::get_settings;

// Global atomic to store the file log level filter
// We use u8 to store the log::LevelFilter as a number
pub static FILE_LOG_LEVEL: AtomicU8 = AtomicU8::new(log::LevelFilter::Debug as u8);

fn level_filter_from_u8(value: u8) -> log::LevelFilter {
    match value {
        0 => log::LevelFilter::Off,
        1 => log::LevelFilter::Error,
        2 => log::LevelFilter::Warn,
        3 => log::LevelFilter::Info,
        4 => log::LevelFilter::Debug,
        5 => log::LevelFilter::Trace,
        _ => log::LevelFilter::Trace,
    }
}

fn build_console_filter() -> env_filter::Filter {
    let mut builder = EnvFilterBuilder::new();

    match std::env::var("RUST_LOG") {
        Ok(spec) if !spec.trim().is_empty() => {
            if let Err(err) = builder.try_parse(&spec) {
                log::warn!(
                    "Ignoring invalid RUST_LOG value '{}': {}. Falling back to info-level console logging",
                    spec,
                    err
                );
                builder.filter_level(log::LevelFilter::Info);
            }
        }
        _ => {
            builder.filter_level(log::LevelFilter::Info);
        }
    }

    builder.build()
}

fn show_main_window(app: &AppHandle) {
    if let Some(main_window) = app.get_webview_window("main") {
        if let Err(e) = main_window.unminimize() {
            log::error!("Failed to unminimize webview window: {}", e);
        }
        if let Err(e) = main_window.show() {
            log::error!("Failed to show webview window: {}", e);
        }
        if let Err(e) = main_window.set_focus() {
            log::error!("Failed to focus webview window: {}", e);
        }
        #[cfg(target_os = "macos")]
        {
            if let Err(e) = app.set_activation_policy(tauri::ActivationPolicy::Regular) {
                log::error!("Failed to set activation policy to Regular: {}", e);
            }
        }
        return;
    }

    let webview_labels = app.webview_windows().keys().cloned().collect::<Vec<_>>();
    log::error!(
        "Main window not found. Webview labels: {:?}",
        webview_labels
    );
}

/// Log a fatal startup error, show it to the user, and exit cleanly. A stable
/// build must not bounce silently (a bare panic in a windowless GUI process is
/// invisible) when a manager fails to init, e.g. a corrupt store or unreadable
/// history database. The dialog is best-effort; the log and stderr always fire.
fn fatal_startup(app: &AppHandle, context: &str, err: impl std::fmt::Display) -> ! {
    let msg = format!("{context}: {err}");
    log::error!("fatal startup error: {msg}");
    eprintln!("Vaporly failed to start: {msg}");
    #[cfg(desktop)]
    {
        use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
        let _ = app
            .dialog()
            .message(format!(
                "Vaporly could not start.\n\n{msg}\n\nYour data is in the computer.vaporly application support folder. Please report this issue."
            ))
            .title("Vaporly failed to start")
            .kind(MessageDialogKind::Error)
            .blocking_show();
    }
    std::process::exit(1);
}

/// Unwrap a startup Result or call `fatal_startup`. Replaces `.expect(...)` on
/// the GUI init path so a failure is surfaced instead of a silent crash.
fn unwrap_or_fatal<T, E: std::fmt::Display>(app: &AppHandle, context: &str, r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => fatal_startup(app, context, e),
    }
}

fn initialize_core_logic(app_handle: &AppHandle) {
    // Note: Enigo (keyboard/mouse simulation) is NOT initialized here.
    // The frontend is responsible for calling the `initialize_enigo` command
    // after onboarding completes. This avoids triggering permission dialogs
    // on macOS before the user is ready.

    // Initialize the managers. The audio recorder receives the streaming router
    // explicitly, so always-on microphone startup can wire live-preview frames
    // even before Tauri state is populated.
    let model_manager = Arc::new(unwrap_or_fatal(
        app_handle,
        "initializing the model manager",
        ModelManager::new(app_handle),
    ));
    let transcription_manager = Arc::new(unwrap_or_fatal(
        app_handle,
        "initializing the transcription manager",
        TranscriptionManager::new(app_handle, model_manager.clone()),
    ));
    let recording_manager = Arc::new(unwrap_or_fatal(
        app_handle,
        "initializing the audio recorder",
        AudioRecordingManager::new(app_handle, transcription_manager.stream_router()),
    ));
    let history_manager = Arc::new(unwrap_or_fatal(
        app_handle,
        "initializing history",
        HistoryManager::new(app_handle),
    ));

    // Initialize the transcribe-cpp native backend (logging + backend module
    // registration) once, before any whisper model is loaded.
    managers::transcription::init_transcribe_backend();

    // Apply accelerator preferences before any model loads
    managers::transcription::apply_accelerator_settings(app_handle);

    // Add managers to Tauri's managed state
    app_handle.manage(recording_manager.clone());
    app_handle.manage(model_manager.clone());
    app_handle.manage(transcription_manager.clone());
    app_handle.manage(history_manager.clone());

    // Bundled LLM engine: reap any llama-server orphaned by a previous hard
    // kill. The engine is demand-driven: a machine with every stage on
    // Deterministic never runs llama-server at all. When some cleanup stage
    // IS set to the Model engine, warm it at launch (round 20) so the FIRST
    // post-boot dictation gets cleaned instead of hitting the cold-engine
    // skip in `engine_ready_or_skip` while the model is still loading.
    let llm_engine = Arc::new(managers::llm_engine::LlmEngineManager::new(
        app_handle.clone(),
    ));
    llm_engine.reap_orphan();
    llm_engine.reap_strays();
    app_handle.manage(llm_engine.clone());
    if settings::get_settings(&app_handle).model_pass_needed() {
        llm_engine.ensure_running();
    }

    // G2 incremental-cleaner slot (one active dictation at a time).
    app_handle.manage(actions::CleanerSlot(std::sync::Mutex::new(None)));
    // F1 per-dictation snapshot (app context + stage config), same lifecycle.
    app_handle.manage(pipeline::DictationContextSlot(std::sync::Mutex::new(None)));
    // F3 textbox-streaming injector slot, same lifecycle.
    app_handle.manage(stream_inject::InjectorSlot(std::sync::Mutex::new(None)));
    // F4 post-paste watcher slot (macOS AX observation; one at a time).
    app_handle.manage(auto_learn::AxWatcherSlot(std::sync::Mutex::new(None)));

    // Note: Shortcuts are NOT initialized here.
    // The frontend is responsible for calling the `initialize_shortcuts` command
    // after permissions are confirmed (on macOS) or after onboarding completes.
    // This matches the pattern used for Enigo initialization.

    #[cfg(unix)]
    let signals = Signals::new([SIGUSR1, SIGUSR2, SIGTERM, SIGINT]).unwrap();
    // Set up signal handlers for toggling transcription
    #[cfg(unix)]
    signal_handle::setup_signal_handler(app_handle.clone(), signals);

    // Get the current theme to set the appropriate initial icon
    let initial_theme = tray::get_current_theme(app_handle);

    // Choose the appropriate initial icon based on theme
    let initial_icon_path = tray::get_icon_path(initial_theme, tray::TrayIconState::Idle);

    let tray = TrayIconBuilder::new()
        .icon(
            Image::from_path(
                app_handle
                    .path()
                    .resolve(initial_icon_path, tauri::path::BaseDirectory::Resource)
                    .unwrap(),
            )
            .unwrap(),
        )
        .tooltip(tray::tray_tooltip())
        .show_menu_on_left_click(true)
        .icon_as_template(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "settings" => {
                show_main_window(app);
            }
            "check_updates" => {
                let settings = settings::get_settings(app);
                if settings.update_checks_enabled {
                    show_main_window(app);
                    let _ = app.emit("check-for-updates", ());
                }
            }
            "copy_last_transcript" => {
                tray::copy_last_transcript(app);
            }
            "cancel" => {
                use crate::utils::cancel_current_operation;

                // Use centralized cancellation that handles all operations
                cancel_current_operation(app);
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .build(app_handle)
        .unwrap();
    app_handle.manage(tray);

    // Initialize tray menu with idle state
    utils::update_tray_menu(app_handle, &utils::TrayIconState::Idle, None);

    // Get the autostart manager and configure based on user setting
    let autostart_manager = app_handle.autolaunch();
    let settings = settings::get_settings(app_handle);

    if settings.autostart_enabled {
        // Enable autostart if user has opted in
        let _ = autostart_manager.enable();
    } else {
        // Disable autostart if user has opted out
        let _ = autostart_manager.disable();
    }

    // Create the recording overlay window (hidden by default)
    utils::create_recording_overlay(app_handle);
}

#[tauri::command]
#[specta::specta]
fn trigger_update_check(app: AppHandle) -> Result<(), String> {
    let settings = settings::get_settings(&app);
    if !settings.update_checks_enabled {
        return Ok(());
    }
    app.emit("check-for-updates", ())
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
fn show_main_window_command(app: AppHandle) -> Result<(), String> {
    show_main_window(&app);
    Ok(())
}

/// Headless one-shot transcription for the `--transcribe-file` / `--list-devices`
/// path. Drives the same `TranscriptionManager::transcribe` the app uses; no
/// mic, no VAD, no download. Returns a process exit code (0 ok, 1 runtime
/// failure, 2 bad input/usage).
fn run_headless_transcription(app: &AppHandle, args: &CliArgs) -> i32 {
    use std::time::Instant;

    // --list-devices: print registered compute devices (with indices) and exit.
    // Useful on multi-GPU machines to discover the index for --device-index.
    if args.list_devices {
        let devices = crate::managers::transcription::describe_compute_devices();
        if devices.is_empty() {
            println!("No transcribe-cpp compute devices registered.");
        } else {
            println!("transcribe-cpp compute devices:");
            for d in &devices {
                println!("  {}", d);
            }
        }
        if args.transcribe_file.is_none() {
            return 0;
        }
    }

    let Some(wav) = args.transcribe_file.clone() else {
        return 0;
    };

    // read_wav_samples reads 16-bit int samples and does no validation; the app
    // only ever saves 16 kHz mono 16-bit PCM, so reject anything else rather than
    // transcribe garbage / mis-time / mis-decode.
    match hound::WavReader::open(&wav) {
        Ok(reader) => {
            let spec = reader.spec();
            if spec.sample_rate != 16_000
                || spec.channels != 1
                || spec.bits_per_sample != 16
                || spec.sample_format != hound::SampleFormat::Int
            {
                eprintln!(
                    "error: expected 16 kHz mono 16-bit PCM WAV, got {} Hz / {} ch / {}-bit {:?}",
                    spec.sample_rate, spec.channels, spec.bits_per_sample, spec.sample_format
                );
                return 2;
            }
        }
        Err(e) => {
            eprintln!("error: cannot open {}: {}", wav.display(), e);
            return 2;
        }
    }

    let samples = match crate::audio_toolkit::read_wav_samples(&wav) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to read {}: {}", wav.display(), e);
            return 2;
        }
    };
    let audio_secs = samples.len() as f64 / 16_000.0;

    let tm = app.state::<Arc<TranscriptionManager>>();

    // The one supported model; no flag or picker.
    let model_id = crate::managers::model::FIXED_STT_MODEL_ID.to_string();

    // --device-index hard-selects a compute device by its --list-devices registry
    // index (not persisted). Omit it to use the persisted accelerator setting.
    let device_index = args.device_index;
    let requested_device = match device_index {
        Some(idx) => format!("index {}", idx),
        None => "settings".to_string(),
    };

    // Cold load (timed).
    let load_start = Instant::now();
    if let Err(e) = tm.load_model_with_device(&model_id, device_index) {
        eprintln!("error: load_model('{}') failed: {}", model_id, e);
        return 1;
    }
    let load_ms = load_start.elapsed().as_millis() as u64;
    let bound_backend = tm.current_backend();

    // Hidden live-preview bench: VAPORLY_STREAM_BENCH=1 replays the WAV through
    // the real pseudo-stream path (paced feed, StreamTextEvent cadence,
    // authoritative finalize) instead of batch transcription. Dev-only; used by
    // the release QA scripts. VAPORLY_STREAM_BENCH_PACE=N replays N× real time.
    if std::env::var("VAPORLY_STREAM_BENCH").is_ok() {
        return run_stream_bench(app, &tm, &samples, audio_secs, &model_id, load_ms);
    }

    let runs = args.repeat.unwrap_or(1).max(1);
    let mut times_ms: Vec<u64> = Vec::new();
    let mut text = String::new();
    for i in 0..runs {
        // If the model's unload-timeout is "Immediately", transcribe() unloads
        // the engine after each run; reload (untimed) so repeats keep working
        // and the inference timing below stays clean.
        if !tm.is_model_loaded() {
            if let Err(e) = tm.load_model_with_device(&model_id, device_index) {
                eprintln!("error: reload before run {} failed: {}", i + 1, e);
                return 1;
            }
        }
        let t = Instant::now();
        match tm.transcribe(samples.clone()) {
            Ok(out) => text = out,
            Err(e) => {
                eprintln!("error: transcribe failed: {}", e);
                return 1;
            }
        }
        times_ms.push(t.elapsed().as_millis() as u64);
    }
    let best_ms = times_ms.iter().copied().min().unwrap_or(0);
    let rtf = if best_ms > 0 {
        audio_secs / (best_ms as f64 / 1000.0)
    } else {
        0.0
    };

    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "model": model_id,
                "requested_device": requested_device,
                "bound_backend": bound_backend,
                "audio_secs": audio_secs,
                "load_ms": load_ms,
                "transcribe_ms": times_ms,
                "best_ms": best_ms,
                "rtf": rtf,
                "text": text,
            })
        );
    } else {
        println!(
            "model={} device={} backend={} audio={:.2}s load={}ms best={}ms rtf={:.2}x",
            model_id,
            requested_device,
            bound_backend.as_deref().unwrap_or("?"),
            audio_secs,
            load_ms,
            best_ms,
            rtf,
        );
        println!("text: {}", text);
    }
    0
}

/// Replay a WAV through the real live-preview path and report cadence stats.
/// Dev-only (env-gated); exercises start_stream → paced feeds → finalize, the
/// exact pipeline a dictation uses, with no mic or overlay involved.
fn run_stream_bench(
    app: &AppHandle,
    tm: &Arc<TranscriptionManager>,
    samples: &[f32],
    audio_secs: f64,
    model_id: &str,
    load_ms: u64,
) -> i32 {
    use std::time::{Duration, Instant};
    use tauri::Listener;

    let pace: f32 = std::env::var("VAPORLY_STREAM_BENCH_PACE")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|p: &f32| *p > 0.0)
        .unwrap_or(1.0);

    // (wall time, committed chars, tentative chars) per StreamTextEvent.
    let events: Arc<std::sync::Mutex<Vec<(Instant, usize, usize)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let events_sink = Arc::clone(&events);
    let listener = app.listen_any("stream-text-event", move |event| {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(event.payload()) {
            let len = |k: &str| v.get(k).and_then(|s| s.as_str()).map_or(0, |s| s.len());
            events_sink
                .lock()
                .unwrap()
                .push((Instant::now(), len("committed"), len("tentative")));
        }
    });

    tm.start_stream(true);
    let router = tm.stream_router();
    let chunk = 1_600; // 100ms at 16 kHz, the recorder's frame ballpark
                       // VAPORLY_STREAM_BENCH_VAD=1 emulates the recorder's VAD gating: silent
                       // frames are NOT fed (their pacing sleep is kept), so real wall-clock
                       // feed gaps appear exactly like live dictation pauses. An RMS energy
                       // gate is deterministic and sufficient for [[slnc]] fixtures.
    let vad_gate = std::env::var("VAPORLY_STREAM_BENCH_VAD").is_ok();
    let feed_start = Instant::now();
    let mut fed_frames: u64 = 0;
    let mut skipped_frames: u64 = 0;
    for (i, frame) in samples.chunks(chunk).enumerate() {
        let silent = vad_gate && {
            let energy: f32 = frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32;
            energy.sqrt() < 0.004
        };
        if silent {
            skipped_frames += 1;
        } else {
            router.feed(frame);
            fed_frames += 1;
        }
        // Pace the replay against absolute audio time to avoid sleep drift.
        let audio_elapsed = (i + 1) as f32 * 0.1 / pace;
        let due = feed_start + Duration::from_secs_f32(audio_elapsed);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
    if vad_gate {
        eprintln!("bench vad gate: fed {fed_frames} frames, skipped {skipped_frames} silent");
    }

    let finalize_start = Instant::now();
    let finalized = tm.finalize_stream();
    let finalize_ms = finalize_start.elapsed().as_millis() as u64;
    app.unlisten(listener);

    let (live_final, live_authoritative) = match finalized {
        Ok(Some(text)) => (text, true),
        Ok(None) => (String::new(), false),
        Err(e) => {
            eprintln!("error: finalize_stream failed: {}", e);
            return 1;
        }
    };

    // Batch reference over the same audio, for output comparison.
    if !tm.is_model_loaded() {
        if let Err(e) = tm.load_model(model_id) {
            eprintln!("error: reload for batch reference failed: {}", e);
            return 1;
        }
    }
    let batch_start = Instant::now();
    let batch_text = match tm.transcribe(samples.to_vec()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: batch reference transcription failed: {}", e);
            return 1;
        }
    };
    let batch_ms = batch_start.elapsed().as_millis() as u64;

    let events = events.lock().unwrap();
    let mut intervals: Vec<u64> = events
        .windows(2)
        .map(|w| w[1].0.duration_since(w[0].0).as_millis() as u64)
        .collect();
    intervals.sort_unstable();
    let pct = |p: f64| -> u64 {
        if intervals.is_empty() {
            0
        } else {
            intervals[((intervals.len() - 1) as f64 * p) as usize]
        }
    };
    let final_committed = events.last().map_or(0, |e| e.1);
    let max_committed_lag_chars = events.iter().map(|e| e.2).max().unwrap_or(0);

    println!(
        "{}",
        serde_json::json!({
            "bench": "stream",
            "model": model_id,
            "audio_secs": audio_secs,
            "pace": pace,
            "load_ms": load_ms,
            "events": events.len(),
            "event_interval_ms": {
                "p50": pct(0.50),
                "p95": pct(0.95),
                "max": intervals.last().copied().unwrap_or(0),
            },
            "final_committed_chars": final_committed,
            "max_tentative_chars": max_committed_lag_chars,
            "finalize_ms": finalize_ms,
            "live_authoritative": live_authoritative,
            "live_text": live_final,
            "batch_ms": batch_ms,
            "batch_text": batch_text,
        })
    );
    0
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run(cli_args: CliArgs) {
    // Detect portable mode before anything else
    portable::init();

    // Parse console logging directives from RUST_LOG, falling back to info-level logging
    // when the variable is unset
    let console_filter = build_console_filter();

    let specta_builder = Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            shortcut::change_binding,
            shortcut::reset_binding,
            shortcut::suspend_binding,
            shortcut::resume_binding,
            shortcut::change_globe_key_notice_dismissed_setting,
            shortcut::change_audio_feedback_setting,
            shortcut::change_audio_feedback_volume_setting,
            shortcut::change_sound_theme_setting,
            shortcut::change_autostart_setting,
            shortcut::change_update_checks_setting,
            shortcut::change_overlay_position_setting,
            shortcut::change_overlay_style_setting,
            shortcut::change_append_trailing_space_setting,
            shortcut::change_keep_result_on_clipboard_setting,
            shortcut::change_onboarding_completed_setting,
            shortcut::change_custom_words_level_setting,
            shortcut::change_custom_phrases_level_setting,
            shortcut::change_whisper_mode_setting,
            shortcut::change_whisper_strength_setting,
            shortcut::change_theme_mode_setting,
            shortcut::change_accent_preset_setting,
            shortcut::change_filler_level_setting,
            shortcut::change_filler_engine_setting,
            shortcut::change_mind_change_level_setting,
            shortcut::change_mind_change_engine_setting,
            shortcut::change_context_awareness_setting,
            shortcut::change_auto_learn_mode_setting,
            shortcut::update_custom_words,
            shortcut::update_custom_phrases,
            shortcut::change_keyboard_implementation_setting,
            shortcut::get_keyboard_implementation,
            shortcut::native_keys::start_native_keys_recording,
            shortcut::native_keys::stop_native_keys_recording,
            trigger_update_check,
            show_main_window_command,
            commands::cancel_operation,
            commands::is_portable,
            commands::get_app_dir_path,
            commands::get_app_settings,
            commands::get_default_settings,
            commands::get_log_dir_path,
            commands::open_recordings_folder,
            commands::open_log_dir,
            commands::open_app_data_dir,
            commands::initialize_enigo,
            commands::initialize_shortcuts,
            commands::reset_all_settings,
            commands::reset_onboarding,
            commands::models::get_available_models,
            commands::models::get_model_info,
            commands::models::download_model,
            commands::models::cancel_download,
            commands::models::get_transcription_model_status,
            commands::models::is_model_loading,
            commands::llm::get_llm_engine_status,
            commands::llm::restart_llm_engine,
            commands::llm::get_hardware_profile,
            commands::llm::get_llm_models,
            commands::llm::download_llm_model,
            commands::llm::cancel_llm_model_download,
            commands::llm::delete_llm_model,
            commands::llm::set_llm_model,
            commands::llm::llm_engine_selftest,
            commands::audio::get_windows_microphone_permission_status,
            commands::audio::open_microphone_privacy_settings,
            commands::audio::get_available_microphones,
            commands::audio::set_selected_microphone,
            commands::audio::get_selected_microphone,
            commands::audio::get_available_output_devices,
            commands::audio::set_selected_output_device,
            commands::audio::get_selected_output_device,
            commands::audio::play_test_sound,
            commands::audio::check_custom_sounds,
            commands::audio::whisper_calibration_phase_start,
            commands::audio::whisper_calibration_phase_stop,
            commands::audio::whisper_calibration_finish,
            commands::audio::whisper_calibration_clear,
            commands::audio::is_recording,
            commands::transcription::get_model_load_status,
            commands::transcription::unload_model_manually,
            commands::history::get_history_entries,
            commands::history::search_history_entries,
            commands::history::get_audio_file_path,
            commands::history::delete_history_entry,
            commands::history::retry_history_entry_transcription,
            commands::history::update_history_entry_text,
            commands::history::update_history_limit,
            commands::history::update_recording_retention_period,
        ])
        .events(collect_events![
            managers::history::HistoryUpdatePayload,
            managers::transcription::StreamTextEvent,
            managers::transcription::StreamPhaseEvent,
        ]);

    #[cfg(debug_assertions)] // <- Only export on non-release builds
    specta_builder
        .export(
            Typescript::default().bigint(BigIntExportBehavior::Number),
            "../src/bindings.ts",
        )
        .expect("Failed to export typescript bindings");

    let invoke_handler = specta_builder.invoke_handler();

    // The headless path must run as its own instance (see the single-instance
    // note below), not forward to an already-running app.
    let headless_mode = cli_args.transcribe_file.is_some() || cli_args.list_devices;

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
        .device_event_filter(tauri::DeviceEventFilter::Always)
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            LogBuilder::new()
                .level(log::LevelFilter::Trace) // Set to most verbose level globally
                .max_file_size(500_000)
                .rotation_strategy(RotationStrategy::KeepOne)
                .clear_targets()
                .targets([
                    // Console output respects RUST_LOG environment variable. In
                    // headless mode (--transcribe-file/--list-devices)
                    // stdout carries only the result (JSON or plain), so send console
                    // logs to stderr instead to keep stdout clean for CI parsing.
                    Target::new(if headless_mode {
                        TargetKind::Stderr
                    } else {
                        TargetKind::Stdout
                    })
                    .filter({
                        let console_filter = console_filter.clone();
                        move |metadata| console_filter.enabled(metadata)
                    }),
                    // File logs respect the user's settings (stored in FILE_LOG_LEVEL atomic)
                    Target::new(if let Some(data_dir) = portable::data_dir() {
                        TargetKind::Folder {
                            path: data_dir.join("logs"),
                            file_name: Some("vaporly".into()),
                        }
                    } else {
                        TargetKind::LogDir {
                            file_name: Some("vaporly".into()),
                        }
                    })
                    .filter(|metadata| {
                        let file_level = FILE_LOG_LEVEL.load(Ordering::Relaxed);
                        metadata.level() <= level_filter_from_u8(file_level)
                    }),
                ])
                .build(),
        );

    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(tauri_nspanel::init());
    }

    // Single-instance forwards CLI args to an already-running Vaporly and exits.
    // That would make the headless path
    // (--transcribe-file/--list-devices) a silent no-op whenever the
    // app is already open, so skip it in headless mode and run a standalone
    // instance instead.
    if !headless_mode {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if args.iter().any(|a| a == "--toggle-transcription") {
                signal_handle::send_transcription_input(app, "transcribe", "CLI");
            } else if args.iter().any(|a| a == "--cancel") {
                crate::utils::cancel_current_operation(app);
            } else {
                show_main_window(app);
            }
        }));
    }

    builder
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_macos_permissions::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![]),
        ))
        .setup(move |app| {
            specta_builder.mount_events(app);

            // Headless one-shot path (`--transcribe-file` / `--list-devices`):
            // initialize only what transcription needs, the
            // store/paths plugins, the model + transcription managers, and the
            // transcribe-cpp backend + accelerator settings, then run on a worker
            // thread and exit. Deliberately skips the window, tray, overlay, audio
            // recorder (so it never opens the mic, even with always_on_microphone),
            // signal handlers, and autostart that initialize_core_logic sets up.
            if headless_mode {
                let app_handle = app.handle().clone();
                let model_manager = Arc::new(
                    ModelManager::new(&app_handle).expect("Failed to initialize model manager"),
                );
                let transcription_manager = Arc::new(
                    TranscriptionManager::new(&app_handle, model_manager.clone())
                        .expect("Failed to initialize transcription manager"),
                );
                app_handle.manage(model_manager);
                app_handle.manage(transcription_manager);
                managers::transcription::init_transcribe_backend();
                managers::transcription::apply_accelerator_settings(&app_handle);

                let handle = app_handle.clone();
                let args = cli_args.clone();
                std::thread::spawn(move || {
                    let code = run_headless_transcription(&handle, &args);
                    // Drop the loaded engine before teardown: ggml-metal's global
                    // device free asserts (SIGABRT) if a model's Metal resources
                    // are still alive at C++ static-destructor time.
                    if let Some(tm) = handle.try_state::<Arc<TranscriptionManager>>() {
                        let _ = tm.unload_model();
                    }
                    // process::exit (not app.exit, which exits 0 regardless) so the
                    // exit code propagates to the shell for CI gating. Flush first
                    // since process::exit runs no destructors / buffer flushes.
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                    let _ = std::io::stderr().flush();
                    std::process::exit(code);
                });
                return Ok(());
            }

            // Create main window programmatically so we can set data_directory
            // for portable mode (redirects WebView2 cache to portable Data dir)
            let mut win_builder =
                tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::App("/".into()))
                    .title("Vaporly")
                    .inner_size(680.0, 570.0)
                    .min_inner_size(680.0, 570.0)
                    .resizable(true)
                    .maximizable(false)
                    .visible(false);

            if let Some(data_dir) = portable::data_dir() {
                win_builder = win_builder.data_directory(data_dir.join("webview"));
            }

            win_builder.build()?;

            let settings = get_settings(app.handle());

            // File logging defaults to Debug (see FILE_LOG_LEVEL); the CLI
            // --debug flag raises it to Trace for this run only.
            if cli_args.debug {
                FILE_LOG_LEVEL.store(log::LevelFilter::Trace as u8, Ordering::Relaxed);
            }
            let app_handle = app.handle().clone();
            app.manage(TranscriptionCoordinator::new(app_handle.clone()));

            initialize_core_logic(&app_handle);

            // Populate the overlay-enabled cache from initial settings so the
            // audio path (overlay::emit_levels, called ~24 Hz during recording)
            // can do a single atomic load instead of reading the Tauri store.
            // Kept in sync by shortcut::change_overlay_style_setting.
            overlay::update_overlay_enabled_cache(
                settings.overlay_style != settings::OverlayStyle::None,
            );

            // v2 always starts visible (no start-hidden mode) with the tray
            // always present.
            show_main_window(&app_handle);

            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _res = window.hide();

                #[cfg(target_os = "macos")]
                {
                    // The tray is always available in v2: hide the dock icon,
                    // the app lives in the tray.
                    let res = window
                        .app_handle()
                        .set_activation_policy(tauri::ActivationPolicy::Accessory);
                    if let Err(e) = res {
                        log::error!("Failed to set activation policy: {}", e);
                    }
                }
            }
            tauri::WindowEvent::ThemeChanged(theme) => {
                log::info!("Theme changed to: {:?}", theme);
                // Update tray icon to match new theme, maintaining idle state
                utils::change_tray_icon(window.app_handle(), utils::TrayIconState::Idle);
            }
            _ => {}
        })
        .invoke_handler(invoke_handler)
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| match &event {
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => {
                show_main_window(app);
            }
            // Teardown transcribe.cpp before exit
            tauri::RunEvent::Exit => {
                // Stop the bundled llama-server before the runtime tears down
                // (kill_on_drop + pidfile-reap cover the pathological paths).
                if let Some(engine) = app.try_state::<Arc<managers::llm_engine::LlmEngineManager>>()
                {
                    engine.stop();
                }
                if let Some(tm) = app.try_state::<Arc<TranscriptionManager>>() {
                    let _ = tm.unload_model();
                }
            }
            _ => {}
        });
}
