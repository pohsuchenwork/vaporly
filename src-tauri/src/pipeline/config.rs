//! Per-dictation stage configuration snapshot (F1).
//!
//! [`StageConfig`] is built ONCE when a dictation starts
//! (`TranscribeAction::start`) and reused by every consumer: the live-preview
//! deterministic pass, the LiveCleaner tick, the finalize/batch path, and
//! (F2) the model pass. Sharing one snapshot fixes a latent v1 race where
//! every LiveCleaner tick re-read settings, so a mid-dictation settings edit
//! could make the live and final passes disagree byte-for-byte.

use crate::context::AppContext;
use crate::pipeline::context_rules::{rules_for, CategoryId, CategoryRules};
use crate::settings::{
    AppSettings, ContextAwarenessSettings, ContextMode, FeatureLevel, StageEngine,
};

/// The dictation-target context after resolving settings against the captured
/// frontmost app. `rules` are the EFFECTIVE deterministic rules: identity when
/// the category is toggled off or the mode is Model-only (prompt hint only).
/// `app_name`/`category`/`mode`/`enabled` feed F2's prompt composer.
#[derive(Debug, Clone)]
pub struct ResolvedContext {
    pub app_name: String,
    pub category: CategoryId,
    pub mode: ContextMode,
    /// Whether this category's toggle is on. Off = identity rules AND no
    /// prompt hint (F2 reads this for the hint decision).
    pub enabled: bool,
    pub rules: CategoryRules,
}

/// Snapshot of every setting the deterministic pipeline reads, plus the
/// resolved dictation context and the live/final flag.
#[derive(Debug, Clone)]
pub struct StageConfig {
    pub custom_words: Vec<String>,
    pub custom_words_level: FeatureLevel,
    /// The words were already handed to the STT model as a decode prompt
    /// (whisper family), so the fuzzy correction stage must not double-apply.
    pub words_already_prompted: bool,
    /// Custom phrases as (say, write) pairs.
    pub custom_phrases: Vec<(String, String)>,
    /// Trigger-matching aggressiveness for those phrases (Off skips stage 2).
    pub custom_phrases_level: FeatureLevel,
    pub filler_level: FeatureLevel,
    pub filler_engine: StageEngine,
    pub mind_change_level: FeatureLevel,
    pub mind_change_engine: StageEngine,
    /// None when no foreground app could be captured (context stages no-op).
    pub context: Option<ResolvedContext>,
    /// Live tick (true) vs final text (false). Live protects the newest words
    /// (tail guard 2) and never applies final-only rules like chat's
    /// drop_final_terminal_period.
    pub live: bool,
}

impl StageConfig {
    /// Build the snapshot from settings + the app context captured at
    /// dictation start. Pure aside from reading its arguments.
    pub fn from_settings(
        settings: &AppSettings,
        live: bool,
        words_already_prompted: bool,
        ctx: Option<&AppContext>,
    ) -> Self {
        StageConfig {
            custom_words: settings.custom_words.clone(),
            custom_words_level: settings.custom_words_level,
            words_already_prompted,
            custom_phrases: settings
                .custom_phrases
                .iter()
                .map(|p| (p.say.clone(), p.write.clone()))
                .collect(),
            custom_phrases_level: settings.custom_phrases_level,
            filler_level: settings.filler_level,
            filler_engine: settings.filler_engine,
            mind_change_level: settings.mind_change_level,
            mind_change_engine: settings.mind_change_engine,
            context: ctx.map(|c| resolve_context(c, &settings.context_awareness)),
            live,
        }
    }

    /// The same snapshot with `live = false`, for the finalize/batch path.
    pub fn final_variant(&self) -> Self {
        let mut cfg = self.clone();
        cfg.live = false;
        cfg
    }

    /// Effective deterministic context rules (identity without context).
    pub fn rules(&self) -> CategoryRules {
        self.context.as_ref().map(|c| c.rules).unwrap_or_default()
    }
}

