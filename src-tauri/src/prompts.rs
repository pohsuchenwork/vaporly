//! Internal prompt constants (v2).
//!
//! v1 stored editable prompt templates in settings; v2 owns them as code. The
//! composer that assembles the per-dictation system prompt lives in
//! `pipeline::model_pass` (`build_model_plan`); this module holds the prompt
//! text that is carried over from v1 verbatim. The rest of the old one-piece
//! cleanup prompt (header, grammar job, tail) was replaced by the per-stage
//! block composition in F2.
//!
//! Hard rules carried from v1 (do not relax):
//! - NEVER include a custom-phrases block (`${custom_phrases}`/`${snippets}`):
//!   small models hallucinated template insertions and returned empties that
//!   blanked pastes. Phrases are deterministic-only.
//! - Keep prompts SHORT and trust the deterministic pass (numbers, dictionary
//!   words, caps, punctuation are already handled before the model sees text).
//! - No em or en dashes in prompt text.

/// The v1 "Smart Formatting" self-corrections job, verbatim: the John->Joan
/// and 5-no-6 worked examples, the chain/rightmost rule, and the scratch-that
/// sentence deletion. The F2 composer uses it as the Medium mind-change job
/// (and as the base of the High job).
pub const MIND_CHANGE_MEDIUM_JOB: &str = "Self-corrections: when the speaker changed their mind, keep only the final version and delete every earlier attempt and its cue (wait, no, no wait, actually, I mean, sorry, scratch that, make that, or rather). Corrections chain and the rightmost choice always wins. \"Scratch that\" with no replacement deletes the previous sentence.\nExamples:\n\"send it to John, no wait, Joan\" -> \"send it to Joan\"\n\"we need 5, no, 6 copies\" -> \"we need 6 copies\"";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn medium_job_keeps_v1_rules_and_examples() {
        assert!(MIND_CHANGE_MEDIUM_JOB.contains("rightmost choice always wins"));
        assert!(MIND_CHANGE_MEDIUM_JOB.contains("deletes the previous sentence"));
        assert!(MIND_CHANGE_MEDIUM_JOB.contains("\"send it to John, no wait, Joan\""));
        assert!(MIND_CHANGE_MEDIUM_JOB.contains("\"we need 6 copies\""));
    }

    #[test]
    fn medium_job_never_mentions_phrases_or_dashes() {
        // v1 hallucination lesson + the project dash ban.
        assert!(!MIND_CHANGE_MEDIUM_JOB.contains("${"));
        assert!(!MIND_CHANGE_MEDIUM_JOB.contains('\u{2014}'));
        assert!(!MIND_CHANGE_MEDIUM_JOB.contains('\u{2013}'));
    }
}
