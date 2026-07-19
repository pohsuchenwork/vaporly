use crate::audio_feedback::{play_feedback_sound, play_feedback_sound_blocking, SoundType};
use crate::audio_toolkit::{is_microphone_access_denied, is_no_input_device_error, VadPolicy};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::history::HistoryManager;
use crate::managers::model::ModelManager;
use crate::managers::transcription::StreamWorkKind;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::{get_settings, AppSettings, OverlayStyle};
use crate::shortcut;
use crate::tray::{change_tray_icon, TrayIconState};
use crate::utils::{
    self, show_processing_overlay, show_recording_overlay, show_transcribing_overlay,
};
use crate::TranscriptionCoordinator;
use log::{debug, error, info, warn};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tauri::Manager;
use tauri::{AppHandle, Emitter};

#[derive(Clone, serde::Serialize)]
struct RecordingErrorEvent {
    error_type: String,
    detail: Option<String>,
}

/// Drop guard that notifies the [`TranscriptionCoordinator`] when the
/// transcription pipeline finishes, whether it completes normally or panics.
/// Also releases the dictation's context snapshot so a later re-transcribe
/// or headless run can never see a stale foreground-app context, and clears
/// the textbox injector slot (already-taken on the normal finalize/cancel
/// paths; on an error path the injector is dropped WITHOUT wiping, leaving
/// the streamed words in the target app: they may be the only surviving copy
/// when transcription itself failed).
struct FinishGuard(AppHandle);
impl Drop for FinishGuard {
    fn drop(&mut self) {
        if let Some(slot) = self.0.try_state::<crate::pipeline::DictationContextSlot>() {
            slot.0.lock().unwrap().take();
        }
        if let Some(slot) = self.0.try_state::<crate::stream_inject::InjectorSlot>() {
            slot.0.lock().unwrap().take();
        }
        if let Some(c) = self.0.try_state::<TranscriptionCoordinator>() {
            c.notify_processing_finished();
        }
    }
}

// Shortcut Action Trait
pub trait ShortcutAction: Send + Sync {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str);
    fn stop(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str);
}

// Transcribe Action
struct TranscribeAction {
    post_process: bool,
}

/// Send a fully-rendered prompt to the bundled cleanup engine and return the
/// raw completion. The engine selftest command's request path; dictation
/// cleanup goes through `pipeline::model_pass::clean_text` instead. Waits on
/// the same single-flight gate, so a selftest can never race a dictation's
/// cleanup call on the local engine (v1 7B-thrash lesson).
pub(crate) async fn llm_complete(settings: &AppSettings, prompt_text: String) -> Option<String> {
    let provider = crate::managers::llm_engine::engine_provider();
    let model = crate::managers::llm_engine::cleanup_model_id(settings);
    if model.trim().is_empty() {
        debug!("LLM call skipped: no cleanup model for this hardware tier");
        return None;
    }

    let _in_flight = crate::pipeline::model_pass::llm_gate().lock().await;

    match crate::llm_client::send_chat_completion(
        &provider,
        String::new(),
        &model,
        prompt_text,
        Some("none".to_string()),
        None,
    )
    .await
    {
        Ok(Some(content)) => {
            let content = crate::pipeline::model_pass::strip_invisible_chars(&content);
            debug!(
                "LLM call succeeded (model '{}'). Output length: {} chars",
                model,
                content.len()
            );
            Some(content)
        }
        Ok(None) => {
            error!("LLM API response has no content");
            None
        }
        Err(e) => {
            error!("LLM call failed: {e}");
            None
        }
    }
}

pub(crate) struct ProcessedTranscription {
    pub final_text: String,
    pub post_processed_text: Option<String>,
    pub post_process_prompt: Option<String>,
}

/// Vaporly-engine warming gate: when the engine's port isn't live yet, wait
/// briefly (covers "dictated right after login"); on timeout emit
/// `post-process-skipped` and let the deterministic text paste. Never blocks
/// longer than 10s.
async fn engine_ready_or_skip(app: &AppHandle) -> bool {
    use crate::managers::llm_engine::{self, EngineState, LlmEngineManager};
    if llm_engine::ENGINE_PORT.load(std::sync::atomic::Ordering::Acquire) != 0 {
        return true;
    }
    let state = app
        .try_state::<std::sync::Arc<LlmEngineManager>>()
        .map(|e| e.state());
    let reason = match state {
        Some(EngineState::Spawning) | Some(EngineState::LoadingModel) => {
            if llm_engine::wait_ready_bounded(std::time::Duration::from_secs(10)).await {
                return true;
            }
            "engine_warming"
        }
        _ => {
            // Nudge a recovery attempt for next time; skip this dictation.
            if let Some(engine) = app.try_state::<std::sync::Arc<LlmEngineManager>>() {
                engine.ensure_running();
            }
            "engine_down"
        }
    };
    warn!("post-processing skipped: {reason}");
    let _ = app.emit(
        "post-process-skipped",
        serde_json::json!({ "reason": reason }),
    );
    false
}