fn category_enabled(ca: &ContextAwarenessSettings, category: CategoryId) -> bool {
    match category {
        CategoryId::Email => ca.email,
        CategoryId::Chat => ca.chat,
        CategoryId::Code => ca.code,
        CategoryId::Browser => ca.browser,
        CategoryId::Notes => ca.notes,
        CategoryId::General => ca.general,
    }
}

fn resolve_context(ctx: &AppContext, ca: &ContextAwarenessSettings) -> ResolvedContext {
    let enabled = category_enabled(ca, ctx.category);
    // Deterministic rules apply for Deterministic and Both; Model mode is a
    // prompt hint only (F2), so its deterministic rules are identity.
    let rules_apply = enabled && matches!(ca.mode, ContextMode::Deterministic | ContextMode::Both);
    ResolvedContext {
        app_name: ctx.app_name.clone(),
        category: ctx.category,
        mode: ca.mode,
        enabled,
        rules: if rules_apply {
            rules_for(ctx.category)
        } else {
            CategoryRules::default()
        },
    }
}

/// Whether any cleanup stage needs the local LLM: gates the lazy engine and
/// the per-dictation model pass. All-deterministic settings never start
/// llama-server at all. (`AppSettings::model_pass_needed` delegates here so
/// existing callers stay stable.)
pub fn model_pass_needed(settings: &AppSettings) -> bool {
    (settings.filler_engine == StageEngine::Model && settings.filler_level != FeatureLevel::Off)
        || (settings.mind_change_engine == StageEngine::Model
            && settings.mind_change_level != FeatureLevel::Off)
        || (matches!(
            settings.context_awareness.mode,
            ContextMode::Model | ContextMode::Both
        ) && settings.context_awareness.any_enabled())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::get_default_settings;

    fn ctx(category: CategoryId) -> AppContext {
        AppContext {
            app_name: "TestApp".to_string(),
            bundle_id: "com.test.app".to_string(),
            category,
            category_desc: crate::context::category_description(category),
        }
    }

    #[test]
    fn snapshot_carries_settings_and_final_variant_only_flips_live() {
        let mut s = get_default_settings();
        s.custom_words = vec!["Kubernetes".to_string()];
        s.custom_phrases = vec![crate::settings::CustomPhrase {
            say: "btw".to_string(),
            write: "by the way".to_string(),
        }];
        let live = StageConfig::from_settings(&s, true, false, Some(&ctx(CategoryId::Chat)));
        assert!(live.live);
        assert_eq!(live.custom_words, vec!["Kubernetes"]);
        assert_eq!(
            live.custom_phrases,
            vec![("btw".to_string(), "by the way".to_string())]
        );
        let fin = live.final_variant();
        assert!(!fin.live);
        assert_eq!(fin.custom_words, live.custom_words);
        assert_eq!(fin.rules(), live.rules());
    }

    #[test]
    fn deterministic_mode_applies_category_rules() {
        let s = get_default_settings(); // mode: Deterministic, all categories on
        let cfg = StageConfig::from_settings(&s, false, false, Some(&ctx(CategoryId::Code)));
        assert!(cfg.rules().skip_itn);
        assert!(cfg.rules().skip_caps);
        let cfg = StageConfig::from_settings(&s, false, false, Some(&ctx(CategoryId::Chat)));
        // Round 23: chat keeps full punctuation; its distinguishing rule is
        // the paragraph structure.
        assert!(!cfg.rules().drop_final_terminal_period);
        assert_eq!(
            cfg.rules().structure,
            crate::pipeline::context_rules::Structure::Paragraphs { max_sentences: 2 }
        );
        // Notes carries only the round-21 paragraph structure; every
        // suppress flag stays default. General is fully default.
        let cfg = StageConfig::from_settings(&s, false, false, Some(&ctx(CategoryId::Notes)));
        assert_eq!(
            cfg.rules(),
            CategoryRules {
                structure: crate::pipeline::context_rules::Structure::Paragraphs {
                    max_sentences: 4
                },
                ..CategoryRules::default()
            }
        );
        let cfg = StageConfig::from_settings(&s, false, false, Some(&ctx(CategoryId::General)));
        assert_eq!(cfg.rules(), CategoryRules::default());
    }

    #[test]
    fn model_mode_is_prompt_hint_only() {
        let mut s = get_default_settings();
        s.context_awareness.mode = ContextMode::Model;
        let cfg = StageConfig::from_settings(&s, false, false, Some(&ctx(CategoryId::Code)));
        assert_eq!(cfg.rules(), CategoryRules::default());
        assert!(cfg.context.as_ref().unwrap().enabled);
        // Both = rules AND hint.
        s.context_awareness.mode = ContextMode::Both;
        let cfg = StageConfig::from_settings(&s, false, false, Some(&ctx(CategoryId::Code)));
        assert!(cfg.rules().skip_itn);
    }

    #[test]
    fn disabled_category_gets_identity_rules_and_no_hint() {
        let mut s = get_default_settings();
        s.context_awareness.chat = false;
        let cfg = StageConfig::from_settings(&s, false, false, Some(&ctx(CategoryId::Chat)));
        let resolved = cfg.context.as_ref().unwrap();
        assert!(!resolved.enabled);
        assert_eq!(resolved.rules, CategoryRules::default());
    }

    #[test]
    fn no_context_means_identity_rules() {
        let s = get_default_settings();
        let cfg = StageConfig::from_settings(&s, false, false, None);
        assert!(cfg.context.is_none());
        assert_eq!(cfg.rules(), CategoryRules::default());
    }

    /// Round 20 proof: turning context awareness OFF never turns cleanup off.
    /// With the owner's live config (filler Deterministic/Medium, mind-change
    /// Model/High, context mode Deterministic), disabling every context
    /// category leaves the model-pass gate ON, composes a byte-identical
    /// plan, and the deterministic pass output is unchanged for a
    /// default-rules category.
    #[test]
    fn disabling_context_never_disables_cleanup() {
        use crate::pipeline::model_pass::build_model_plan;
        use crate::pipeline::run_deterministic;

        let mut on = get_default_settings();
        on.filler_engine = StageEngine::Deterministic;
        on.filler_level = FeatureLevel::Medium;
        on.mind_change_engine = StageEngine::Model;
        on.mind_change_level = FeatureLevel::High;
        on.context_awareness.mode = ContextMode::Deterministic;

        let mut off = on.clone();
        off.context_awareness.email = false;
        off.context_awareness.chat = false;
        off.context_awareness.code = false;
        off.context_awareness.browser = false;
        off.context_awareness.notes = false;
        off.context_awareness.general = false;
        assert!(!off.context_awareness.any_enabled());

        // The LLM-pass gate ignores the context toggles in this config.
        assert!(model_pass_needed(&on));
        assert!(model_pass_needed(&off));

        // The composed plan (the mind-change job) is byte-identical.
        let notes = ctx(CategoryId::Notes);
        let cfg_on = StageConfig::from_settings(&on, false, false, Some(&notes));
        let cfg_off = StageConfig::from_settings(&off, false, false, Some(&notes));
        let plan_on = build_model_plan(&cfg_on).expect("plan with context on");
        let plan_off = build_model_plan(&cfg_off).expect("plan with context off");
        assert_eq!(plan_on.system_prompt, plan_off.system_prompt);

        // And the deterministic pass output does not change either.
        let text = "um so the meeting is at eight no wait nine";
        assert_eq!(
            run_deterministic(text, &cfg_on),
            run_deterministic(text, &cfg_off)
        );
    }

    #[test]
    fn model_pass_needed_free_fn_matches_method() {
        // True defaults want the pass (mind-change Light+Model, round 2).
        let mut s = get_default_settings();
        assert!(model_pass_needed(&s));
        assert_eq!(model_pass_needed(&s), s.model_pass_needed());
        // All-deterministic stages do not.
        s.mind_change_engine = StageEngine::Deterministic;
        assert!(!model_pass_needed(&s));
        assert_eq!(model_pass_needed(&s), s.model_pass_needed());
        s.filler_engine = StageEngine::Model;
        assert!(model_pass_needed(&s));
        assert_eq!(model_pass_needed(&s), s.model_pass_needed());
    }
}
