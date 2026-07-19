//! The deterministic text pipeline (F1).
//!
//! `run_deterministic` replaces v1's `post_process_transcription_text` (which
//! survives as a thin shim). Stage order, with the rationale locked in the
//! plan:
//!
//! 1. custom_words (skipped when Off or already whisper-prompted)
//! 2. custom_phrases (whole-dictation verbatim short-circuit stays in the
//!    orchestrator, actions.rs)
//! 3. filler_det, only when the stage engine is Deterministic and level > Off
//! 4. mind_change_det, same gating; live tail guard 2
//! 5. ITN, always on unless the context rules skip it (code category)
//! 6. shaping: sentence caps + terminal punctuation, each suppressible by
//!    context rules; chat's drop_final_terminal_period runs ONLY on final
//!    text (live prefix stability)
//!
//! Fillers run before mind-change so cue detection sees contiguous cues
//! ("no, um, wait" -> "no, wait"). Mind-change runs BEFORE ITN so both the
//! dropped span and its replacement are still in spoken form ("at eight, no
//! wait, nine" -> "at nine" -> "at 9" -> "At 9."). ITN after fillers is a
//! small recall win over v1 ("twenty um five" -> "25").
//!
//! Invariant every stage keeps: output on a completed-sentence prefix is
//! independent of the live flag and of later text (the one sanctioned
//! exception is a standalone retraction sentence, whose cross-sentence delete
//! is absorbed by the LiveCleaner cue-glue and stitch fallback).

pub mod config;
pub mod context_rules;
pub mod live;
pub mod model_pass;

pub use config::StageConfig;

use crate::audio_toolkit::{
    apply_custom_phrases, apply_custom_words, apply_itn, apply_mind_change,
    ensure_terminal_punctuation, filter_transcription_output, normalize_sentence_caps, FillerLevel,
    MindChangeLevel,
};
use crate::settings::{FeatureLevel, StageEngine};

/// Words the live tail guard protects during live ticks (ITN + mind-change).
const LIVE_TAIL_GUARD_WORDS: usize = 2;

fn filler_level(level: FeatureLevel) -> Option<FillerLevel> {
    match level {
        FeatureLevel::Off => None,
        FeatureLevel::Light => Some(FillerLevel::Light),
        FeatureLevel::Medium => Some(FillerLevel::Medium),
        FeatureLevel::High => Some(FillerLevel::High),
    }
}

fn mind_change_level(level: FeatureLevel) -> Option<MindChangeLevel> {
    match level {
        FeatureLevel::Off => None,
        FeatureLevel::Light => Some(MindChangeLevel::Light),
        FeatureLevel::Medium => Some(MindChangeLevel::Medium),
        FeatureLevel::High => Some(MindChangeLevel::High),
    }
}

/// The full deterministic pass over `text` under one dictation's snapshot.
pub fn run_deterministic(text: &str, cfg: &StageConfig) -> String {
    let tail_guard = if cfg.live { LIVE_TAIL_GUARD_WORDS } else { 0 };

    // 1. Custom words (fuzzy dictionary correction).
    let mut out = if cfg.custom_words_level != FeatureLevel::Off
        && !cfg.custom_words.is_empty()
        && !cfg.words_already_prompted
    {
        apply_custom_words(
            text,
            &cfg.custom_words,
            crate::defaults::word_threshold(cfg.custom_words_level),
        )
    } else {
        text.to_string()
    };

    // 2. Custom phrases (say -> write expansion). The level loosens how far a
    // spoken trigger may be misheard and still expand; Off skips the stage.
    if cfg.custom_phrases_level != FeatureLevel::Off && !cfg.custom_phrases.is_empty() {
        let pairs: Vec<(&str, &str)> = cfg
            .custom_phrases
            .iter()
            .map(|(say, write)| (say.as_str(), write.as_str()))
            .collect();
        out = apply_custom_phrases(
            &out,
            &pairs,
            crate::defaults::phrase_threshold(cfg.custom_phrases_level),
        );
    }

    // 3. Filler fix up (deterministic engine only; Model defers to F2's pass).
    if cfg.filler_engine == StageEngine::Deterministic {
        if let Some(level) = filler_level(cfg.filler_level) {
            out = filter_transcription_output(&out, "en", level);
        }
    }

    // 4. Mind-change resolution (deterministic engine only).
    if cfg.mind_change_engine == StageEngine::Deterministic {
        if let Some(level) = mind_change_level(cfg.mind_change_level) {
            out = apply_mind_change(&out, level, tail_guard);
        }
    }

    // 5. Inverse text normalization (always on; code category skips it).
    let rules = cfg.rules();
    if !rules.skip_itn {
        out = apply_itn(&out, tail_guard);
    }

    // 6. Shaping.
    shape_text(&out, cfg)
}