/// Lowercase alphanumerics only: compares two texts ignoring whitespace,
/// punctuation, and case (so a shaped/expanded transcript still matches the
/// user's saved phrase text).
fn norm_loose(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// I7 over-collapse guard: true when the model returned EXACTLY one phrase's
/// saved text while the transcript clearly contained more than its trigger,
/// i.e. the model ate the dictation.
fn over_collapsed(settings: &AppSettings, input: &str, output: &str) -> bool {
    let norm = |s: &str| {
        s.split_whitespace()
            .map(|w| w.to_lowercase())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let out_n = norm(output);
    settings.custom_phrases.iter().any(|p| {
        norm(&p.write) == out_n
            && input.split_whitespace().count() > p.say.split_whitespace().count() + 8
    })
}

/// True when the cleanup model answered the dictation as if it were a chat
/// prompt instead of returning a cleaned transcript. Dictating a command-like
/// phrase ("rewrite this") can pull a small instruct model into replying
/// ("Sure, please provide the text..."), which is longer than the input and
/// opens with an assistant tell the speaker never said. When detected we keep
/// the deterministic text (the words actually spoken) rather than paste the
/// model's answer.
fn looks_conversational(input: &str, output: &str) -> bool {
    const TELLS: &[&str] = &[
        "sure",
        "certainly",
        "of course",
        "i can ",
        "i can't",
        "i cannot",
        "i'm sorry",
        "i am sorry",
        "as an ai",
        "please provide",
        "could you",
        "i'd be happy",
        "i would be happy",
        "here is",
        "here's",
        "it seems",
        "let me know",
        "understood",
        "i'll help",
        "i will help",
    ];
    let norm_start = |s: &str| {
        s.trim_start()
            .trim_start_matches(['"', '\'', '`'])
            .to_lowercase()
    };
    let out_l = norm_start(output);
    let in_l = norm_start(input);
    // A tell that opens the OUTPUT but not the INPUT was injected by the model,
    // not spoken by the user (so a genuine dictation starting with "Sure, ..."
    // is not flagged, since the input opens with it too).
    let opener_injected = TELLS
        .iter()
        .any(|t| out_l.starts_with(t) && !in_l.starts_with(t));

    // Cleanup never meaningfully lengthens text. A reply much longer than the
    // input that also shares few of the input's words is the model writing
    // something new rather than cleaning.
    let in_words: Vec<String> = input
        .split_whitespace()
        .map(norm_loose)
        .filter(|w| !w.is_empty())
        .collect();
    let out_word_count = output.split_whitespace().count();
    let over_expanded = out_word_count > in_words.len() * 8 / 5 + 5;
    let low_overlap = if in_words.is_empty() {
        false
    } else {
        let out_words: std::collections::HashSet<String> = output
            .split_whitespace()
            .map(norm_loose)
            .filter(|w| !w.is_empty())
            .collect();
        let present = in_words.iter().filter(|w| out_words.contains(*w)).count();
        (present as f32) / (in_words.len() as f32) < 0.5
    };

    opener_injected || (over_expanded && low_overlap)
}

/// One line per dictation stating which cleanup layers actually ran (round
/// 20 observability): makes any "it did not clean up" report a log read
/// instead of a guess. `model` is the model-pass outcome for THIS dictation.
fn log_cleanup_summary(settings: &AppSettings, cfg: &crate::pipeline::StageConfig, model: &str) {
    let ctx = cfg
        .context
        .as_ref()
        .map(|c| {
            format!(
                "{:?}/{:?}({})",
                c.category,
                c.mode,
                if c.enabled { "on" } else { "off" }
            )
        })
        .unwrap_or_else(|| "none".to_string());
    info!(
        "cleanup summary: filler={:?}:{:?} mind_change={:?}:{:?} words={:?} phrases={:?} model={} ctx={}",
        settings.filler_engine,
        settings.filler_level,
        settings.mind_change_engine,
        settings.mind_change_level,
        settings.custom_words_level,
        settings.custom_phrases_level,
        model,
        ctx
    );
}

pub(crate) async fn process_transcription_output(
    app: &AppHandle,
    transcription: &str,
    post_process: bool,
    cfg: &crate::pipeline::StageConfig,
    plan: Option<&crate::pipeline::model_pass::ModelPlan>,
) -> ProcessedTranscription {
    let settings = get_settings(app);
    let mut final_text = transcription.to_string();
    let mut post_processed_text: Option<String> = None;
    let mut post_process_prompt: Option<String> = None;

    // Custom phrases are deterministic and authoritative. When the whole
    // dictation is just a trigger, the deterministic expansion already turned
    // final_text into the saved template; paste that template EXACTLY as the
    // user saved it (original newlines and punctuation), and never send it
    // through the cleanup LLM (which used to mangle or blank it). This runs
    // whether or not post-processing is on.
    if let Some(phrase) = settings
        .custom_phrases
        .iter()
        .find(|p| norm_loose(&final_text) == norm_loose(&p.write) && !p.write.trim().is_empty())
    {
        final_text = phrase.write.clone();
        post_processed_text = Some(final_text.clone());
        log_cleanup_summary(&settings, cfg, "phrase_template");
        return ProcessedTranscription {
            final_text,
            post_processed_text,
            post_process_prompt,
        };
    }

    if post_process {
        // The pass was requested (settings-level gate) but this particular
        // dictation composed no model jobs, e.g. context-only Model mode with
        // no captured foreground app: the deterministic text is final.
        let Some(plan) = plan else {
            debug!("Model pass skipped: no plan composed for this dictation");
            log_cleanup_summary(&settings, cfg, "skipped_no_plan");
            return ProcessedTranscription {
                final_text,
                post_processed_text,
                post_process_prompt,
            };
        };
        if !engine_ready_or_skip(app).await {
            // Bundled engine not ready, paste the raw text now rather than
            // ever blocking the user's dictation on a warming model.
            log_cleanup_summary(&settings, cfg, "skipped_engine_cold");
            return ProcessedTranscription {
                final_text,
                post_processed_text,
                post_process_prompt,
            };
        }
        // G2: reuse sentence cleanups computed while the user spoke; any
        // mismatch (or no active cleaner) falls back to one full cleanup call.
        // Timed (round 21): the summary line reports stitch-vs-full plus the
        // wall time, so "cleanup feels slow" is a log read - and a run of
        // `applied_full` lines means the during-speech work is being wasted.
        let cleanup_started = std::time::Instant::now();
        let cleaner = app
            .try_state::<CleanerSlot>()
            .and_then(|slot| slot.0.lock().unwrap().take());
        let processed = match &cleaner {
            Some(c) => c.stitch(&final_text, &settings, plan).await,
            None => None,
        };
        let used_stitch = processed.is_some();
        let processed = match processed {
            Some(stitched) => Some(stitched),
            None => crate::pipeline::model_pass::clean_text(app, plan, &final_text).await,
        };
        let cleanup_ms = cleanup_started.elapsed().as_millis();
        let path = if used_stitch { "stitch" } else { "full" };
        // Never let an empty (or over-collapsed) LLM reply blank the paste:
        // keep the deterministic final_text instead. This is what stops a
        // dictation from vanishing entirely.
        let processed = processed.filter(|out| {
            if out.trim().is_empty() {
                warn!("cleanup returned empty; keeping the deterministic text");
                false
            } else if over_collapsed(&settings, &final_text, out) {
                warn!(
                    "cleanup over-collapsed to a phrase template; keeping the deterministic text"
                );
                false
            } else if looks_conversational(&final_text, out) {
                warn!("cleanup answered the dictation instead of cleaning it; keeping the deterministic text");
                false
            } else {
                true
            }
        });
        if let Some(processed_text) = processed {
            // Same deterministic nets on the LLM's output: small models
            // occasionally drop the final period or a sentence-start capital.
            // Routed through the pipeline's stage-6 shaping so the context
            // rules (chat's dropped period, code's literalness) get the last
            // word over the model reply too.
            let shaped = crate::pipeline::shape_output(&processed_text, cfg);
            post_processed_text = Some(shaped.clone());
            final_text = shaped;
            // History records which prompt produced the cleanup (the exact
            // per-dictation composition).
            post_process_prompt = Some(plan.system_prompt.clone());
            log_cleanup_summary(&settings, cfg, &format!("applied_{path}({cleanup_ms}ms)"));
        } else {
            log_cleanup_summary(
                &settings,
                cfg,
                &format!("rejected_kept_deterministic_{path}({cleanup_ms}ms)"),
            );
        }
    } else {
        if final_text != transcription {
            post_processed_text = Some(final_text.clone());
        }
        log_cleanup_summary(&settings, cfg, "off");
    }

    ProcessedTranscription {
        final_text,
        post_processed_text,
        post_process_prompt,
    }
}

/// The one dictation hotkey runs the LLM pass only when some cleanup stage is
/// set to the Model engine. Unknown ids (tests, future bindings) keep their
/// action's static flag.
fn effective_post_process(binding_id: &str, static_flag: bool, settings: &AppSettings) -> bool {
    match binding_id {
        "transcribe" => settings.model_pass_needed(),
        _ => static_flag,
    }
}

/// Whether a starting dictation should warm the bundled engine NOW (round 2):
/// exactly when its model pass will actually run, i.e. the settings-level
/// gate is on AND the dictation composed a plan. Warming at start (instead of
/// first use at finalize) closes the cold-engine hole where an unloaded
/// engine produced ZERO live chunks and then skipped the cleanup outright;
/// with the warm-up racing the dictation, the first post-boot finalize
/// bounded-waits up to 10s in `engine_ready_or_skip` instead of skipping.
fn should_warm_engine(post_process: bool, snapshot: &crate::pipeline::DictationSnapshot) -> bool {
    post_process && snapshot.plan.is_some()
}

/// Styles that open the live transcript panel. Only BarLive: the textbox
/// styles stream into the target app itself (F3) and keep the compact status
/// pill for every state.
fn shows_live_panel(style: OverlayStyle) -> bool {
    matches!(style, OverlayStyle::BarLive)
}

/// G2: sentence-incremental cleanup. While the user speaks, completed
/// sentences from the live COMMITTED text run through the exact same LLM
/// pipeline the final call uses (same rendered prompt shape, which also keeps
/// the engine's prefix cache warm). At finalize, cleaned chunks whose source
/// text is a byte-prefix of the final LLM input are reused and only the
/// residual tail is cleaned, so finish latency stays near-constant in
/// dictation length. ANY mismatch falls back to the historical full-text
/// cleanup call: correctness never depends on the incremental path.
///
/// App-managed slot holding the active dictation's cleaner (one at a time).
pub struct CleanerSlot(pub std::sync::Mutex<Option<std::sync::Arc<LiveCleaner>>>);

struct CleanedChunk {
    /// This chunk's own (trimmed) source text, kept for cue retraction.
    source: String,
    /// Byte prefix of the observed filtered text up to this chunk's end. The
    /// binding test at finalize: the final LLM input must start with it.
    src_prefix: String,
    cleaned: String,
}

#[derive(Default)]
struct CleanerState {
    chunks: Vec<CleanedChunk>,
    busy: bool,
}

pub struct LiveCleaner {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    state: std::sync::Arc<std::sync::Mutex<CleanerState>>,
}

const CLEANER_TICK_MS: u64 = 500;
/// Complete sentences held back from incremental cleaning. Round 2 sets this
/// to 0: the newest complete sentence is cleaned the moment its terminator
/// arrives (holdback 1 shipped sentence k only when k+1 began, a full spoken
/// sentence of extra latency). A correction that then rewrites it lands via
/// the existing cue-glue: the joint re-clean re-emits the SAME chunk index
/// and the textbox injector repairs it in place.
const CLEANER_HOLDBACK_SENTENCES: usize = 0;

impl LiveCleaner {
    fn start(
        app: &AppHandle,
        snapshot: std::sync::Arc<crate::pipeline::DictationSnapshot>,
    ) -> std::sync::Arc<Self> {
        use std::sync::atomic::Ordering;
        let cleaner = std::sync::Arc::new(LiveCleaner {
            stop: Default::default(),
            state: Default::default(),
        });

        // Track the newest committed live text.
        let latest_committed = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let latest_for_listener = std::sync::Arc::clone(&latest_committed);
        let listener = {
            use tauri::Listener;
            app.listen_any("stream-text-event", move |event| {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(event.payload()) {
                    if let Some(c) = v.get("committed").and_then(|c| c.as_str()) {
                        *latest_for_listener.lock().unwrap() = c.to_string();
                    }
                }
            })
        };

        let ah = app.clone();
        let me = std::sync::Arc::clone(&cleaner);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(CLEANER_TICK_MS)).await;
                if me.stop.load(Ordering::Acquire) {
                    break;
                }
                let settings = get_settings(&ah);
                if !settings.model_pass_needed() {
                    continue;
                }
                // The plan composed at dictation start; the orchestrator only
                // starts a cleaner when one exists, so this is belt and
                // braces (and keeps every chunk on the ONE shared prompt).
                let Some(plan) = snapshot.plan.as_ref() else {
                    break;
                };
                // Engine not up yet: don't queue chunk calls that would each
                // wait out a connect timeout.
                if crate::managers::llm_engine::ENGINE_PORT
                    .load(std::sync::atomic::Ordering::Acquire)
                    == 0
                {
                    continue;
                }
                // Busy peek BEFORE the deterministic pass: at tick 500 with a
                // 1-3s model call in flight most ticks are busy, so don't pay
                // the det pass and sentence walk just to find that out under
                // the lock. This loop is the only busy setter, so the peek
                // cannot race a concurrent set.
                if me.state.lock().unwrap().busy {
                    continue;
                }

                // The exact transformation the final path applies to raw text
                // before its LLM call, so chunk sources bind byte-for-byte.
                // Uses the dictation's start-time snapshot (live variant):
                // every tick and the finalize path share ONE config.
                let raw = latest_committed.lock().unwrap().clone();
                if raw.trim().is_empty() {
                    continue;
                }
                let filtered = crate::pipeline::run_deterministic(&raw, &snapshot.cfg_live);
                let ranges = crate::audio_toolkit::complete_sentence_ranges(&filtered);
                if ranges.len() <= CLEANER_HOLDBACK_SENTENCES {
                    continue;
                }
                // A chunk must never END right after a cue-opening sentence:
                // "Scratch that." edits BOTH neighbors, and its replacement
                // arrives in the sentence after it. Walk the boundary back so
                // cue sentences always ship with their successor.
                let mut last_idx = ranges.len() - 1 - CLEANER_HOLDBACK_SENTENCES;
                loop {
                    let sent = &filtered[ranges[last_idx].clone()];
                    if !crate::audio_toolkit::starts_with_correction_cue(sent) {
                        break;
                    }
                    if last_idx == 0 {
                        break;
                    }
                    last_idx -= 1;
                }
                if crate::audio_toolkit::starts_with_correction_cue(
                    &filtered[ranges[last_idx].clone()],
                ) {
                    continue; // everything pending is cue-glued; the residual covers it
                }
                let eligible_end = ranges[last_idx].end;

                // Pick the next chunk under the lock; clean it outside the
                // lock (single call in flight).
                let (chunk, src_prefix) = {
                    let mut st = me.state.lock().unwrap();
                    let done = st.chunks.last().map_or(0, |c| c.src_prefix.len());
                    // The observed text must still extend what was cleaned;
                    // anything else means history was rewritten and the safe
                    // move is to stop accumulating (stitch will then bind
                    // whatever prefix still matches, or nothing).
                    if done > 0
                        && !st
                            .chunks
                            .last()
                            .is_some_and(|c| filtered.starts_with(c.src_prefix.as_str()))
                    {
                        continue;
                    }
                    if eligible_end <= done {
                        continue;
                    }
                    let chunk = filtered[done..eligible_end].trim().to_string();
                    if chunk.is_empty() {
                        continue;
                    }
                    // Leading correction cue: the new chunk may rewrite the
                    // previous sentence. Re-clean them TOGETHER; the joint
                    // result replaces the previous chunk's output.
                    let chunk = if crate::audio_toolkit::starts_with_correction_cue(&chunk) {
                        match st.chunks.pop() {
                            Some(prev) => format!("{} {}", prev.source, chunk),
                            None => chunk,
                        }
                    } else {
                        chunk
                    };
                    st.busy = true;
                    (chunk, filtered[..eligible_end].to_string())
                };

                let cleaned =
                    crate::pipeline::model_pass::clean_text_with_settings(&settings, plan, &chunk)
                        .await;
                let notify = {
                    let mut st = me.state.lock().unwrap();
                    st.busy = false;
                    cleaned.map(|cleaned| {
                        st.chunks.push(CleanedChunk {
                            source: chunk,
                            src_prefix: src_prefix.clone(),
                            cleaned: cleaned.clone(),
                        });
                        (st.chunks.len() - 1, cleaned, src_prefix)
                    })
                };
                // F3 (TextboxClean model mode, and Inline): mirror the stored
                // chunk into the active textbox injector. A repeated index
                // (cue-glue popped the previous chunk and re-cleaned jointly)
                // tells the injector to repair that chunk's injected text;
                // the src_prefix lets Inline bind the polish to the raw
                // underlined region it replaces.
                if let Some((index, cleaned, src_prefix)) = notify {
                    if let Some(slot) = ah.try_state::<crate::stream_inject::InjectorSlot>() {
                        let injector = slot.0.lock().unwrap().clone();
                        if let Some(injector) = injector {
                            injector.on_chunk(index, &cleaned, &src_prefix);
                        }
                    }
                }
                if me.stop.load(Ordering::Acquire) {
                    break;
                }
            }
            {
                use tauri::Listener;
                ah.unlisten(listener);
            }
        });
        cleaner
    }

    /// Stop ticking. The cleaner stays in its slot so the finalize path can
    /// stitch; the slot owner drops it afterwards.
    pub fn finish(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
    }

    /// Reuse every cleaned chunk that binds to `final_llm_input` and clean
    /// only the residual tail (under the same `plan` every chunk used).
    /// `None` = nothing usable (or the residual call failed); the caller then
    /// runs the historical full-text cleanup.
    async fn stitch(
        &self,
        final_llm_input: &str,
        settings: &AppSettings,
        plan: &crate::pipeline::model_pass::ModelPlan,
    ) -> Option<String> {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        // Let an in-flight chunk land (bounded wait); it extends the prefix.
        for _ in 0..60 {
            if !self.state.lock().unwrap().busy {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        let (mut cleaned_parts, consumed, last_source) = {
            let st = self.state.lock().unwrap();
            // Chunks form a prefix chain, so the LAST one that binds
            // validates every earlier one byte-for-byte.
            let idx = st
                .chunks
                .iter()
                .rposition(|c| final_llm_input.starts_with(c.src_prefix.as_str()))?;
            let parts: Vec<String> = st.chunks[..=idx]
                .iter()
                .map(|c| c.cleaned.clone())
                .collect();
            (
                parts,
                st.chunks[idx].src_prefix.len(),
                st.chunks[idx].source.clone(),
            )
        };

        let residual = final_llm_input[consumed..].trim().to_string();
        if !residual.is_empty() {
            // A residual opening with a correction cue may rewrite the last
            // stitched sentence: re-clean them together.
            let residual = if crate::audio_toolkit::starts_with_correction_cue(&residual)
                && !cleaned_parts.is_empty()
            {
                cleaned_parts.pop();
                format!("{} {}", last_source, residual)
            } else {
                residual
            };
            let cleaned_residual =
                crate::pipeline::model_pass::clean_text_with_settings(settings, plan, &residual)
                    .await?;
            cleaned_parts.push(cleaned_residual);
        }

        let mut out = String::new();
        for part in cleaned_parts {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(part);
        }
        if out.trim().is_empty() {
            return None;
        }
        info!(
            "incremental cleanup: reused {} pre-cleaned chars, final call covered {} chars",
            consumed,
            final_llm_input.len() - consumed
        );
        Some(out)
    }
}

impl ShortcutAction for TranscribeAction {
    fn start(&self, app: &AppHandle, binding_id: &str, _shortcut_str: &str) {
        let start_time = Instant::now();
        debug!("TranscribeAction::start called for binding: {}", binding_id);

        // Load model in the background
        let tm = app.state::<Arc<TranscriptionManager>>();
        let rm = app.state::<Arc<AudioRecordingManager>>();

        // Load ASR model and VAD model in parallel
        tm.initiate_model_load();
        let rm_clone = Arc::clone(&rm);
        std::thread::spawn(move || {
            if let Err(e) = rm_clone.preload_vad() {
                debug!("VAD pre-load failed: {}", e);
            }
        });

        let binding_id = binding_id.to_string();
        change_tray_icon(app, TrayIconState::Recording);

        // Microphone mode is a fixed default in v2 (on-demand).
        let settings = get_settings(app);
        let is_always_on = crate::defaults::ALWAYS_ON_MICROPHONE;

        let selected_model_info = app
            .state::<Arc<ModelManager>>()
            .get_model_info(crate::managers::model::FIXED_STT_MODEL_ID);

        // Use the app-facing model capability as the single pre-recording source
        // for live streaming decisions. Unknown support is represented as false
        // until the model registry is updated by discovery or runtime load.
        let model_supports_streaming = selected_model_info
            .as_ref()
            .map(|m| m.supports_streaming)
            .unwrap_or(false);
        let vad_policy = if !crate::defaults::VAD_ENABLED {
            VadPolicy::Disabled
        } else if model_supports_streaming {
            VadPolicy::Streaming
        } else {
            VadPolicy::Offline
        };
        // F4: a new dictation invalidates any post-paste watcher still
        // polling a previous dictation's field (this paste may rewrite the
        // very text being watched).
        crate::auto_learn::cancel_active_watch(app);
        // F1: capture the dictation context ONCE (frontmost app) and freeze
        // the stage settings into one snapshot. Every consumer of this
        // dictation (LiveCleaner ticks, stream finalize, batch fallback,
        // history's target app, post-LLM shaping) reads THIS snapshot, so a
        // mid-dictation settings edit or focus change cannot desync them.
        let snapshot = std::sync::Arc::new(crate::pipeline::DictationSnapshot::capture(&settings));
        if let Some(slot) = app.try_state::<crate::pipeline::DictationContextSlot>() {
            *slot.0.lock().unwrap() = Some(std::sync::Arc::clone(&snapshot));
        }

        // Round 2: a dictation that WILL run the model warms the engine the
        // moment it starts (idempotent; ensure_running no-ops when already
        // up). Before this, nothing started llama-server until the finalize
        // path touched it, so the first Model-mode dictation after boot got
        // zero live chunks and then skipped its cleanup.
        let wants_model = should_warm_engine(
            effective_post_process(&binding_id, self.post_process, &settings),
            &snapshot,
        );
        if wants_model {
            if let Some(engine) =
                app.try_state::<std::sync::Arc<crate::managers::llm_engine::LlmEngineManager>>()
            {
                engine.ensure_running();
            }
        }

        // G2: clean completed sentences while the user is still speaking so
        // the final LLM call only covers the tail. Only for dictations that
        // will run the LLM: the settings-level gate AND a composed plan (the
        // plan is None when e.g. only context is on Model and no foreground
        // app was captured, so there is nothing to ask the model). The slot
        // is cleared first so a stale cleaner from a cancelled run can never
        // bind to this dictation.
        let mut cleaner_running = false;
        if let Some(slot) = app.try_state::<CleanerSlot>() {
            let mut slot = slot.0.lock().unwrap();
            *slot = None;
            if wants_model {
                *slot = Some(LiveCleaner::start(app, std::sync::Arc::clone(&snapshot)));
                cleaner_running = true;
            }
        }

        // F3: textbox styles stream text into the frontmost app while the
        // user speaks. Preflight (secure event input, a captured home app,
        // a ready input system) can refuse; the dictation then degrades to
        // Bar behavior for this run: compact pill, no injector, no stream.
        // The slot is cleared first so a stale injector from a cancelled run
        // can never type into this dictation's target.
        let mut textbox_injecting = false;
        if let Some(slot) = app.try_state::<crate::stream_inject::InjectorSlot>() {
            let mut slot = slot.0.lock().unwrap();
            *slot = None;
            let mode = match settings.overlay_style {
                OverlayStyle::TextboxRaw => Some(crate::stream_inject::InjectMode::Raw),
                OverlayStyle::TextboxClean => Some(if cleaner_running {
                    crate::stream_inject::InjectMode::CleanModel
                } else {
                    crate::stream_inject::InjectMode::CleanDet
                }),
                // Inline streams the det-filtered raw text either way; the
                // LiveCleaner (running only with a plan) adds in-place polish.
                OverlayStyle::Inline => Some(crate::stream_inject::InjectMode::Inline),
                _ => None,
            };
            if let Some(mode) = mode {
                *slot = crate::stream_inject::StreamInjector::try_create(
                    app, mode, &snapshot, &settings,
                );
                textbox_injecting = slot.is_some();
            }
        }

        // Live text flows for the BarLive panel and for an active textbox
        // injector: natively-streaming models stream; batch-only models
        // pseudo-stream (periodic re-decode; partials are display/injection
        // only, the final batch decode stays authoritative). The worker
        // trusts the loaded session's caps for the native path, so a stale
        // registry flag can't silently kill streaming.
        let live_preview = shows_live_panel(settings.overlay_style) || textbox_injecting;
        if model_supports_streaming || live_preview {
            tm.start_stream(live_preview);
        }

        match settings.overlay_style {
            OverlayStyle::BarLive => utils::show_streaming_overlay(app),
            // F3: textbox styles keep the compact status pill; the live text
            // panel must never open (words appear in the target app itself).
            // A preflight-degraded run shows the same pill (Bar behavior).
            OverlayStyle::TextboxRaw
            | OverlayStyle::TextboxClean
            | OverlayStyle::Inline
            | OverlayStyle::Bar => show_recording_overlay(app),
            OverlayStyle::None => {} // show_overlay_state no-ops on None anyway
        }
        debug!("Microphone mode - always_on: {}", is_always_on);

        let mut recording_error: Option<String> = None;
        if is_always_on {
            // Always-on mode: Play audio feedback immediately, then apply mute after sound finishes
            debug!("Always-on mode: Playing audio feedback immediately");
            let rm_clone = Arc::clone(&rm);
            let app_clone = app.clone();
            // The blocking helper exits immediately if audio feedback is disabled,
            // so we can always reuse this thread to ensure mute happens right after playback.
            std::thread::spawn(move || {
                play_feedback_sound_blocking(&app_clone, SoundType::Start);
                rm_clone.apply_mute();
            });

            if let Err(e) = rm.try_start_recording(&binding_id, vad_policy) {
                debug!("Recording failed: {}", e);
                recording_error = Some(e);
            }
        } else {
            // On-demand mode: Start recording first, then play audio feedback, then apply mute
            // This allows the microphone to be activated before playing the sound
            debug!("On-demand mode: Starting recording first, then audio feedback");
            let recording_start_time = Instant::now();
            match rm.try_start_recording(&binding_id, vad_policy) {
                Ok(()) => {
                    debug!("Recording started in {:?}", recording_start_time.elapsed());
                    // Small delay to ensure microphone stream is active
                    let app_clone = app.clone();
                    let rm_clone = Arc::clone(&rm);
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        debug!("Handling delayed audio feedback/mute sequence");
                        // Helper handles disabled audio feedback by returning early, so we reuse it
                        // to keep mute sequencing consistent in every mode.
                        play_feedback_sound_blocking(&app_clone, SoundType::Start);
                        rm_clone.apply_mute();
                    });
                }
                Err(e) => {
                    debug!("Failed to start recording: {}", e);
                    recording_error = Some(e);
                }
            }
        }

        if recording_error.is_none() {
            // Dynamically register the cancel shortcut in a separate task to avoid deadlock
            shortcut::register_cancel_shortcut(app);
        } else {
            // Starting failed (for example due to blocked microphone permissions).
            // Revert UI state so we don't stay stuck in the recording overlay.
            tm.cancel_stream();
            utils::hide_recording_overlay(app);
            change_tray_icon(app, TrayIconState::Idle);
            if let Some(err) = recording_error {
                let error_type = if is_microphone_access_denied(&err) {
                    "microphone_permission_denied"
                } else if is_no_input_device_error(&err) {
                    "no_input_device"
                } else {
                    "unknown"
                };
                let _ = app.emit(
                    "recording-error",
                    RecordingErrorEvent {
                        error_type: error_type.to_string(),
                        detail: Some(err),
                    },
                );
            }
        }

        debug!(
            "TranscribeAction::start completed in {:?}",
            start_time.elapsed()
        );
    }

    fn stop(&self, app: &AppHandle, binding_id: &str, _shortcut_str: &str) {
        // Unregister the cancel shortcut when transcription stops
        shortcut::unregister_cancel_shortcut(app);

        let stop_time = Instant::now();
        debug!("TranscribeAction::stop called for binding: {}", binding_id);

        let ah = app.clone();
        let rm = Arc::clone(&app.state::<Arc<AudioRecordingManager>>());
        let tm = Arc::clone(&app.state::<Arc<TranscriptionManager>>());
        let hm = Arc::clone(&app.state::<Arc<HistoryManager>>());

        change_tray_icon(app, TrayIconState::Transcribing);
        // Stop should give immediate visual feedback. The BarLive panel keeps
        // its size but switches from listening to a working spinner while the
        // stream finalizes. Every other style (including the F3 textbox
        // styles, whose text lives in the target app) uses the compact
        // transcribing pill (None no-ops in show_*).
        let style = get_settings(app).overlay_style;
        match (shows_live_panel(style), tm.is_streaming()) {
            (true, true) => {
                tm.emit_stream_working(StreamWorkKind::Transcribing);
            }
            _ => show_transcribing_overlay(app),
        }

        // Unmute before playing audio feedback so the stop sound is audible
        rm.remove_mute();

        // Play audio feedback for recording stop
        play_feedback_sound(app, SoundType::Stop);

        if let Some(slot) = app.try_state::<CleanerSlot>() {
            // Stop ticking but leave the cleaner in place: the transcription
            // task below stitches from it, then drops it.
            if let Some(cleaner) = slot.0.lock().unwrap().as_ref() {
                cleaner.finish();
            }
        }
        let binding_id = binding_id.to_string(); // Clone binding_id for the async task
        let post_process = {
            let settings = get_settings(app);
            effective_post_process(&binding_id, self.post_process, &settings)
        };
        let cancel_generation = rm.cancel_generation();

        tauri::async_runtime::spawn(async move {
            let _guard = FinishGuard(ah.clone());
            debug!(
                "Starting async transcription task for binding: {}",
                binding_id
            );

            let stop_recording_time = Instant::now();
            if let Some(samples) = rm.stop_recording(&binding_id, cancel_generation) {
                debug!(
                    "Recording stopped and samples retrieved in {:?}, sample count: {}",
                    stop_recording_time.elapsed(),
                    samples.len()
                );

                if rm.was_cancelled_since(cancel_generation) {
                    debug!("Transcription operation cancelled after recording stop");
                    tm.cancel_stream();
                    utils::hide_recording_overlay(&ah);
                    change_tray_icon(&ah, TrayIconState::Idle);
                    return;
                }

                if samples.is_empty() {
                    debug!("Recording produced no audio samples; skipping persistence");
                    // Tear down any streaming worker so its channel doesn't leak
                    // and block the next start_stream.
                    tm.cancel_stream();
                    utils::hide_recording_overlay(&ah);
                    change_tray_icon(&ah, TrayIconState::Idle);
                } else {
                    // Save WAV concurrently with transcription
                    let sample_count = samples.len();
                    let file_name = format!("vaporly-{}.wav", chrono::Utc::now().timestamp());
                    let wav_path = hm.recordings_dir().join(&file_name);
                    let wav_path_for_verify = wav_path.clone();
                    let samples_for_wav = samples.clone();
                    let wav_handle = tauri::async_runtime::spawn_blocking(move || {
                        crate::audio_toolkit::save_wav_file(&wav_path, &samples_for_wav)
                    });

                    // Transcribe concurrently with WAV save. If a live stream was
                    // running, finalize it and use its text (all audio was already
                    // fed to the stream); otherwise batch-transcribe the samples.
                    let transcription_time = Instant::now();
                    let transcription_result = match tm.finalize_stream() {
                        // A finalized stream with usable text wins. An empty result
                        // (no active stream, produced nothing, or a finalize error
                        // after the engine was returned) falls back to a full batch
                        // transcription of the same audio. A finalize timeout is
                        // surfaced instead, the worker may still hold the engine,
                        // so a batch fallback would contend with it.
                        Ok(Some(text)) if !text.trim().is_empty() => Ok(text),
                        Ok(_) => tm.transcribe(samples),
                        Err(err) => Err(err),
                    };

                    // Await WAV save and verify
                    let wav_saved = match wav_handle.await {
                        Ok(Ok(())) => {
                            match crate::audio_toolkit::verify_wav_file(
                                &wav_path_for_verify,
                                sample_count,
                            ) {
                                Ok(()) => true,
                                Err(e) => {
                                    error!("WAV verification failed: {}", e);
                                    false
                                }
                            }
                        }
                        Ok(Err(e)) => {
                            error!("Failed to save WAV file: {}", e);
                            false
                        }
                        Err(e) => {
                            error!("WAV save task panicked: {}", e);
                            false
                        }
                    };

                    if rm.was_cancelled_since(cancel_generation) {
                        debug!("Transcription operation cancelled before output handling");
                        utils::hide_recording_overlay(&ah);
                        change_tray_icon(&ah, TrayIconState::Idle);
                        return;
                    }

                    match transcription_result {
                        Ok(transcription) => {
                            debug!(
                                "Transcription completed in {:?}: '{}'",
                                transcription_time.elapsed(),
                                transcription
                            );

                            // Vaporly: the dictation's context snapshot was captured
                            // ONCE at start (v1 re-captured near finalize, racing the
                            // user's focus). History's target app, the stage config,
                            // and the composed model plan all come from that same
                            // snapshot; when it is somehow gone (never for a real
                            // dictation), fall back fresh.
                            let snapshot = ah
                                .try_state::<crate::pipeline::DictationContextSlot>()
                                .and_then(|slot| slot.0.lock().unwrap().clone());
                            let target_app = snapshot
                                .as_ref()
                                .and_then(|s| s.ctx.as_ref())
                                .map(|ctx| ctx.app_name.clone());
                            let cfg_final = snapshot
                                .as_ref()
                                .map(|s| s.cfg_final.clone())
                                .unwrap_or_else(|| {
                                    crate::pipeline::StageConfig::from_settings(
                                        &get_settings(&ah),
                                        false,
                                        false,
                                        None,
                                    )
                                });
                            let plan = match &snapshot {
                                Some(s) => s.plan.clone(),
                                None => crate::pipeline::model_pass::build_model_plan(&cfg_final),
                            };
                            // The settings-level gate said a model pass MAY be
                            // needed; the composed plan is the per-dictation
                            // truth (None when nothing was composed, e.g.
                            // context-only Model mode with no captured app).
                            // No plan = deterministic text is final, so never
                            // show Polishing for it or record it as processed.
                            let post_process = post_process && plan.is_some();

                            if post_process {
                                // Textbox styles use the compact processing
                                // pill; only BarLive has a panel to keep.
                                if shows_live_panel(style) {
                                    tm.emit_stream_working(StreamWorkKind::Polishing);
                                } else {
                                    show_processing_overlay(&ah);
                                }
                            }

                            let processed = process_transcription_output(
                                &ah,
                                &transcription,
                                post_process,
                                &cfg_final,
                                plan.as_ref(),
                            )
                            .await;

                            if rm.was_cancelled_since(cancel_generation) {
                                debug!("Transcription operation cancelled before paste");
                                utils::hide_recording_overlay(&ah);
                                change_tray_icon(&ah, TrayIconState::Idle);
                                return;
                            }

                            // Save to history if WAV was saved
                            if wav_saved {
                                match hm.save_entry(
                                    file_name,
                                    transcription,
                                    post_process,
                                    processed.post_processed_text.clone(),
                                    processed.post_process_prompt.clone(),
                                    target_app.clone(),
                                ) {
                                    Err(err) => {
                                        error!("Failed to save history entry: {}", err);
                                    }
                                    Ok(_) => {
                                        // F4 RepeatedWords: count this dictation's
                                        // unknown words toward the repeat
                                        // threshold. spawn_blocking keeps the
                                        // SQLite work off the paste critical path.
                                        let settings = get_settings(&ah);
                                        if crate::auto_learn::repeated_words_enabled(
                                            settings.auto_learn_mode,
                                        ) && !processed.final_text.trim().is_empty()
                                        {
                                            let hm_observe = Arc::clone(&hm);
                                            let ah_observe = ah.clone();
                                            let text = processed.final_text.clone();
                                            let known = settings.custom_words.clone();
                                            tauri::async_runtime::spawn_blocking(move || {
                                                let now = chrono::Utc::now().timestamp();
                                                match hm_observe
                                                    .observe_dictation_words(&text, now, &known)
                                                {
                                                    Ok(learned) if !learned.is_empty() => {
                                                        crate::auto_learn::learn_words(
                                                            &ah_observe,
                                                            learned
                                                                .into_iter()
                                                                .map(|word| {
                                                                    crate::auto_learn::LearnedWord {
                                                                        word,
                                                                        source: crate::auto_learn::LearnSource::RepeatedWord,
                                                                    }
                                                                })
                                                                .collect(),
                                                        );
                                                    }
                                                    Ok(_) => {}
                                                    Err(e) => warn!(
                                                        "repeated-words observation failed: {}",
                                                        e
                                                    ),
                                                }
                                            });
                                        }
                                    }
                                }
                            }

                            // F3: an active textbox injector owns the output.
                            // Taken (not borrowed) so a concurrent cancel can
                            // never double-drive it; FinishGuard clears any
                            // leftover on the error paths.
                            let injector = ah
                                .try_state::<crate::stream_inject::InjectorSlot>()
                                .and_then(|slot| slot.0.lock().unwrap().take());

                            if processed.final_text.is_empty() {
                                if let Some(injector) = injector {
                                    // Nothing survived the pipeline: wipe the
                                    // words streamed into the target app.
                                    injector.cancel();
                                }
                                utils::hide_recording_overlay(&ah);
                                change_tray_icon(&ah, TrayIconState::Idle);
                            } else if let Some(injector) = injector {
                                if rm.was_cancelled_since(cancel_generation) {
                                    debug!("Transcription operation cancelled before finalize");
                                    injector.cancel();
                                    utils::hide_recording_overlay(&ah);
                                    change_tray_icon(&ah, TrayIconState::Idle);
                                    return;
                                }
                                let finalize_time = Instant::now();
                                let final_text = processed.final_text;
                                // F4: the post-paste watcher needs the exact
                                // delivered text after the injector consumes it.
                                let watch_text = final_text.clone();
                                let inj = std::sync::Arc::clone(&injector);
                                // The worker types/backspaces via the main
                                // thread; wait for its verdict off the async
                                // executor.
                                let outcome = tauri::async_runtime::spawn_blocking(move || {
                                    inj.finalize(&final_text)
                                })
                                .await
                                .unwrap_or_else(|e| {
                                    crate::stream_inject::FinalResult::Error(format!(
                                        "finalize task failed: {e}"
                                    ))
                                });
                                match outcome {
                                    crate::stream_inject::FinalResult::Inserted => {
                                        debug!(
                                            "Textbox injection finalized in {:?}",
                                            finalize_time.elapsed()
                                        );
                                        // Success flash owns its own hide.
                                        utils::flash_inserted_overlay(&ah);
                                        // F4 WatchPostPaste: observe the target
                                        // field for a manual correction (no-op
                                        // unless the mode selects it).
                                        crate::auto_learn::start_post_paste_watch(&ah, watch_text);
                                    }
                                    crate::stream_inject::FinalResult::SkippedFocusChanged => {
                                        warn!(
                                            "focus left the target app mid-dictation; streamed \
                                             text left as-is, final text not inserted (history \
                                             and keep-on-clipboard still apply)"
                                        );
                                        utils::flash_insert_error_overlay(&ah);
                                    }
                                    crate::stream_inject::FinalResult::Error(e) => {
                                        error!("Textbox injection failed: {}", e);
                                        let _ = ah.emit("paste-error", ());
                                        utils::flash_insert_error_overlay(&ah);
                                    }
                                }
                                change_tray_icon(&ah, TrayIconState::Idle);
                            } else {
                                let ah_clone = ah.clone();
                                let paste_time = Instant::now();
                                let final_text = processed.final_text;
                                let rm_for_paste = Arc::clone(&rm);
                                ah.run_on_main_thread(move || {
                                    if rm_for_paste.was_cancelled_since(cancel_generation) {
                                        debug!("Transcription operation cancelled before paste");
                                        utils::hide_recording_overlay(&ah_clone);
                                        change_tray_icon(&ah_clone, TrayIconState::Idle);
                                        return;
                                    }

                                    // F4: keep the delivered text for the
                                    // post-paste watcher (paste consumes it).
                                    let watch_text = final_text.clone();
                                    match utils::paste(final_text, ah_clone.clone()) {
                                        Ok(()) => {
                                            debug!(
                                                "Text pasted successfully in {:?}",
                                                paste_time.elapsed()
                                            );
                                            // Success flash owns its own hide.
                                            utils::flash_inserted_overlay(&ah_clone);
                                            // F4 WatchPostPaste: observe the
                                            // target field for a manual
                                            // correction (no-op unless the
                                            // mode selects it; spawns off the
                                            // paste path immediately).
                                            crate::auto_learn::start_post_paste_watch(
                                                &ah_clone, watch_text,
                                            );
                                        }
                                        Err(e) => {
                                            error!("Failed to paste transcription: {}", e);
                                            let _ = ah_clone.emit("paste-error", ());
                                            utils::flash_insert_error_overlay(&ah_clone);
                                        }
                                    }
                                    change_tray_icon(&ah_clone, TrayIconState::Idle);
                                })
                                .unwrap_or_else(|e| {
                                    error!("Failed to run paste on main thread: {:?}", e);
                                    utils::hide_recording_overlay(&ah);
                                    change_tray_icon(&ah, TrayIconState::Idle);
                                });
                            }
                        }
                        Err(err) => {
                            if rm.was_cancelled_since(cancel_generation) {
                                debug!(
                                    "Transcription operation cancelled after transcription error"
                                );
                                utils::hide_recording_overlay(&ah);
                                change_tray_icon(&ah, TrayIconState::Idle);
                                return;
                            }

                            error!("Transcription failed: {}", err);
                            // Surface the failure to the UI (toast). The full
                            // message is also in vaporly.log via the line above.
                            let _ = ah.emit("transcription-error", err.to_string());
                            // Save entry with empty text so user can retry
                            if wav_saved {
                                if let Err(save_err) = hm.save_entry(
                                    file_name,
                                    String::new(),
                                    post_process,
                                    None,
                                    None,
                                    None,
                                ) {
                                    error!("Failed to save failed history entry: {}", save_err);
                                }
                            }
                            utils::hide_recording_overlay(&ah);
                            change_tray_icon(&ah, TrayIconState::Idle);
                        }
                    }
                }
            } else {
                debug!("No samples retrieved from recording stop");
                // Tear down any streaming worker so its channel doesn't leak.
                tm.cancel_stream();
                utils::hide_recording_overlay(&ah);
                change_tray_icon(&ah, TrayIconState::Idle);
            }
        });

        debug!(
            "TranscribeAction::stop completed in {:?}",
            stop_time.elapsed()
        );
    }
}

