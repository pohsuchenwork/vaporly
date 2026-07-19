//! Deterministic context-awareness rules (F1, structure added in round 21).
//!
//! `context::categorize` resolves the frontmost app to a [`CategoryId`]; this
//! module says what the deterministic pipeline DOES with that category. The
//! suppress rules turn always-on polish stages off (ITN, caps, terminal
//! punctuation); `structure` is the one ADDITIVE rule (round 21): it reshapes
//! the FINAL text into blocks. A category with default rules is still
//! byte-identical to no context at all, and structure never runs on live
//! text.
//!
//! Per-category decisions (locked with the owner):
//! - Chat: full punctuation (round 23 reversed the old "messaging style"
//!   period-drop: the owner wants text to always end with a terminal mark),
//!   and the text splits into short paragraphs (blank lines) at discourse
//!   cues or every 2 sentences.
//! - Code editor or terminal: literal text. No ITN, no caps, no terminal
//!   punctuation, no structure - the ONLY punctuation-free category (a
//!   period appended to a shell command is destructive). Custom
//!   words/phrases, filler fix up, and the mind-change pass still run: they
//!   fix STT errors, not formatting.
//! - Email: greeting / body / sign-off blocks.
//! - Notes, Browser: mild paragraphs (every 4 sentences or a discourse cue).
//! - General (and any category toggled off): one flat block, unchanged.

use serde::{Deserialize, Serialize};

/// Typed category of the app a dictation targets. The prompt-facing
/// description string lives in `context::category_description`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CategoryId {
    Email,
    Chat,
    Code,
    Browser,
    Notes,
    General,
}

/// How the FINAL text is arranged into blocks (round 21). Applied only when
/// `live == false`, after caps/punctuation and before chat's dropped period,
/// so the live preview stays flat while the pasted text gets the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Structure {
    /// One flat block (the pre-round-21 behavior everywhere).
    #[default]
    Flat,
    /// Greeting line, blank line, body, blank line, sign-off + name line.
    Email,
    /// Paragraphs separated by blank lines: a new paragraph starts at a
    /// discourse cue ("also", "anyway", "by the way", ...) or after
    /// `max_sentences` sentences.
    Paragraphs { max_sentences: u8 },
}

/// What the deterministic pass skips for a category, plus the one additive
/// `structure` rule. `Default` = do everything, one flat block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CategoryRules {
    /// Skip inverse text normalization (spoken numbers stay spoken).
    pub skip_itn: bool,
    /// Skip sentence-start capitalization.
    pub skip_caps: bool,
    /// Skip appending terminal punctuation.
    pub skip_terminal_punct: bool,
    /// Drop ONE trailing '.' from the final sentence (never '?' or '!').
    /// Applied only on final text (`live == false`): a live prefix must not
    /// lose its period only to regain it when more text arrives.
    pub drop_final_terminal_period: bool,
    /// Final-text block layout (round 21). The owner's tuning table lives in
    /// `rules_for`; tweaks are one-line edits there.
    pub structure: Structure,
}

/// The deterministic rules for a category.
pub fn rules_for(category: CategoryId) -> CategoryRules {
    match category {
        CategoryId::Chat => CategoryRules {
            structure: Structure::Paragraphs { max_sentences: 2 },
            ..CategoryRules::default()
        },
        CategoryId::Code => CategoryRules {
            skip_itn: true,
            skip_caps: true,
            skip_terminal_punct: true,
            ..CategoryRules::default()
        },
        CategoryId::Email => CategoryRules {
            structure: Structure::Email,
            ..CategoryRules::default()
        },
        CategoryId::Notes | CategoryId::Browser => CategoryRules {
            structure: Structure::Paragraphs { max_sentences: 4 },
            ..CategoryRules::default()
        },
        CategoryId::General => CategoryRules::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_keeps_full_punctuation_and_splits_paragraphs() {
        // Round 23: the owner wants every dictation to end with a terminal
        // mark, so chat no longer skips or drops punctuation.
        let r = rules_for(CategoryId::Chat);
        assert!(!r.skip_itn);
        assert!(!r.skip_caps);
        assert!(!r.skip_terminal_punct);
        assert!(!r.drop_final_terminal_period);
        assert_eq!(r.structure, Structure::Paragraphs { max_sentences: 2 });
    }

    #[test]
    fn code_is_fully_literal() {
        let r = rules_for(CategoryId::Code);
        assert!(r.skip_itn);
        assert!(r.skip_caps);
        assert!(r.skip_terminal_punct);
        assert!(!r.drop_final_terminal_period);
        assert_eq!(r.structure, Structure::Flat);
    }

    #[test]
    fn structure_table_matches_the_owner_decisions() {
        // The round-21 tuning table: email layout for Email, short paragraphs
        // for Chat, milder paragraphs for Notes + Browser, flat for General.
        assert_eq!(rules_for(CategoryId::Email).structure, Structure::Email);
        assert_eq!(
            rules_for(CategoryId::Notes).structure,
            Structure::Paragraphs { max_sentences: 4 }
        );
        assert_eq!(
            rules_for(CategoryId::Browser).structure,
            Structure::Paragraphs { max_sentences: 4 }
        );
        assert_eq!(rules_for(CategoryId::General), CategoryRules::default());
        // Aside from structure, the prose categories still suppress nothing.
        for cat in [CategoryId::Email, CategoryId::Notes, CategoryId::Browser] {
            let r = rules_for(cat);
            assert!(
                !r.skip_itn && !r.skip_caps && !r.skip_terminal_punct,
                "{cat:?}"
            );
            assert!(!r.drop_final_terminal_period, "{cat:?}");
        }
    }
}