/// Stage-6 shaping only: sentence caps + terminal punctuation + the final
/// block structure (round 21) + chat's final-period drop, under the
/// snapshot's context rules. Exposed separately so the model pass (F2)
/// re-shapes LLM replies through the same rules; actions.rs already routes
/// its post-LLM shaping here.
pub fn shape_output(text: &str, cfg: &StageConfig) -> String {
    shape_text(text, cfg)
}

fn shape_text(text: &str, cfg: &StageConfig) -> String {
    let rules = cfg.rules();
    let mut out = text.to_string();
    if !rules.skip_caps {
        out = normalize_sentence_caps(&out);
    }
    if !rules.skip_terminal_punct {
        out = ensure_terminal_punctuation(&out);
    }
    // Block structure (round 21), FINAL text only: the live preview stays a
    // flat line while the pasted text gets the layout. Runs before chat's
    // period drop so sentence splitting still sees the punctuation.
    if !cfg.live {
        out = match rules.structure {
            context_rules::Structure::Flat => out,
            context_rules::Structure::Email => crate::audio_toolkit::apply_email_structure(&out),
            context_rules::Structure::Paragraphs { max_sentences } => {
                crate::audio_toolkit::apply_paragraphs(&out, usize::from(max_sentences))
            }
        };
    }
    // Final text only: a live prefix must never lose its period only to
    // regain it when more text arrives. Strips exactly ONE trailing '.',
    // never '?' or '!'.
    if rules.drop_final_terminal_period && !cfg.live {
        let trimmed = out.trim_end();
        if trimmed.ends_with('.') && !trimmed.ends_with("..") {
            out = trimmed[..trimmed.len() - 1].to_string();
        }
    }
    out
}

/// Everything the dictation captured at start and every consumer reuses:
/// the foreground app context and the live/final stage snapshots. Stored in
/// [`DictationContextSlot`] (managed like `CleanerSlot`): set at
/// `TranscribeAction::start`, read by the LiveCleaner tick and the
/// finalize/batch paths, cleared when the dictation's processing finishes.
pub struct DictationSnapshot {
    pub ctx: Option<crate::context::AppContext>,
    pub cfg_live: StageConfig,
    pub cfg_final: StageConfig,
    /// The composed model pass for this dictation (F2), built once here so
    /// LiveCleaner chunks and the finalize call share one byte-identical
    /// prompt. `None` = fully deterministic dictation: no LiveCleaner, no
    /// engine call, deterministic text pastes as final.
    pub plan: Option<model_pass::ModelPlan>,
}

impl DictationSnapshot {
    pub fn capture(settings: &crate::settings::AppSettings) -> Self {
        let ctx = crate::context::capture_foreground_app();
        let cfg_live = StageConfig::from_settings(settings, true, false, ctx.as_ref());
        let cfg_final = cfg_live.final_variant();
        let plan = model_pass::build_model_plan(&cfg_final);
        DictationSnapshot {
            cfg_final,
            cfg_live,
            ctx,
            plan,
        }
    }
}