// Cancel Action
struct CancelAction;

impl ShortcutAction for CancelAction {
    fn start(&self, app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        utils::cancel_current_operation(app);
    }

    fn stop(&self, _app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        // Nothing to do on stop for cancel
    }
}

// Whisper Mode toggle: flips the setting for the NEXT dictation (the gain
// params are read at recording start). Confirms audibly through the normal
// feedback path (respects the audio-feedback setting) and tells the settings
// UI to refresh.
struct WhisperToggleAction;

impl ShortcutAction for WhisperToggleAction {
    fn start(&self, app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        let mut settings = get_settings(app);
        settings.whisper_mode = !settings.whisper_mode;
        let now_on = settings.whisper_mode;
        crate::settings::write_settings(app, settings);
        play_feedback_sound(
            app,
            if now_on {
                SoundType::Start
            } else {
                SoundType::Stop
            },
        );
        let _ = app.emit("whisper-mode-changed", now_on);
        info!("Whisper Mode toggled {}", if now_on { "on" } else { "off" });
    }

    fn stop(&self, _app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        // Toggle acts on press only.
    }
}

// Static Action Map
pub static ACTION_MAP: Lazy<HashMap<String, Arc<dyn ShortcutAction>>> = Lazy::new(|| {
    let mut map = HashMap::new();
    map.insert(
        "transcribe".to_string(),
        Arc::new(TranscribeAction {
            post_process: false,
        }) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "cancel".to_string(),
        Arc::new(CancelAction) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "whisper_toggle".to_string(),
        Arc::new(WhisperToggleAction) as Arc<dyn ShortcutAction>,
    );
    map
});

