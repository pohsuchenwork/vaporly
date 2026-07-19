use crate::TranscriptionCoordinator;
#[cfg(unix)]
use log::debug;
use log::warn;
use tauri::{AppHandle, Manager};

#[cfg(unix)]
use signal_hook::consts::{SIGINT, SIGTERM, SIGUSR1, SIGUSR2};
#[cfg(unix)]
use signal_hook::iterator::Signals;
#[cfg(unix)]
use std::thread;

/// Send a transcription input to the coordinator.
/// Used by signal handlers, CLI flags, and any other external trigger.
pub fn send_transcription_input(app: &AppHandle, binding_id: &str, source: &str) {
    if let Some(c) = app.try_state::<TranscriptionCoordinator>() {
        c.send_input(binding_id, source, true, false);
    } else {
        warn!("TranscriptionCoordinator not initialized");
    }
}

#[cfg(unix)]
pub fn setup_signal_handler(app_handle: AppHandle, mut signals: Signals) {
    debug!("Signal handlers registered (SIGUSR1, SIGUSR2, SIGTERM, SIGINT)");
    thread::spawn(move || {
        for sig in signals.forever() {
            let (binding_id, signal_name) = match sig {
                // Both user signals toggle the one dictation binding (the
                // alternate raw/formatted binding is gone in v2).
                SIGUSR1 => ("transcribe", "SIGUSR1"),
                SIGUSR2 => ("transcribe", "SIGUSR2"),
                SIGTERM | SIGINT => {
                    // Default signal death would skip every destructor and
                    // orphan the llama-server (observed: a stray holding
                    // gigabytes after a pkill). Stop the engine, then exit
                    // through the normal path.
                    warn!("Received termination signal {sig}; stopping engine and exiting");
                    if let Some(engine) = app_handle
                        .try_state::<std::sync::Arc<crate::managers::llm_engine::LlmEngineManager>>(
                        )
                    {
                        engine.stop();
                    }
                    app_handle.exit(0);
                    continue;
                }
                _ => continue,
            };
            debug!("Received {signal_name}");
            send_transcription_input(&app_handle, binding_id, signal_name);
        }
    });
}