/// App-managed slot holding the active dictation's snapshot (one at a time).
pub struct DictationContextSlot(pub std::sync::Mutex<Option<std::sync::Arc<DictationSnapshot>>>);

/// The final-text StageConfig for the CURRENT dictation, or a fresh
/// context-less snapshot when no dictation is active (headless CLI,
/// re-transcribe from History). `words_already_prompted` is decided at
/// transcription time (whisper-family prompt), so it overrides the snapshot.
pub fn current_final_cfg(
    app: &tauri::AppHandle,
    settings: &crate::settings::AppSettings,
    words_already_prompted: bool,
) -> StageConfig {
    use tauri::Manager;
    let snapshot = app
        .try_state::<DictationContextSlot>()
        .and_then(|slot| slot.0.lock().unwrap().clone());
    match snapshot {
        Some(snap) => {
            let mut cfg = snap.cfg_final.clone();
            cfg.words_already_prompted = words_already_prompted;
            cfg
        }
        None => StageConfig::from_settings(settings, false, words_already_prompted, None),
    }
}

#[cfg(test)]
mod pipeline_tests {
    use super::*;
    use crate::context::AppContext;
    use crate::pipeline::context_rules::CategoryId;
    use crate::settings::get_default_settings;

    /// Deterministic-behavior baseline: round-2 DEFAULTS put mind-change on
    /// the Model engine, so these rows pin it back to Deterministic Medium
    /// (the deterministic engine's behavior is what they test).
    fn cfg_with(category: Option<CategoryId>, filler: FeatureLevel, live: bool) -> StageConfig {
        let mut s = get_default_settings();
        s.filler_level = filler;
        s.mind_change_engine = crate::settings::StageEngine::Deterministic;
        s.mind_change_level = FeatureLevel::Medium;
        let ctx = category.map(|c| AppContext {
            app_name: "TestApp".to_string(),
            bundle_id: "com.test.app".to_string(),
            category: c,
            category_desc: crate::context::category_description(c),
        });
        StageConfig::from_settings(&s, live, false, ctx.as_ref())
    }

    #[test]
    fn e2e_email_final_gets_the_layout_and_live_stays_flat() {
        let text = "Hey Sarah, today was a great day. Thanks, John.";
        let fin = run_deterministic(
            text,
            &cfg_with(Some(CategoryId::Email), FeatureLevel::Off, false),
        );
        assert_eq!(fin, "Hey Sarah,\n\nToday was a great day.\n\nThanks,\nJohn");
        // Live preview never reflows.
        let live = run_deterministic(
            text,
            &cfg_with(Some(CategoryId::Email), FeatureLevel::Off, true),
        );
        assert!(!live.contains('\n'), "live must stay flat: {live:?}");
    }

    #[test]
    fn e2e_chat_splits_paragraphs_and_keeps_punctuation() {
        let text = "We shipped it today. The tests are green. Also the docs are updated.";
        let fin = run_deterministic(
            text,
            &cfg_with(Some(CategoryId::Chat), FeatureLevel::Off, false),
        );
        // Round 23: chat keeps its final period (the owner wants every
        // dictation to end with a terminal mark).
        assert_eq!(
            fin,
            "We shipped it today. The tests are green.\n\nAlso the docs are updated."
        );
        let live = run_deterministic(
            text,
            &cfg_with(Some(CategoryId::Chat), FeatureLevel::Off, true),
        );
        assert!(!live.contains('\n'), "live must stay flat: {live:?}");
    }