#[cfg(test)]
mod tests {
    use super::{effective_post_process, should_warm_engine};

    /// Snapshot builder for the warm-up rows (no foreground capture: ctx is
    /// injected as None, exactly what a headless test environment yields).
    fn snapshot_for(settings: &crate::settings::AppSettings) -> crate::pipeline::DictationSnapshot {
        let cfg_live = crate::pipeline::StageConfig::from_settings(settings, true, false, None);
        let cfg_final = cfg_live.final_variant();
        let plan = crate::pipeline::model_pass::build_model_plan(&cfg_final);
        crate::pipeline::DictationSnapshot {
            ctx: None,
            cfg_live,
            cfg_final,
            plan,
        }
    }

    #[test]
    fn conversational_reply_to_a_command_is_rejected() {
        use super::looks_conversational;
        // Dictating a command-like phrase must NOT paste the model's answer.
        assert!(looks_conversational(
            "rewrite this",
            "Sure, please provide the text you would like me to rewrite."
        ));
        assert!(looks_conversational(
            "summarize the following",
            "Of course! Here is a summary once you share the content."
        ));
        // A genuine cleaned transcript is kept (echoes the input, no tell).
        assert!(!looks_conversational(
            "um so we should uh ship it tomorrow",
            "So we should ship it tomorrow."
        ));
        // A dictation that itself opens with a tell word is not flagged: the
        // input opens with it too, so the model injected nothing.
        assert!(!looks_conversational(
            "sure I will be there at noon",
            "Sure, I will be there at noon."
        ));
    }

