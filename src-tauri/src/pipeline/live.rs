//! Shared live-text sentence walk (F3). Single-sources the boundary rule the
//! LiveCleaner tick and the TextboxClean deterministic injector both need:
//! how much of the deterministically filtered live text is safe to hand off
//! incrementally.

/// Byte end of the completed-sentence region of `filtered` that is safe to
/// process incrementally, holding back the newest `holdback` complete
/// sentences (the newest one can still be rewritten by a correction in the
/// words that follow it) and never ending right after a correction-cue
/// sentence: "Scratch that." edits BOTH neighbors, and its replacement
/// arrives in the sentence after it, so cue sentences always ship with their
/// successor. `None` when nothing is safely eligible yet.
pub fn eligible_sentence_end(filtered: &str, holdback: usize) -> Option<usize> {
    let ranges = crate::audio_toolkit::complete_sentence_ranges(filtered);
    if ranges.len() <= holdback {
        return None;
    }
    let mut last_idx = ranges.len() - 1 - holdback;
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
    if crate::audio_toolkit::starts_with_correction_cue(&filtered[ranges[last_idx].clone()]) {
        return None; // everything pending is cue-glued to text still coming
    }
    Some(ranges[last_idx].end)
}

#[cfg(test)]
mod tests {
    use super::eligible_sentence_end;

    #[test]
    fn holdback_keeps_the_newest_sentence() {
        assert_eq!(eligible_sentence_end("One done.", 1), None);
        assert_eq!(
            eligible_sentence_end("One done. Two done.", 1),
            Some("One done.".len())
        );
        assert_eq!(
            eligible_sentence_end("One done. Two done. Three done.", 1),
            Some("One done. Two done.".len())
        );
        // Holdback 0 hands off everything complete.
        assert_eq!(
            eligible_sentence_end("One done. Two done.", 0),
            Some("One done. Two done.".len())
        );
    }

    #[test]
    fn incomplete_tail_is_never_eligible() {
        assert_eq!(
            eligible_sentence_end("One done. Two done. still talking", 1),
            Some("One done.".len())
        );
    }

    #[test]
    fn cue_sentences_ship_with_their_successor() {
        // The boundary walks back past a cue-opening sentence.
        assert_eq!(
            eligible_sentence_end("Send it to John. Scratch that. Use Joan instead.", 1),
            Some("Send it to John.".len())
        );
        // Everything pending is cue-glued: nothing eligible yet.
        assert_eq!(eligible_sentence_end("Scratch that. No wait.", 1), None);
    }
}