    #[test]
    fn e2e_notes_split_milder_and_general_stays_flat() {
        // No number words: ITN must not rewrite the fixture.
        let five = "The alpha part is done. The beta part is done. The gamma part is done. The delta part is done. The epsilon part is done.";
        let notes = run_deterministic(
            five,
            &cfg_with(Some(CategoryId::Notes), FeatureLevel::Off, false),
        );
        assert_eq!(
            notes,
            "The alpha part is done. The beta part is done. The gamma part is done. The delta part is done.\n\nThe epsilon part is done."
        );
        let general = run_deterministic(
            five,
            &cfg_with(Some(CategoryId::General), FeatureLevel::Off, false),
        );
        assert!(!general.contains('\n'), "general must stay flat");
        // No context at all = flat too.
        let none = run_deterministic(five, &cfg_with(None, FeatureLevel::Off, false));
        assert!(!none.contains('\n'), "no context must stay flat");
    }

    #[test]
    fn e2e_category_toggle_off_means_no_structure() {
        let mut s = get_default_settings();
        s.filler_level = FeatureLevel::Off;
        s.mind_change_engine = crate::settings::StageEngine::Deterministic;
        s.context_awareness.email = false;
        let ctx = AppContext {
            app_name: "Mail".to_string(),
            bundle_id: "com.apple.mail".to_string(),
            category: CategoryId::Email,
            category_desc: crate::context::category_description(CategoryId::Email),
        };
        let cfg = StageConfig::from_settings(&s, false, false, Some(&ctx));
        let out = run_deterministic("Hey Sarah, today was a great day. Thanks, John.", &cfg);
        assert!(
            !out.contains('\n'),
            "disabled category must stay flat: {out:?}"
        );
    }

    #[test]
    fn shape_output_structures_a_model_reply_for_email() {
        let cfg = cfg_with(Some(CategoryId::Email), FeatureLevel::Off, false);
        let out = shape_output("Hey Sarah, today was a great day. Thanks, John.", &cfg);
        assert_eq!(out, "Hey Sarah,\n\nToday was a great day.\n\nThanks,\nJohn");
    }

    #[test]
    fn phrase_threshold_ladder_loosens_trigger_matching() {
        use crate::defaults::phrase_threshold;
        let pairs: Vec<(&str, &str)> = vec![("insert my email", "user@example.com")];
        // "emale" distorts the trigger by ~0.15; "insurt me emeel" by ~0.31.
        let near = "please insert my emale now";
        let far = "please insurt me emeel now";
        let hit = |text: &str, level: FeatureLevel| {
            apply_custom_phrases(text, &pairs, phrase_threshold(level)).contains("user@example.com")
        };
        assert!(!hit(near, FeatureLevel::Light)); // 0.12 rejects the mishearing
        assert!(hit(near, FeatureLevel::Medium)); // 0.25 = the old fixed behavior
        assert!(!hit(far, FeatureLevel::Medium));
        assert!(hit(far, FeatureLevel::High)); // 0.35 accepts the looser one
                                               // Single-word triggers stay exact at every level (the precision rule).
        let single: Vec<(&str, &str)> = vec![("brb", "be right back")];
        assert!(
            !apply_custom_phrases("brc then", &single, phrase_threshold(FeatureLevel::High))
                .contains("be right back")
        );
    }

    #[test]
    fn phrases_level_off_skips_the_stage() {
        let mut s = get_default_settings();
        s.custom_phrases = vec![crate::settings::CustomPhrase {
            say: "insert my email".to_string(),
            write: "user@example.com".to_string(),
        }];
        s.custom_phrases_level = FeatureLevel::Off;
        s.mind_change_engine = crate::settings::StageEngine::Deterministic;
        let cfg = StageConfig::from_settings(&s, false, false, None);
        let out = run_deterministic("insert my email", &cfg);
        assert!(!out.contains("user@example.com"));
    }

    // ---- The plan's E2E rows. At High the filler stage removes the
    // sentence-initial "So um," cluster, reproducing the plan's outputs
    // exactly; the Medium row keeps "So" by the byte-identity guarantee on
    // Medium filtering.