    #[test]
    fn engine_warms_exactly_when_the_dictation_will_use_it() {
        use crate::settings::{ContextMode, StageEngine};

        // All-deterministic stages: no plan, never warm.
        let mut s = crate::settings::get_default_settings();
        s.filler_engine = StageEngine::Deterministic;
        s.mind_change_engine = StageEngine::Deterministic;
        s.context_awareness.mode = ContextMode::Deterministic;
        let det = snapshot_for(&s);
        assert!(!should_warm_engine(true, &det));
        assert!(!should_warm_engine(false, &det));

        // A Model stage composes a plan: warm exactly when the pass runs.
        let mut s = crate::settings::get_default_settings();
        s.mind_change_engine = StageEngine::Model;
        let model = snapshot_for(&s);
        assert!(should_warm_engine(true, &model));
        assert!(!should_warm_engine(false, &model), "gate off, no warm");

        // Context-only Model mode with NO captured app: the settings-level
        // gate says yes but THIS dictation composed nothing, so no warm.
        let mut s = crate::settings::get_default_settings();
        s.filler_engine = StageEngine::Deterministic;
        s.mind_change_engine = StageEngine::Deterministic;
        s.context_awareness.mode = ContextMode::Model;
        let ctxless = snapshot_for(&s);
        assert!(ctxless.plan.is_none());
        assert!(!should_warm_engine(true, &ctxless));
    }

    #[test]
    fn model_stage_routes_the_dictation_key() {
        use crate::settings::{FeatureLevel, StageEngine};
        let mut s = crate::settings::get_default_settings();

        // Round-2 defaults ship mind-change Light+Model: the dictation key
        // runs the pass out of the box.
        assert!(effective_post_process("transcribe", false, &s));

        // All stages Deterministic: the key never calls the LLM.
        s.mind_change_engine = StageEngine::Deterministic;
        assert!(!effective_post_process("transcribe", false, &s));

        // Any stage on Model (with a live level) turns the pass back on.
        s.filler_engine = StageEngine::Model;
        assert!(effective_post_process("transcribe", false, &s));

        // Model engine with the stage Off is still no pass.
        s.filler_level = FeatureLevel::Off;
        assert!(!effective_post_process("transcribe", false, &s));

        // Unknown ids keep the action's static flag.
        assert!(effective_post_process("some_future_binding", true, &s));
        assert!(!effective_post_process("some_future_binding", false, &s));
    }
}

#[cfg(test)]
mod over_collapse_tests {
    use super::over_collapsed;

    fn settings_with_template() -> crate::settings::AppSettings {
        let mut s = crate::settings::get_default_settings();
        s.custom_phrases = vec![crate::settings::CustomPhrase {
            say: "write my email format".to_string(),
            write: "Hi team,\n\nStatus below.\n\nThanks".to_string(),
        }];
        s
    }