    #[test]
    fn e2e_deterministic_medium() {
        let cfg = cfg_with(None, FeatureLevel::Medium, false);
        assert_eq!(
            run_deterministic("So um, at eight, no wait, nine", &cfg),
            "So at 9."
        );
    }

    #[test]
    fn e2e_true_defaults_leave_the_correction_for_the_model() {
        // Round-2 out-of-the-box defaults: mind-change rides the MODEL
        // engine, so the deterministic pass must NOT resolve the retraction;
        // it fixes fillers, numbers, caps, and punctuation and hands "no
        // wait" to the LLM downstream.
        let s = get_default_settings();
        let cfg = StageConfig::from_settings(&s, false, false, None);
        assert_eq!(
            run_deterministic("So um, at eight, no wait, nine", &cfg),
            "So at 8, no wait, 9."
        );
    }

    #[test]
    fn e2e_high_filler_general() {
        let cfg = cfg_with(Some(CategoryId::General), FeatureLevel::High, false);
        assert_eq!(
            run_deterministic("So um, at eight, no wait, nine", &cfg),
            "At 9."
        );
    }

    #[test]
    fn e2e_chat_keeps_full_punctuation() {
        // Round 23: chat no longer drops or skips the final terminal mark.
        let cfg = cfg_with(Some(CategoryId::Chat), FeatureLevel::High, false);
        assert_eq!(
            run_deterministic("So um, at eight, no wait, nine", &cfg),
            "At 9."
        );
        assert_eq!(
            run_deterministic("are we on for nine?", &cfg),
            "Are we on for 9?"
        );
        // Live text gets the provisional period too (prefix stability).
        let live = cfg_with(Some(CategoryId::Chat), FeatureLevel::High, true);
        assert_eq!(
            run_deterministic("So um, at eight, no wait, nine works. More", &live),
            "At 9 works. More."
        );
    }

    #[test]
    fn e2e_code_is_literal_but_still_corrected() {
        // Filler and mind-change still run (they fix STT errors), but no
        // ITN, no caps, no terminal punctuation.
        let cfg = cfg_with(Some(CategoryId::Code), FeatureLevel::High, false);
        assert_eq!(
            run_deterministic("So um, at eight, no wait, nine", &cfg),
            "at nine"
        );
    }

    #[test]
    fn stage_order_mind_change_runs_before_itn() {
        // Regression pin: if ITN ran first, "eight" would already be "8" and
        // the WHOLE point is moot; worse, "nine" alone converts and the cue
        // span would misalign. The pipeline output proves spoken-form
        // resolution then conversion.
        let cfg = cfg_with(None, FeatureLevel::Medium, false);
        assert_eq!(run_deterministic("at eight, no wait, nine", &cfg), "At 9.");
        // And fillers run before mind-change: the hesitation inside the cue
        // must not break cue detection.
        assert_eq!(
            run_deterministic("at eight, no, um, wait, nine", &cfg),
            "At 9."
        );
    }

    #[test]
    fn filler_off_or_model_engine_keeps_fillers_but_still_shapes() {
        let mut s = get_default_settings();
        s.filler_level = FeatureLevel::Off;
        let cfg = StageConfig::from_settings(&s, false, false, None);
        assert_eq!(run_deterministic("um hello there", &cfg), "Um hello there.");

        let mut s = get_default_settings();
        s.filler_engine = crate::settings::StageEngine::Model;
        let cfg = StageConfig::from_settings(&s, false, false, None);
        assert_eq!(run_deterministic("um hello there", &cfg), "Um hello there.");
    }

    #[test]
    fn mind_change_gating_matches_filler_gating() {
        let mut s = get_default_settings();
        s.mind_change_level = FeatureLevel::Off;
        let cfg = StageConfig::from_settings(&s, false, false, None);
        assert_eq!(
            run_deterministic("at eight, no wait, nine", &cfg),
            "At 8, no wait, 9."
        );
        let mut s = get_default_settings();
        s.mind_change_engine = crate::settings::StageEngine::Model;
        let cfg = StageConfig::from_settings(&s, false, false, None);
        assert_eq!(
            run_deterministic("at eight, no wait, nine", &cfg),
            "At 8, no wait, 9."
        );
    }

    #[test]
    fn shape_output_applies_context_rules_to_model_replies() {
        // Round 23: chat keeps (and gains, when missing) the final period.
        let chat = cfg_with(Some(CategoryId::Chat), FeatureLevel::Medium, false);
        assert_eq!(shape_output("sounds good.", &chat), "Sounds good.");
        assert_eq!(shape_output("sounds good", &chat), "Sounds good.");
        let code = cfg_with(Some(CategoryId::Code), FeatureLevel::Medium, false);
        assert_eq!(shape_output("git status", &code), "git status");
        let notes = cfg_with(Some(CategoryId::Notes), FeatureLevel::Medium, false);
        assert_eq!(shape_output("sounds good", &notes), "Sounds good.");
    }

    // ---- Live/final prefix binding property (the LiveCleaner contract) ----

    #[test]
    fn live_prefix_of_completed_sentences_is_stable_as_text_grows() {
        // Feed a growing dictation word by word through the LIVE config,
        // capturing what LiveCleaner would capture: the region of completed
        // sentences minus the holdback becomes a chunk src_prefix. The
        // binding contract is byte-prefix stability: every later live output
        // (and the final-variant output) must START WITH every captured
        // region. (The region's sentence COUNT may flicker when the growing
        // tail's provisional period reads as an abbreviation, e.g. "After
        // that I." That never breaks the byte-prefix binding.)
        let full = "So um, at eight, no wait, nine works for me. Then we can, uh, review the twenty five files. After that I will email John about the plan. Sounds good to me";
        let cfg_live = cfg_with(None, FeatureLevel::Medium, true);
        let cfg_final = cfg_live.final_variant();

        let words: Vec<&str> = full.split_whitespace().collect();
        let mut stable_prefix = String::new();
        for n in 1..=words.len() {
            let visible = words[..n].join(" ");
            let filtered = run_deterministic(&visible, &cfg_live);
            assert!(
                filtered.starts_with(&stable_prefix),
                "captured prefix no longer binds at word {n}:\n prefix: {stable_prefix:?}\n output: {filtered:?}"
            );
            let ranges = crate::audio_toolkit::complete_sentence_ranges(&filtered);
            if ranges.len() <= 1 {
                continue; // holdback: the newest sentence may still rewrite
            }
            let eligible_end = ranges[ranges.len() - 2].end;
            if eligible_end > stable_prefix.len() {
                stable_prefix = filtered[..eligible_end].to_string();
            }
        }
        assert!(
            stable_prefix.starts_with("So at 9 works for me."),
            "the series never captured the corrected first sentence: {stable_prefix:?}"
        );
        let final_out = run_deterministic(full, &cfg_final);
        assert!(
            final_out.starts_with(&stable_prefix),
            "final output does not extend the live stable region:\n stable: {stable_prefix:?}\n final: {final_out:?}"
        );
    }

    #[test]
    fn completed_sentence_prefix_is_independent_of_live_flag() {
        // run_deterministic on a completed-sentence prefix gives the same
        // bytes under live and final configs once the tail guard has words
        // beyond it (holdback in practice): the invariant the plan states.
        let prefix = "At eight, no wait, nine works for me. Then we review the plan.";
        let cfg_live = cfg_with(None, FeatureLevel::Medium, true);
        let cfg_final = cfg_live.final_variant();
        let live_out = run_deterministic(&format!("{prefix} And one more thing"), &cfg_live);
        let final_out = run_deterministic(prefix, &cfg_final);
        assert!(
            live_out.starts_with(&final_out),
            "live: {live_out:?} vs final: {final_out:?}"
        );
    }
}