    #[test]
    fn collapsed_reply_is_rejected() {
        let s = settings_with_template();
        let input = "The meeting went well and we shipped the feature on time. Also write my email format for the update.";
        let output = "Hi team,\n\nStatus below.\n\nThanks";
        assert!(over_collapsed(&s, input, output));
    }

    #[test]
    fn template_alone_is_legitimate() {
        let s = settings_with_template();
        // The whole dictation WAS the trigger: outputting only the template
        // is exactly right.
        assert!(!over_collapsed(
            &s,
            "write my email format",
            "Hi team,\n\nStatus below.\n\nThanks"
        ));
        // Normal cleanup output is never rejected.
        assert!(!over_collapsed(&s, "hello there", "Hello there."));
    }
}

#[cfg(test)]
mod verbatim_shortcircuit_tests {
    use super::norm_loose;

    fn settings_with_template() -> crate::settings::AppSettings {
        let mut s = crate::settings::get_default_settings();
        s.custom_phrases = vec![crate::settings::CustomPhrase {
            say: "introduce yourself".to_string(),
            write: "Hi team,\n\nI'm Po-Hsu.\n\nThanks".to_string(),
        }];
        s
    }

    // The predicate Fix B applies BEFORE the LLM: when the deterministic
    // expansion turned the whole transcript INTO a saved template, paste that
    // template verbatim and skip the model entirely.
    fn matches_phrase(s: &crate::settings::AppSettings, final_text: &str) -> bool {
        s.custom_phrases
            .iter()
            .any(|p| norm_loose(final_text) == norm_loose(&p.write) && !p.write.trim().is_empty())
    }

    #[test]
    fn norm_loose_ignores_case_space_punct() {
        assert_eq!(
            norm_loose("Hi team,\n\nStatus below.\n\nThanks"),
            norm_loose("hi team status below thanks")
        );
        assert_eq!(norm_loose("8:45 PM!"), "845pm");
        assert_eq!(norm_loose("   \n "), "");
    }

    #[test]
    fn deterministic_expansion_matches_the_phrase() {
        // apply_custom_phrases already replaced the whole transcript with the
        // template; even after shaping tweaks caps/whitespace, it still matches.
        let s = settings_with_template();
        assert!(matches_phrase(&s, "Hi team,\n\nI'm Po-Hsu.\n\nThanks"));
        assert!(matches_phrase(&s, "hi team i'm po-hsu thanks"));
    }

    #[test]
    fn unrelated_sentence_matches_nothing() {
        // Entry-70 regression: filler speech must NEVER resolve to a template.
        let s = settings_with_template();
        assert!(!matches_phrase(&s, "Buddy, could we just, you know?"));
    }

    #[test]
    fn blank_template_never_swallows_blank_text() {
        let mut s = settings_with_template();
        s.custom_phrases[0].write = "   ".to_string();
        assert!(!matches_phrase(&s, ""));
    }
}

#[cfg(test)]
mod live_cleaner_tests {
    use super::*;

    fn cleaner_with(chunks: Vec<CleanedChunk>) -> LiveCleaner {
        LiveCleaner {
            stop: Default::default(),
            state: std::sync::Arc::new(std::sync::Mutex::new(CleanerState {
                chunks,
                busy: false,
            })),
        }
    }

    fn chunk(source: &str, src_prefix: &str, cleaned: &str) -> CleanedChunk {
        CleanedChunk {
            source: source.to_string(),
            src_prefix: src_prefix.to_string(),
            cleaned: cleaned.to_string(),
        }
    }

    fn stitch_now(c: &LiveCleaner, final_input: &str) -> Option<String> {
        // Settings and the plan are only consulted for the residual LLM call;
        // these cases have no residual (or no binding), so stand-ins are fine.
        let settings = crate::settings::get_default_settings();
        let plan = crate::pipeline::model_pass::ModelPlan {
            system_prompt: String::new(),
            protected_writes: Vec::new(),
        };
        tauri::async_runtime::block_on(c.stitch(final_input, &settings, &plan))
    }

    #[test]
    fn stitch_reuses_fully_bound_chunks() {
        let c = cleaner_with(vec![
            chunk("first sentence.", "first sentence.", "First sentence."),
            chunk(
                "second one here.",
                "first sentence. second one here.",
                "Second one here.",
            ),
        ]);
        let out = stitch_now(&c, "first sentence. second one here.");
        assert_eq!(out.as_deref(), Some("First sentence. Second one here."));
    }

    #[test]
    fn stitch_rejects_rewritten_history() {
        let c = cleaner_with(vec![chunk(
            "first sentence.",
            "first sentence.",
            "First sentence.",
        )]);
        // The batch fallback produced different text: nothing binds.
        assert_eq!(stitch_now(&c, "a different transcription entirely."), None);
    }

    #[test]
    fn stitch_binds_longest_matching_prefix() {
        let c = cleaner_with(vec![
            chunk("alpha.", "alpha.", "Alpha."),
            chunk("beta.", "alpha. beta.", "Beta."),
            chunk("gamma.", "alpha. beta. gamma.", "Gamma."),
        ]);
        // Final input matches only through the second chunk (the third was
        // never spoken in the final take): reuse exactly those two.
        let out = stitch_now(&c, "alpha. beta.");
        assert_eq!(out.as_deref(), Some("Alpha. Beta."));
    }

    #[test]
    fn stitch_with_no_chunks_returns_none() {
        let c = cleaner_with(vec![]);
        assert_eq!(stitch_now(&c, "anything at all."), None);
    }

    #[test]
    fn stitch_reuses_a_cue_glued_joint_chunk_at_holdback_zero() {
        // Holdback 0 cleans "send it to john." the moment its terminator
        // arrives; the correction then completes as its own sentence, so the
        // tick pops the chunk and re-cleans the PAIR jointly (re-emitting
        // the same index toward the injector). What stitch sees afterwards
        // is one joint chunk whose src_prefix covers both sentences; it must
        // bind and be reused with zero residual.
        let c = cleaner_with(vec![chunk(
            "send it to john. no wait, joan.",
            "send it to john. no wait, joan.",
            "Send it to Joan.",
        )]);
        let out = stitch_now(&c, "send it to john. no wait, joan.");
        assert_eq!(out.as_deref(), Some("Send it to Joan."));
    }
}

/// Live E2E for the bundled-engine cleanup chain (moved out of the deleted
/// Command Mode test module; this test was never command-mode-specific).
#[cfg(test)]
mod engine_chain_tests {
    use super::*;

    /// Live E2E through the BUNDLED engine: spawns the staged llama-server
    /// directly on the Ollama-cached 7B (or VAPORLY_LLM_TEST_GGUF), publishes
    /// its port via ENGINE_PORT, and runs the real F2 cleanup chain (a
    /// composed `build_model_plan` prompt through `model_pass::clean_text`'s
    /// core) with the vaporly_engine provider, proving resolve_provider, the
    /// bearer-token injection, the single-flight gate, the reasoning arm, and
    /// the timeout path. Soft-skips when the payload or a model file is
    /// absent so CI stays green without them.
    #[test]
    fn vaporly_engine_cleanup_chain_live() {
        use std::time::Duration;

        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let payload = std::env::var("VAPORLY_LLAMA_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| repo_root.join("resources/llama"));
        let server = payload.join(if cfg!(windows) {
            "llama-server.exe"
        } else {
            "llama-server"
        });
        if !server.exists() {
            eprintln!(
                "SKIP: no staged llama-server at {server:?} (run scripts/ci/fetch-llama-server.sh)"
            );
            return;
        }

        // Model: explicit override, else the Ollama blob for qwen2.5:7b if present.
        let gguf = std::env::var("VAPORLY_LLM_TEST_GGUF")
            .map(std::path::PathBuf::from)
            .ok()
            .filter(|p| p.exists())
            .or_else(|| {
                let home = std::env::var("HOME").ok()?;
                let blobs = std::path::Path::new(&home).join(".ollama/models/blobs");
                // Largest blob is the 7B weights on this dev machine.
                std::fs::read_dir(blobs)
                    .ok()?
                    .flatten()
                    .filter_map(|e| {
                        let m = e.metadata().ok()?;
                        Some((m.len(), e.path()))
                    })
                    .max_by_key(|(len, _)| *len)
                    .filter(|(len, _)| *len > 1_000_000_000)
                    .map(|(_, p)| p)
            });
        let Some(gguf) = gguf else {
            eprintln!("SKIP: no test GGUF (set VAPORLY_LLM_TEST_GGUF)");
            return;
        };

        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map(|a| a.port())
            .expect("port");
        let mut child = std::process::Command::new(&server)
            .arg("-m")
            .arg(&gguf)
            .args(["--host", "127.0.0.1", "--port", &port.to_string()])
            .args(["-c", "4096", "--no-webui", "-ngl", "0"])
            // Same auth the production spawn uses: llm_client must inject
            // this token for the vaporly_engine provider or the call 401s.
            .args(["--api-key", "live-test-token"])
            .current_dir(&payload)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn llama-server");

        // Health poll (blocking, generous for a cold 7B mmap).
        let healthy = (0..180).any(|_| {
            std::thread::sleep(Duration::from_secs(1));
            std::net::TcpStream::connect_timeout(
                &format!("127.0.0.1:{port}").parse().unwrap(),
                Duration::from_millis(300),
            )
            .is_ok()
                && tauri::async_runtime::block_on(async {
                    reqwest::get(format!("http://127.0.0.1:{port}/health"))
                        .await
                        .map(|r| r.status().is_success())
                        .unwrap_or(false)
                })
        });
        if !healthy {
            let _ = child.kill();
            panic!("bundled llama-server never became healthy");
        }

        crate::managers::llm_engine::ENGINE_PORT.store(port, std::sync::atomic::Ordering::Release);
        *crate::managers::llm_engine::ENGINE_TOKEN
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = "live-test-token".to_string();

        // Compose the exact prompt a Model-mode dictation into TextEdit would
        // get (filler + mind-change jobs + the notes context hint) and run
        // the real chain, timing it (the greedy + capped path is bounded).
        let mut settings = crate::settings::get_default_settings();
        settings.filler_engine = crate::settings::StageEngine::Model;
        settings.mind_change_engine = crate::settings::StageEngine::Model;
        settings.context_awareness.mode = crate::settings::ContextMode::Model;
        // Fix A: a saved phrase makes the plan carry protected_writes, so the
        // in-sentence expansion below rides the wire as a [[Pn]] sentinel.
        settings.custom_phrases = vec![crate::settings::CustomPhrase {
            say: "my signature".to_string(),
            write: "Bnegbvjkbekjvbjk".to_string(),
        }];
        let ctx = crate::context::AppContext {
            app_name: "TextEdit".to_string(),
            bundle_id: "com.apple.textedit".to_string(),
            category: crate::pipeline::context_rules::CategoryId::Notes,
            category_desc: crate::context::category_description(
                crate::pipeline::context_rules::CategoryId::Notes,
            ),
        };
        let cfg = crate::pipeline::StageConfig::from_settings(&settings, false, false, Some(&ctx));
        let plan = crate::pipeline::model_pass::build_model_plan(&cfg)
            .expect("Model-mode stages compose a plan");

        let run = |raw: &str| -> Option<String> {
            let t = std::time::Instant::now();
            let out = tauri::async_runtime::block_on(
                crate::pipeline::model_pass::clean_text_with_settings(&settings, &plan, raw),
            );
            eprintln!("cleanup latency for {raw:?}: {:?}", t.elapsed());
            out
        };

        // Case 1: fillers stripped, wording preserved.
        let filler = run("um so basically i think we should uh move the meeting to thursday");
        // Case 2: a self-correction keeps the FINAL choice and drops the rest.
        let correction = run("send it to john no wait joan from accounting");
        // Case 3: an in-sentence custom-phrase expansion travels as a
        // protected sentinel and is restored verbatim (fix A).
        let phrase = run("um send the report to Bnegbvjkbekjvbjk by friday");

        // Case 4 (round-2 defaults): a TRUE-defaults plan (only the Light
        // mind-change job composes) must resolve what the deterministic pass
        // now deliberately leaves behind: the qa_correction fixture's
        // deterministic output keeps "no wait" because mind-change rides the
        // Model engine out of the box.
        let default_settings = crate::settings::get_default_settings();
        let default_cfg =
            crate::pipeline::StageConfig::from_settings(&default_settings, false, false, None);
        let default_plan = crate::pipeline::model_pass::build_model_plan(&default_cfg)
            .expect("round-2 defaults compose a plan");
        let t = std::time::Instant::now();
        let defaults_out =
            tauri::async_runtime::block_on(crate::pipeline::model_pass::clean_text_with_settings(
                &default_settings,
                &default_plan,
                "So let's meet at 8, no wait, 9. Send the invite to Joan.",
            ));
        eprintln!("true-defaults cleanup latency: {:?}", t.elapsed());

        // Restore globals and stop the server before asserting, so a failed
        // assert cannot leak the child process or the published port.
        crate::managers::llm_engine::ENGINE_PORT.store(0, std::sync::atomic::Ordering::Release);
        crate::managers::llm_engine::ENGINE_TOKEN
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        let _ = child.kill();
        let _ = child.wait();

        let filler = filler.expect("bundled engine returned no text (filler case)");
        eprintln!("filler cleanup output: {filler}");
        let fl = filler.to_lowercase();
        assert!(
            !fl.starts_with("um") && !fl.contains(" um ") && fl.contains("thursday"),
            "filler cleanup failed: {filler}"
        );

        let correction = correction.expect("bundled engine returned no text (correction case)");
        eprintln!("correction cleanup output: {correction}");
        let cl = correction.to_lowercase();
        assert!(
            cl.contains("joan"),
            "self-correction dropped the final choice: {correction}"
        );
        assert!(
            !cl.contains("john"),
            "self-correction kept the discarded choice: {correction}"
        );
        for preamble in ["sure", "here", "okay", "certainly"] {
            assert!(
                !cl.starts_with(preamble),
                "cleanup added a conversational preamble: {correction}"
            );
        }

        let phrase = phrase.expect("bundled engine returned no text (phrase case)");
        eprintln!("phrase protection output: {phrase}");
        assert!(
            phrase.contains("Bnegbvjkbekjvbjk"),
            "the protected write did not survive the model pass verbatim: {phrase}"
        );
        assert!(
            !phrase.contains("[[") && !phrase.contains("]]"),
            "a sentinel leaked into the decoded reply: {phrase}"
        );

        let defaults_out = defaults_out.expect("bundled engine returned no text (defaults case)");
        eprintln!("true-defaults cleanup output: {defaults_out}");
        let dl = defaults_out.to_lowercase();
        assert!(
            dl.contains("at 9") && dl.contains("joan"),
            "true-defaults pass failed to resolve the correction: {defaults_out}"
        );
        assert!(
            !dl.contains('8'),
            "true-defaults pass kept the retracted time: {defaults_out}"
        );
    }
}
