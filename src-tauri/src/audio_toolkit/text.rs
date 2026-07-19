use natural::phonetics::soundex;
use once_cell::sync::Lazy;
use regex::Regex;
use strsim::levenshtein;

/// Builds an n-gram string by cleaning and concatenating words
///
/// Strips punctuation from each word, lowercases, and joins without spaces.
/// This allows matching "Charge B" against "ChargeBee".
fn build_ngram(words: &[&str]) -> String {
    words
        .iter()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .collect::<Vec<_>>()
        .concat()
}

/// Finds the best matching custom word for a candidate string
///
/// Uses Levenshtein distance and Soundex phonetic matching to find
/// the best match above the given threshold.
///
/// # Arguments
/// * `candidate` - The cleaned/lowercased candidate string to match
/// * `custom_words` - Original custom words (for returning the replacement)
/// * `custom_words_nospace` - Custom words with spaces removed, lowercased (for comparison)
/// * `threshold` - Maximum similarity score to accept
///
/// # Returns
/// The best matching custom word and its score, if any match was found
fn find_best_match<'a>(
    candidate: &str,
    custom_words: &'a [String],
    custom_words_nospace: &[String],
    threshold: f64,
) -> Option<(&'a String, f64)> {
    if candidate.is_empty() || candidate.len() > 50 {
        return None;
    }

    let mut best_match: Option<&String> = None;
    let mut best_score = f64::MAX;

    for (i, custom_word_nospace) in custom_words_nospace.iter().enumerate() {
        // Skip if lengths are too different (optimization + prevents over-matching)
        // Use percentage-based check: max 25% length difference (prevents n-grams from
        // matching significantly shorter custom words, e.g., "openaigpt" vs "openai")
        let len_diff = (candidate.len() as i32 - custom_word_nospace.len() as i32).abs() as f64;
        let max_len = candidate.len().max(custom_word_nospace.len()) as f64;
        let max_allowed_diff = (max_len * 0.25).max(2.0); // At least 2 chars difference allowed
        if len_diff > max_allowed_diff {
            continue;
        }

        // Calculate Levenshtein distance (normalized by length)
        let levenshtein_dist = levenshtein(candidate, custom_word_nospace);
        let max_len = candidate.len().max(custom_word_nospace.len()) as f64;
        let levenshtein_score = if max_len > 0.0 {
            levenshtein_dist as f64 / max_len
        } else {
            1.0
        };

        // Calculate phonetic similarity using Soundex
        let phonetic_match = soundex(candidate, custom_word_nospace);

        // Combine scores: favor phonetic matches, but also consider string similarity
        let combined_score = if phonetic_match {
            levenshtein_score * 0.3 // Give significant boost to phonetic matches
        } else {
            levenshtein_score
        };

        // Accept if the score is good enough (configurable threshold)
        if combined_score < threshold && combined_score < best_score {
            best_match = Some(&custom_words[i]);
            best_score = combined_score;
        }
    }

    best_match.map(|m| (m, best_score))
}

/// Whether `word` is already covered by the custom-word list: an exact
/// case-insensitive match, or a fuzzy match under `threshold` using the SAME
/// scoring `apply_custom_words` uses (levenshtein + soundex boost). "Covered"
/// means the corrector would already rewrite this word toward an entry, so
/// auto-learn (F4) must not add it again.
pub fn covered_by_custom_words(word: &str, custom_words: &[String], threshold: f64) -> bool {
    if custom_words.is_empty() {
        return false;
    }
    let candidate = word
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    if candidate.is_empty() {
        return false;
    }
    let custom_words_nospace: Vec<String> = custom_words
        .iter()
        .map(|w| w.to_lowercase().replace(' ', ""))
        .collect();
    if custom_words_nospace.contains(&candidate) {
        return true;
    }
    find_best_match(&candidate, custom_words, &custom_words_nospace, threshold).is_some()
}

/// Applies custom word corrections to transcribed text using fuzzy matching
///
/// This function corrects words in the input text by finding the best matches
/// from a list of custom words using a combination of:
/// - Levenshtein distance for string similarity
/// - Soundex phonetic matching for pronunciation similarity
/// - N-gram matching for multi-word speech artifacts (e.g., "Charge B" -> "ChargeBee")
///
/// # Arguments
/// * `text` - The input text to correct
/// * `custom_words` - List of custom words to match against
/// * `threshold` - Maximum similarity score to accept (0.0 = exact match, 1.0 = any match)
///
/// # Returns
/// The corrected text with custom words applied
pub fn apply_custom_words(text: &str, custom_words: &[String], threshold: f64) -> String {
    if custom_words.is_empty() {
        return text.to_string();
    }

    // Pre-compute lowercase versions to avoid repeated allocations
    let custom_words_lower: Vec<String> = custom_words.iter().map(|w| w.to_lowercase()).collect();

    // Pre-compute versions with spaces removed for n-gram comparison
    let custom_words_nospace: Vec<String> = custom_words_lower
        .iter()
        .map(|w| w.replace(' ', ""))
        .collect();

    let words: Vec<&str> = text.split_whitespace().collect();
    let mut result = Vec::new();
    let mut i = 0;

    while i < words.len() {
        let mut matched = false;

        // Try n-grams from longest (3) to shortest (1) - greedy matching
        for n in (1..=3).rev() {
            if i + n > words.len() {
                continue;
            }

            let ngram_words = &words[i..i + n];
            let ngram = build_ngram(ngram_words);

            if let Some((replacement, _score)) =
                find_best_match(&ngram, custom_words, &custom_words_nospace, threshold)
            {
                // Extract punctuation from first and last words of the n-gram
                let (prefix, _) = extract_punctuation(ngram_words[0]);
                let (_, suffix) = extract_punctuation(ngram_words[n - 1]);

                // Preserve case from first word
                let corrected = preserve_case_pattern(ngram_words[0], replacement);

                result.push(format!("{}{}{}", prefix, corrected, suffix));
                i += n;
                matched = true;
                break;
            }
        }

        if !matched {
            result.push(words[i].to_string());
            i += 1;
        }
    }

    result.join(" ")
}

/// Fuzzy-match threshold for custom phrases. Looser than the word threshold
/// (default 0.18): phrase keys are long concatenations where a one-word
/// mishearing ("right" for "write") lands near 0.22, and the length gate plus
/// the exact-match fast path keep false positives rare.
pub const PHRASE_MATCH_THRESHOLD: f64 = 0.25;
/// Longest spoken trigger, in words.
const MAX_PHRASE_NGRAM: usize = 8;
/// Soundex saturates and misleads on long concatenations; boost only shorties.
const PHRASE_SOUNDEX_MAX_LEN: usize = 12;
const MAX_PHRASE_CANDIDATE_LEN: usize = 100;

/// Applies custom phrase expansions: each (say, write) pair replaces a fuzzy
/// occurrence of the spoken trigger with its saved text. "btw" becomes
/// "by the way"; "write my email format" becomes a whole template. Runs on
/// every dictation, including with the LLM cleanup off.
///
/// Matching mirrors `apply_custom_words` (normalized n-gram scan, levenshtein
/// plus soundex) with three phrase-specific rules: n-grams run up to 8 words;
/// a candidate span never crosses a sentence terminator (which also keeps the
/// live-preview chunk prefixes stable, see LiveCleaner); and exact normalized
/// equality always matches regardless of threshold.
pub fn apply_custom_phrases(text: &str, phrases: &[(&str, &str)], threshold: f64) -> String {
    if phrases.is_empty() {
        return text.to_string();
    }

    // (say_key, write) with empty keys dropped.
    let keys: Vec<(String, Vec<String>, &str)> = phrases
        .iter()
        .filter_map(|(say, write)| {
            // Triggers longer than the n-gram cap can never be scanned in
            // full, and a partial fuzzy match against their prefix would be
            // wrong; drop them entirely (the UI hint says 8 words max).
            if say.split_whitespace().count() > MAX_PHRASE_NGRAM {
                return None;
            }
            let words: Vec<String> = say.split_whitespace().map(|w| build_ngram(&[w])).collect();
            let key = build_ngram(&say.split_whitespace().collect::<Vec<_>>());
            if key.is_empty() {
                None
            } else {
                Some((key, words, *write))
            }
        })
        .collect();
    if keys.is_empty() {
        return text.to_string();
    }
    let max_words = phrases
        .iter()
        .map(|(say, _)| say.split_whitespace().count())
        .max()
        .unwrap_or(1)
        .min(MAX_PHRASE_NGRAM);

    let words: Vec<&str> = text.split_whitespace().collect();
    let mut result: Vec<String> = Vec::new();
    let mut i = 0;

    while i < words.len() {
        let mut matched = false;
        for n in (1..=max_words).rev() {
            if i + n > words.len() {
                continue;
            }
            let span = &words[i..i + n];
            // A trigger never crosses a sentence boundary: every word but the
            // last must be terminator-free.
            if n > 1 && span[..n - 1].iter().any(|w| is_terminator_suffix(w)) {
                continue;
            }
            let candidate = build_ngram(span);
            if candidate.is_empty() || candidate.len() > MAX_PHRASE_CANDIDATE_LEN {
                continue;
            }

            let mut best: Option<(&str, f64)> = None;
            for (key, key_words, write) in &keys {
                if candidate == *key {
                    best = Some((write, 0.0));
                    break;
                }
                // Precision rule (replace ONLY the trigger, never a near
                // neighbor): single-word triggers require exact normalized
                // equality ("mail" must never become the "email" template).
                // Multi-word triggers keep whole-span fuzzy matching at the
                // 0.25 threshold, which is tight in practice for spans of
                // two-plus words and is what catches the designed mishearing
                // class ("right my email format").
                if key_words.len() < 2 || n < 2 {
                    continue;
                }
                let len_diff = (candidate.len() as i32 - key.len() as i32).abs() as f64;
                let max_len = candidate.len().max(key.len()) as f64;
                if len_diff > (max_len * 0.25).max(2.0) {
                    continue;
                }
                let lev = levenshtein(&candidate, key) as f64 / max_len.max(1.0);
                let score = if key.len() <= PHRASE_SOUNDEX_MAX_LEN && soundex(&candidate, key) {
                    lev * 0.3
                } else {
                    lev
                };
                if score < threshold && best.is_none_or(|(_, b)| score < b) {
                    best = Some((write, score));
                }
            }

            if let Some((write, _)) = best {
                let (prefix, _) = extract_punctuation(span[0]);
                let (_, suffix) = extract_punctuation(span[n - 1]);
                // First-letter-only capitalization: never all-caps a template
                // just because the trigger was heard as "BTW".
                let mut rendered = write.to_string();
                if span[0]
                    .chars()
                    .find(|c| c.is_alphabetic())
                    .is_some_and(|c| c.is_uppercase())
                {
                    let mut chars: Vec<char> = rendered.chars().collect();
                    if let Some(first) = chars.iter_mut().find(|c| c.is_alphabetic()) {
                        *first = first.to_uppercase().next().unwrap_or(*first);
                    }
                    rendered = chars.into_iter().collect();
                }
                // Avoid double punctuation when the write already ends closed.
                let keep_suffix = !rendered
                    .trim_end()
                    .ends_with(['.', ',', '!', '?', ';', ':']);
                result.push(format!(
                    "{prefix}{rendered}{}",
                    if keep_suffix { suffix } else { "" }
                ));
                i += n;
                matched = true;
                break;
            }
        }
        if !matched {
            result.push(words[i].to_string());
            i += 1;
        }
    }

    result.join(" ")
}

/// Preserves the case pattern of the original word when applying a replacement
fn preserve_case_pattern(original: &str, replacement: &str) -> String {
    if original.chars().all(|c| c.is_uppercase()) {
        replacement.to_uppercase()
    } else if original.chars().next().is_some_and(|c| c.is_uppercase()) {
        let mut chars: Vec<char> = replacement.chars().collect();
        if let Some(first_char) = chars.get_mut(0) {
            *first_char = first_char.to_uppercase().next().unwrap_or(*first_char);
        }
        chars.into_iter().collect()
    } else {
        replacement.to_string()
    }
}

/// Extracts punctuation prefix and suffix from a word
/// Whether a word carries a sentence-ending punctuation suffix.
pub(crate) fn is_terminator_suffix(word: &str) -> bool {
    let (_, suffix) = extract_punctuation(word);
    suffix
        .chars()
        .any(|c| matches!(c, '.' | '!' | '?' | '\u{3002}' | '\u{FF01}' | '\u{FF1F}'))
}

pub(crate) fn extract_punctuation(word: &str) -> (&str, &str) {
    let prefix_end = word.chars().take_while(|c| !c.is_alphanumeric()).count();
    let suffix_start = word
        .char_indices()
        .rev()
        .take_while(|(_, c)| !c.is_alphanumeric())
        .count();

    let prefix = if prefix_end > 0 {
        &word[..prefix_end]
    } else {
        ""
    };

    let suffix = if suffix_start > 0 {
        &word[word.len() - suffix_start..]
    } else {
        ""
    };

    (prefix, suffix)
}

/// Filler fix up aggressiveness. Local to the toolkit layer (the pipeline
/// maps `settings::FeatureLevel` onto it; Off never reaches this module).
///
/// - `Light`: core hesitations only (uh, um, ...) + 3+ stutter collapse.
/// - `Medium`: the full per-language list + stutter collapse. Byte-identical
///   to the v1 streamlining behavior; the original test corpus guards it.
/// - `High`: Medium + positionally-guarded discourse fillers (comma-bounded
///   ", you know," / ", like,"; sentence-initial "so," "well," "anyway,"
///   clusters), pair dedup at exactly 2 repeats (with an allowlist for words
///   that legitimately double), and clause-initial false-start collapse
///   ("I went, I went to the store" -> "I went to the store").
///
/// Inter-stage contract: NO level ever removes "actually" or "I mean"; those
/// are mind-change cues owned by `audio_toolkit::mind_change`, which runs
/// after this stage and needs to see them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FillerLevel {
    Light,
    Medium,
    High,
}

/// Core hesitation sounds for Light mode (English). A strict subset of the
/// Medium list: never words that could carry meaning.
const LIGHT_FILLERS_EN: &[&str] = &[
    "uh", "um", "uhm", "umm", "uhh", "erm", "mhm", "hmm", "hm", "mmm", "mm",
];

/// Words that legitimately appear doubled in English ("that that", "had had",
/// "no no is fine"): High's pair dedup must leave them alone.
const PAIR_DEDUP_ALLOWLIST: &[&str] = &["that", "had", "very", "really", "no", "so", "ha"];

/// Returns filler words appropriate for the given language code.
///
/// Some words like "um" and "ha" are real words in certain languages
/// (e.g., Portuguese "um" = "a/an", Spanish "ha" = "has"), so we only
/// include them as fillers for languages where they are truly fillers.
fn get_filler_words_for_language(lang: &str) -> &'static [&'static str] {
    let base_lang = lang.split(&['-', '_'][..]).next().unwrap_or(lang);

    match base_lang {
        "en" => &[
            "uh", "um", "uhm", "umm", "uhh", "uhhh", "ah", "hmm", "hm", "mmm", "mm", "mh", "eh",
            "er", "erm", "mhm", "ehh", "ha",
        ],
        "es" => &["ehm", "mmm", "hmm", "hm"],
        "pt" => &["ahm", "hmm", "mmm", "hm"],
        "fr" => &["euh", "hmm", "hm", "mmm"],
        "de" => &["äh", "ähm", "hmm", "hm", "mmm"],
        "it" => &["ehm", "hmm", "mmm", "hm"],
        "cs" => &["ehm", "hmm", "mmm", "hm"],
        "pl" => &["hmm", "mmm", "hm"],
        "tr" => &["hmm", "mmm", "hm"],
        "ru" => &["хм", "ммм", "hmm", "mmm"],
        "uk" => &["хм", "ммм", "hmm", "mmm"],
        "ar" => &["hmm", "mmm"],
        "ja" => &["hmm", "mmm"],
        "ko" => &["hmm", "mmm"],
        "vi" => &["hmm", "mmm", "hm"],
        "zh" => &["hmm", "mmm"],
        // Conservative universal fallback (no "um", "eh", "ha")
        _ => &[
            "uh", "uhm", "umm", "uhh", "uhhh", "ah", "hmm", "hm", "mmm", "mm", "mh", "ehh",
        ],
    }
}

static MULTI_SPACE_PATTERN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s{2,}").unwrap());

/// Comma-bounded discourse fillers removed only at High: ", you know," and
/// ", like," collapse to a single comma.
static DISCOURSE_COMMA_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i),\s+(?:you know|like),").unwrap());

/// Sentence-initial "so," / "well," / "anyway," clusters removed only at
/// High. Hesitations between the discourse word and its comma are absorbed
/// ("So um, at eight" -> "at eight"), which is why this runs BEFORE the
/// per-word hesitation removal.
static SENTENCE_INITIAL_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(^|[.!?]\s+)(?:so|well|anyway)(?:\s+(?:uh|um|uhm|umm|uhh|erm|mhm|hmm|hm|mmm|mm))*,\s+")
        .unwrap()
});

/// Collapses repeated words to a single instance.
///
/// Always collapses 3+ repetitions (stutter artifacts: "wh wh wh wh" -> "wh",
/// "I I I I" -> "I"). With `pair_dedup` (High), exactly-2 repeats also
/// collapse unless the word is on [`PAIR_DEDUP_ALLOWLIST`] ("no no is fine").
fn collapse_stutters(text: &str, pair_dedup: bool) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return text.to_string();
    }

    let mut result: Vec<&str> = Vec::new();
    let mut i = 0;

    while i < words.len() {
        let word = words[i];
        let word_lower = word.to_lowercase();

        if word_lower.chars().all(|c| c.is_alphabetic()) {
            // Count consecutive repetitions (case-insensitive)
            let mut count = 1;
            while i + count < words.len() && words[i + count].to_lowercase() == word_lower {
                count += 1;
            }

            // If 3+ repetitions, collapse to single instance
            if count >= 3 {
                result.push(word);
                i += count;
            } else if count == 2
                && pair_dedup
                && !PAIR_DEDUP_ALLOWLIST.contains(&word_lower.as_str())
            {
                result.push(word);
                i += count;
            } else {
                result.push(word);
                i += 1;
            }
        } else {
            result.push(word);
            i += 1;
        }
    }

    result.join(" ")
}

/// Collapses clause-initial false starts: an exact word run repeated after a
/// comma restart ("I went, I went to the store" -> "I went to the store").
/// Only fires when the first run begins a clause (text start or right after
/// punctuation) and ends with the restart comma; the repeat must match
/// word-for-word (case-insensitive), so rephrasings are never touched.
fn collapse_false_starts(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return text.to_string();
    }
    let core = |w: &str| -> String {
        let (p, s) = extract_punctuation(w);
        w[p.len()..w.len() - s.len()].to_lowercase()
    };
    let has_clause_punct = |w: &str| -> bool {
        let (_, s) = extract_punctuation(w);
        s.chars()
            .any(|c| matches!(c, ',' | '.' | '!' | '?' | ':' | ';'))
    };
    let has_comma_only = |w: &str| -> bool {
        let (_, s) = extract_punctuation(w);
        s.contains(',') && !s.chars().any(|c| matches!(c, '.' | '!' | '?'))
    };

    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let clause_initial = out.last().map_or(true, |w| has_clause_punct(w));
        let mut collapsed = false;
        if clause_initial {
            for k in (1..=4).rev() {
                if i + 2 * k > words.len() {
                    continue;
                }
                // First run: no internal punctuation, restart comma on its
                // last word; repeat run matches core-for-core.
                let internal_clean = (0..k - 1).all(|j| !has_clause_punct(words[i + j]));
                let restart_comma = has_comma_only(words[i + k - 1]);
                let repeats = (0..k).all(|j| {
                    let c = core(words[i + j]);
                    !c.is_empty() && c == core(words[i + k + j])
                });
                if internal_clean && restart_comma && repeats {
                    // Drop the false start; re-examine the same position so
                    // "we should, we should, we should ship" fully collapses.
                    i += k;
                    collapsed = true;
                    break;
                }
            }
        }
        if !collapsed {
            out.push(words[i]);
            i += 1;
        }
    }
    out.join(" ")
}

/// Filters transcription output by removing filler words and stutter artifacts.
///
/// This function cleans up raw transcription text by:
/// 1. (High) Removing sentence-initial discourse clusters ("So um," ...)
/// 2. Removing filler words for the language, scoped by [`FillerLevel`]
/// 3. (High) Removing comma-bounded ", you know," / ", like,"
/// 4. Collapsing repeated word stutters (3+ always; exact pairs at High)
/// 5. (High) Collapsing clause-initial false starts
/// 6. Cleaning up excess whitespace
///
/// Medium is byte-identical to the historical (v1) behavior: the original
/// test corpus below guards it. See [`FillerLevel`] for the level contract,
/// including the rule that "actually" / "I mean" are NEVER removed (they are
/// mind-change cues owned by `audio_toolkit::mind_change`).
///
/// # Arguments
/// * `text` - The raw transcription text to filter
/// * `lang` - The app language code (e.g., "en", "pt-BR") used to select filler words
/// * `level` - Filler fix up aggressiveness (Off never reaches this function)
///
/// # Returns
/// The filtered text with filler words and stutters removed
pub fn filter_transcription_output(text: &str, lang: &str, level: FillerLevel) -> String {
    let mut filtered = text.to_string();

    // High: strip sentence-initial discourse clusters BEFORE the per-word
    // hesitation removal, so "So um," is seen as one comma-closed cluster.
    // The positional patterns are English-shaped; other languages skip them.
    let base_lang = lang.split(&['-', '_'][..]).next().unwrap_or(lang);
    let english_extras = level >= FillerLevel::High && base_lang == "en";
    if english_extras {
        filtered = SENTENCE_INITIAL_PATTERN
            .replace_all(&filtered, "$1")
            .to_string();
    }

    // Build filler patterns for the language, scoped by level. Light keeps
    // only core hesitations (English; other languages' lists already are
    // core-only and apply unchanged).
    let words: &[&str] = match (level, base_lang) {
        (FillerLevel::Light, "en") => LIGHT_FILLERS_EN,
        _ => get_filler_words_for_language(lang),
    };
    let patterns: Vec<Regex> = words
        .iter()
        .map(|word| Regex::new(&format!(r"(?i)\b{}\b[,.]?", regex::escape(word))).unwrap())
        .collect();

    // Remove filler words
    for pattern in &patterns {
        filtered = pattern.replace_all(&filtered, "").to_string();
    }

    if english_extras {
        filtered = DISCOURSE_COMMA_PATTERN
            .replace_all(&filtered, ",")
            .to_string();
    }

    // Collapse repeated 1-2 letter words (stutter artifacts like "wh wh wh wh");
    // High also dedups exact pairs (allowlist-guarded).
    filtered = collapse_stutters(&filtered, level >= FillerLevel::High);

    if level >= FillerLevel::High {
        filtered = collapse_false_starts(&filtered);
    }

    // Clean up multiple spaces to single space
    filtered = MULTI_SPACE_PATTERN.replace_all(&filtered, " ").to_string();

    // Trim leading/trailing whitespace
    filtered.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_custom_words_exact_match() {
        let text = "hello world";
        let custom_words = vec!["Hello".to_string(), "World".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_apply_custom_words_fuzzy_match() {
        let text = "helo wrold";
        let custom_words = vec!["hello".to_string(), "world".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_preserve_case_pattern() {
        assert_eq!(preserve_case_pattern("HELLO", "world"), "WORLD");
        assert_eq!(preserve_case_pattern("Hello", "world"), "World");
        assert_eq!(preserve_case_pattern("hello", "WORLD"), "WORLD");
    }

    #[test]
    fn test_extract_punctuation() {
        assert_eq!(extract_punctuation("hello"), ("", ""));
        assert_eq!(extract_punctuation("!hello?"), ("!", "?"));
        assert_eq!(extract_punctuation("...hello..."), ("...", "..."));
    }

    #[test]
    fn test_empty_custom_words() {
        let text = "hello world";
        let custom_words = vec![];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_filter_filler_words() {
        let text = "So uhm I was thinking uh about this";
        let result = filter_transcription_output(text, "en", FillerLevel::Medium);
        assert_eq!(result, "So I was thinking about this");
    }

    #[test]
    fn test_filter_filler_words_case_insensitive() {
        let text = "UHM this is UH a test";
        let result = filter_transcription_output(text, "en", FillerLevel::Medium);
        assert_eq!(result, "this is a test");
    }

    #[test]
    fn test_filter_filler_words_with_punctuation() {
        let text = "Well, uhm, I think, uh. that's right";
        let result = filter_transcription_output(text, "en", FillerLevel::Medium);
        assert_eq!(result, "Well, I think, that's right");
    }

    #[test]
    fn test_filter_cleans_whitespace() {
        let text = "Hello    world   test";
        let result = filter_transcription_output(text, "en", FillerLevel::Medium);
        assert_eq!(result, "Hello world test");
    }

    #[test]
    fn test_filter_trims() {
        let text = "  Hello world  ";
        let result = filter_transcription_output(text, "en", FillerLevel::Medium);
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn test_filter_combined() {
        let text = "  Uhm, so I was, uh, thinking about this  ";
        let result = filter_transcription_output(text, "en", FillerLevel::Medium);
        assert_eq!(result, "so I was, thinking about this");
    }

    #[test]
    fn test_filter_preserves_valid_text() {
        let text = "This is a completely normal sentence.";
        let result = filter_transcription_output(text, "en", FillerLevel::Medium);
        assert_eq!(result, "This is a completely normal sentence.");
    }

    #[test]
    fn test_filter_stutter_collapse() {
        let text = "w wh wh wh wh wh wh wh wh wh why";
        let result = filter_transcription_output(text, "en", FillerLevel::Medium);
        assert_eq!(result, "w wh why");
    }

    #[test]
    fn test_filter_stutter_short_words() {
        let text = "I I I I think so so so so";
        let result = filter_transcription_output(text, "en", FillerLevel::Medium);
        assert_eq!(result, "I think so");
    }

    #[test]
    fn test_filter_stutter_longer_words() {
        let text = "Check data doc doc doc doc documentation.";
        let result = filter_transcription_output(text, "en", FillerLevel::Medium);
        assert_eq!(result, "Check data doc documentation.");
    }

    #[test]
    fn test_filter_stutter_mixed_case() {
        let text = "No NO no NO no";
        let result = filter_transcription_output(text, "en", FillerLevel::Medium);
        assert_eq!(result, "No");
    }

    #[test]
    fn test_filter_stutter_preserves_two_repetitions() {
        let text = "no no is fine";
        let result = filter_transcription_output(text, "en", FillerLevel::Medium);
        assert_eq!(result, "no no is fine");
    }

    #[test]
    fn test_filter_english_removes_um() {
        let text = "um I think um this is good";
        let result = filter_transcription_output(text, "en", FillerLevel::Medium);
        assert_eq!(result, "I think this is good");
    }

    #[test]
    fn test_filter_portuguese_preserves_um() {
        // "um" means "a/an" in Portuguese
        let text = "um gato bonito";
        let result = filter_transcription_output(text, "pt", FillerLevel::Medium);
        assert_eq!(result, "um gato bonito");
    }

    #[test]
    fn test_filter_spanish_preserves_ha() {
        // "ha" means "has" in Spanish
        let text = "ha sido un buen día";
        let result = filter_transcription_output(text, "es", FillerLevel::Medium);
        assert_eq!(result, "ha sido un buen día");
    }

    #[test]
    fn test_filter_language_code_with_region() {
        // "pt-BR" should normalize to "pt"
        let text = "um gato bonito";
        let result = filter_transcription_output(text, "pt-BR", FillerLevel::Medium);
        assert_eq!(result, "um gato bonito");
    }

    // ---- Light: core hesitations only ----

    #[test]
    fn light_removes_core_hesitations() {
        let text = "So uhm I was thinking uh about this";
        let result = filter_transcription_output(text, "en", FillerLevel::Light);
        assert_eq!(result, "So I was thinking about this");
    }

    #[test]
    fn light_keeps_medium_only_words() {
        // "ah", "eh", "er", "ha" are Medium-list words; Light must not touch
        // them (nor "so"/"well", which even Medium keeps).
        let text = "ah well er it was eh fine ha";
        let result = filter_transcription_output(text, "en", FillerLevel::Light);
        assert_eq!(result, "ah well er it was eh fine ha");
    }

    #[test]
    fn light_still_collapses_stutters() {
        let text = "wh wh wh wh why not";
        let result = filter_transcription_output(text, "en", FillerLevel::Light);
        assert_eq!(result, "wh why not");
    }

    #[test]
    fn light_list_is_a_subset_of_medium() {
        let medium = get_filler_words_for_language("en");
        for w in LIGHT_FILLERS_EN {
            assert!(medium.contains(w), "{w} missing from the Medium list");
        }
    }

    // ---- High: discourse fillers, pair dedup, false starts ----

    #[test]
    fn high_removes_comma_bounded_discourse_fillers() {
        let text = "I think, you know, we should go";
        let result = filter_transcription_output(text, "en", FillerLevel::High);
        assert_eq!(result, "I think, we should go");
        let text = "It was, like, really good";
        let result = filter_transcription_output(text, "en", FillerLevel::High);
        assert_eq!(result, "It was, really good");
    }

    #[test]
    fn high_keeps_meaningful_you_know_and_like() {
        // Not comma-bounded: these carry meaning and stay.
        let text = "Do you know the answer";
        assert_eq!(
            filter_transcription_output(text, "en", FillerLevel::High),
            text
        );
        let text = "I like this plan";
        assert_eq!(
            filter_transcription_output(text, "en", FillerLevel::High),
            text
        );
    }

    #[test]
    fn high_removes_sentence_initial_discourse_words() {
        assert_eq!(
            filter_transcription_output("So, let's begin", "en", FillerLevel::High),
            "let's begin"
        );
        assert_eq!(
            filter_transcription_output(
                "Well, that failed. Anyway, next item",
                "en",
                FillerLevel::High
            ),
            "that failed. next item"
        );
        // The cluster absorbs hesitations before its comma.
        assert_eq!(
            filter_transcription_output("So um, at eight", "en", FillerLevel::High),
            "at eight"
        );
    }

    #[test]
    fn high_keeps_so_without_comma_and_medium_keeps_so_always() {
        // No comma = possibly a real conjunction ("So far so good").
        assert_eq!(
            filter_transcription_output("So far so good", "en", FillerLevel::High),
            "So far so good"
        );
        assert_eq!(
            filter_transcription_output("So, let's begin", "en", FillerLevel::Medium),
            "So, let's begin"
        );
    }

    #[test]
    fn high_pair_dedup_with_allowlist() {
        assert_eq!(
            filter_transcription_output("the the meeting", "en", FillerLevel::High),
            "the meeting"
        );
        // Allowlisted doubles survive even at High.
        assert_eq!(
            filter_transcription_output("no no is fine", "en", FillerLevel::High),
            "no no is fine"
        );
        assert_eq!(
            filter_transcription_output("he had had enough", "en", FillerLevel::High),
            "he had had enough"
        );
        assert_eq!(
            filter_transcription_output("it was very very good", "en", FillerLevel::High),
            "it was very very good"
        );
        // Medium never pair-dedups.
        assert_eq!(
            filter_transcription_output("the the meeting", "en", FillerLevel::Medium),
            "the the meeting"
        );
    }

    #[test]
    fn high_collapses_false_starts() {
        assert_eq!(
            filter_transcription_output("I went, I went to the store", "en", FillerLevel::High),
            "I went to the store"
        );
        assert_eq!(
            filter_transcription_output(
                "we should, we should, we should ship it",
                "en",
                FillerLevel::High
            ),
            "we should ship it"
        );
        // Mid-clause repeats are not false starts.
        assert_eq!(
            filter_transcription_output("and then we, we tried", "en", FillerLevel::High),
            "and then we, we tried"
        );
        // Medium keeps false starts (LLM territory below High).
        assert_eq!(
            filter_transcription_output("I went, I went to the store", "en", FillerLevel::Medium),
            "I went, I went to the store"
        );
    }

    #[test]
    fn no_level_ever_removes_mind_change_cues() {
        // Contract with audio_toolkit::mind_change: "actually" and "I mean"
        // are correction cues; the filler stage must leave them for the
        // mind-change resolver that runs right after it.
        for level in [FillerLevel::Light, FillerLevel::Medium, FillerLevel::High] {
            let out = filter_transcription_output("at eight, actually, nine", "en", level);
            assert!(out.contains("actually"), "{level:?} ate 'actually': {out}");
            let out = filter_transcription_output("the red one, I mean, the blue one", "en", level);
            assert!(out.contains("I mean"), "{level:?} ate 'I mean': {out}");
        }
    }

    #[test]
    fn test_filter_unknown_language_uses_fallback() {
        let text = "uh I think uhm this works";
        let result = filter_transcription_output(text, "xx", FillerLevel::Medium);
        assert_eq!(result, "I think this works");
    }

    #[test]
    fn test_filter_fallback_does_not_remove_um() {
        // Fallback (unknown language) should not remove "um" since it's a real word in some languages
        let text = "um I think this works";
        let result = filter_transcription_output(text, "xx", FillerLevel::Medium);
        assert_eq!(result, "um I think this works");
    }

    #[test]
    fn test_apply_custom_words_ngram_two_words() {
        let text = "il cui nome è Charge B, che permette";
        let custom_words = vec!["ChargeBee".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert!(result.contains("ChargeBee,"));
        assert!(!result.contains("Charge B"));
    }

    #[test]
    fn test_apply_custom_words_ngram_three_words() {
        let text = "use Chat G P T for this";
        let custom_words = vec!["ChatGPT".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert!(result.contains("ChatGPT"));
    }

    #[test]
    fn test_apply_custom_words_prefers_longer_ngram() {
        let text = "Open AI GPT model";
        let custom_words = vec!["OpenAI".to_string(), "GPT".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert_eq!(result, "OpenAI GPT model");
    }

    #[test]
    fn test_apply_custom_words_ngram_preserves_case() {
        let text = "CHARGE B is great";
        let custom_words = vec!["ChargeBee".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert!(result.contains("CHARGEBEE"));
    }

    #[test]
    fn test_apply_custom_words_ngram_with_spaces_in_custom() {
        // Custom word with space should also match against split words
        let text = "using Mac Book Pro";
        let custom_words = vec!["MacBook Pro".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert!(result.contains("MacBook"));
    }

    #[test]
    fn test_apply_custom_words_trailing_number_not_doubled() {
        // Verify that trailing non-alpha chars (like numbers) aren't double-counted
        // between build_ngram stripping them and extract_punctuation capturing them
        let text = "use GPT4 for this";
        let custom_words = vec!["GPT-4".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.5);
        // Should NOT produce "GPT-44" (double-counting the trailing 4)
        assert!(
            !result.contains("GPT-44"),
            "got double-counted result: {}",
            result
        );
    }
}

/// Capitalize the first letter of the text and of every sentence (after . ? !).
/// Purely additive: it NEVER lowercases, so proper nouns and acronyms survive.
/// Spurious mid-sentence caps are the LLM pass's job (context needed).
pub fn normalize_sentence_caps(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut capitalize_next = true;
    for ch in text.chars() {
        if capitalize_next && ch.is_alphabetic() {
            out.extend(ch.to_uppercase());
            capitalize_next = false;
        } else {
            if matches!(ch, '.' | '?' | '!') {
                capitalize_next = true;
            } else if !ch.is_whitespace() && !matches!(ch, '"' | '\'' | ')' | ']' | '(' | '[') {
                // Any other visible character (digit, comma, currency...) means
                // the next letter is mid-sentence; stop hunting until the next
                // sentence terminator.
                capitalize_next = false;
            }
            out.push(ch);
        }
    }
    out
}

/// Append a period when the text ends without terminal punctuation. Closing
/// quotes/brackets after the punctuation are respected.
pub fn ensure_terminal_punctuation(text: &str) -> String {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return text.to_string();
    }
    let core = trimmed.trim_end_matches(['"', '\'', ')', ']', '}']);
    let last = core.chars().last();
    match last {
        Some('.') | Some('?') | Some('!') | Some(':') | Some(';') | Some(',') => {
            trimmed.to_string()
        }
        Some(_) => format!("{trimmed}."),
        None => trimmed.to_string(),
    }
}

#[cfg(test)]
mod net_tests {
    use super::*;

    #[test]
    fn caps_start_and_after_terminators() {
        assert_eq!(
            normalize_sentence_caps("hello there. how are you? fine! good"),
            "Hello there. How are you? Fine! Good"
        );
    }

    #[test]
    fn caps_never_lowercases() {
        assert_eq!(
            normalize_sentence_caps("meet NASA at HQ. iPhone stays."),
            "Meet NASA at HQ. IPhone stays."
        );
    }

    #[test]
    fn caps_skips_digits_leading_sentences() {
        assert_eq!(
            normalize_sentence_caps("42 is the answer. yes"),
            "42 is the answer. Yes"
        );
    }

    #[test]
    fn terminal_punctuation_appended_only_when_missing() {
        assert_eq!(ensure_terminal_punctuation("hello world"), "hello world.");
        assert_eq!(ensure_terminal_punctuation("hello world."), "hello world.");
        assert_eq!(ensure_terminal_punctuation("really?"), "really?");
        assert_eq!(ensure_terminal_punctuation("wow!"), "wow!");
        assert_eq!(
            ensure_terminal_punctuation("he said \"done\""),
            "he said \"done\"."
        );
        assert_eq!(ensure_terminal_punctuation(""), "");
        assert_eq!(ensure_terminal_punctuation("   "), "   ");
    }
}

/// Byte ranges of COMPLETE sentences (terminated by . ! ? or CJK equivalents,
/// with trailing closing quotes/brackets included). A terminator only closes a
/// sentence at end-of-text or before whitespace, so decimals ("6.5") survive.
/// Common abbreviations and single-letter initials ("Dr.", "J.", "e.g.") do
/// not split. Leading whitespace is excluded from each range.
pub fn complete_sentence_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    const ABBREV: &[&str] = &[
        "dr", "mr", "mrs", "ms", "prof", "sr", "jr", "vs", "etc", "inc", "dept", "approx", "st",
    ];
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut ranges = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        let (bi, c) = chars[i];
        if matches!(c, '.' | '!' | '?' | '\u{3002}' | '\u{FF01}' | '\u{FF1F}') {
            let mut end = bi + c.len_utf8();
            let mut j = i + 1;
            while j < chars.len()
                && matches!(
                    chars[j].1,
                    '"' | '\'' | ')' | ']' | '\u{00BB}' | '\u{201D}' | '\u{2019}'
                )
            {
                end = chars[j].0 + chars[j].1.len_utf8();
                j += 1;
            }
            let at_end = j >= chars.len();
            let before_space = !at_end && chars[j].1.is_whitespace();
            if at_end || before_space {
                let mut split = true;
                if c == '.' {
                    let prev_word: String = text[start..bi]
                        .chars()
                        .rev()
                        .take_while(|ch| ch.is_alphanumeric())
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect::<String>()
                        .to_lowercase();
                    // Single letters cover initials AND dotted abbreviations
                    // ("e.g." ends in the word "g").
                    if prev_word.chars().count() == 1 && prev_word.chars().all(char::is_alphabetic)
                    {
                        split = false;
                    }
                    if ABBREV.contains(&prev_word.as_str()) {
                        split = false;
                    }
                }
                if split {
                    let s_start = text[start..end]
                        .char_indices()
                        .find(|(_, ch)| !ch.is_whitespace())
                        .map(|(o, _)| start + o)
                        .unwrap_or(start);
                    if s_start < end {
                        ranges.push(s_start..end);
                    }
                    start = end;
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    ranges
}

/// Whether text begins with a self-correction cue ("wait", "scratch that",
/// "I mean"...), meaning it may retroactively edit the PREVIOUS sentence.
/// Recall beats precision here: a false positive only widens the chunk an
/// incremental cleanup call sees, it never changes the final text.
pub fn starts_with_correction_cue(text: &str) -> bool {
    const CUES: &[&str] = &[
        "wait",
        "no wait",
        "no,",
        "no.",
        "no no",
        "actually",
        "sorry",
        "my bad",
        "i mean",
        "i meant",
        "scratch that",
        "strike that",
        "forget that",
        "delete that",
        "make that",
        "or rather",
        "rather",
        "correction",
        "let me rephrase",
        "hold on",
    ];
    let head: String = text.trim_start().chars().take(24).collect();
    let head = head.to_lowercase();
    let head = head.trim_start_matches(|c: char| !c.is_alphanumeric());
    CUES.iter().any(|cue| {
        head.starts_with(cue)
            // Word boundary: "waiting" must not match the cue "wait".
            && !head[cue.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric())
    })
}

#[cfg(test)]
mod phrase_tests {
    use super::*;

    fn phrases() -> Vec<(&'static str, &'static str)> {
        vec![
            ("btw", "by the way"),
            (
                "write my email format",
                "Hi team,\n\nStatus update below.\n\nThanks,\nPo",
            ),
            ("my email", "pohsuchenwork@gmail.com"),
        ]
    }

    fn run(text: &str) -> String {
        apply_custom_phrases(text, &phrases(), PHRASE_MATCH_THRESHOLD)
    }

    #[test]
    fn exact_single_word_expands() {
        assert_eq!(run("btw the demo moved"), "by the way the demo moved");
    }

    #[test]
    fn caps_and_spelled_letters_normalize() {
        assert_eq!(run("BTW the demo moved"), "By the way the demo moved");
        assert_eq!(run("B T W the demo moved"), "By the way the demo moved");
        assert_eq!(run("Btw, see you"), "By the way, see you");
    }

    #[test]
    fn fuzzy_loose_trigger_hits_at_phrase_threshold() {
        let out = run("right my email format");
        assert!(out.contains("Status update"), "got: {out}");
        // The stricter words threshold would reject this candidate.
        let strict = apply_custom_phrases("right my email format", &phrases(), 0.18);
        assert!(!strict.contains("Status update"), "got: {strict}");
    }

    #[test]
    fn no_match_passes_through() {
        let t = "completely unrelated sentence here";
        assert_eq!(run(t), t);
        // "my remail" IS a designed fuzzy hit for "my email" (one letter,
        // the right/write class); a real non-match needs more distance.
        let far_miss = "bright my rewall formats maybe";
        assert_eq!(run(far_miss), far_miss);
    }

    #[test]
    fn mid_sentence_splice_keeps_neighbors() {
        assert_eq!(
            run("I will do it btw tomorrow"),
            "I will do it by the way tomorrow"
        );
    }

    #[test]
    fn repeated_triggers_all_expand() {
        assert_eq!(
            run("btw one thing btw another"),
            "by the way one thing by the way another"
        );
    }

    #[test]
    fn never_crosses_sentence_terminator() {
        // "format" ends a sentence; the trigger must not absorb the next one.
        let out = run("I said write my email. Format it nicely");
        assert!(
            !out.contains("Status update"),
            "trigger crossed a sentence boundary: {out}"
        );
    }

    #[test]
    fn multiline_write_survives_and_suffix_drops_on_closed_writes() {
        let out = run("please write my email format.");
        assert!(
            out.contains("Hi team,\n\nStatus update below."),
            "got: {out}"
        );
        let out = run("send my email.");
        // write ends without punctuation: the span suffix (.) is kept.
        assert!(out.ends_with("pohsuchenwork@gmail.com."), "got: {out}");
    }

    #[test]
    fn longest_trigger_wins_overlaps() {
        let out = run("write my email format now");
        assert!(out.contains("Status update"), "got: {out}");
        assert!(
            !out.contains("pohsuchenwork"),
            "shorter overlapping trigger must lose: {out}"
        );
    }

    #[test]
    fn nine_word_trigger_never_matches() {
        let long_say = "one two three four five six seven eight nine";
        let pairs = vec![(long_say, "X")];
        let text = long_say;
        assert_eq!(
            apply_custom_phrases(text, &pairs, PHRASE_MATCH_THRESHOLD),
            text,
            "8-word cap"
        );
    }

    #[test]
    fn empty_phrases_is_identity() {
        assert_eq!(
            apply_custom_phrases("hello", &[], PHRASE_MATCH_THRESHOLD),
            "hello"
        );
    }
}

#[cfg(test)]
mod sentence_tests {
    use super::*;

    fn sents(text: &str) -> Vec<&str> {
        complete_sentence_ranges(text)
            .into_iter()
            .map(|r| &text[r])
            .collect()
    }

    #[test]
    fn splits_basic_sentences() {
        assert_eq!(
            sents("Hello there. How are you? Great! trailing fragment"),
            vec!["Hello there.", "How are you?", "Great!"]
        );
    }

    #[test]
    fn decimals_and_times_do_not_split() {
        assert_eq!(
            sents("The price is 6.5 dollars today. Meet at 9.30 sharp."),
            vec!["The price is 6.5 dollars today.", "Meet at 9.30 sharp."]
        );
    }

    #[test]
    fn abbreviations_do_not_split() {
        assert_eq!(
            sents("Dr. Smith arrived, e.g. early. Everyone cheered."),
            vec!["Dr. Smith arrived, e.g. early.", "Everyone cheered."]
        );
    }

    #[test]
    fn closing_quotes_stay_attached() {
        assert_eq!(
            sents("He said \"done.\" Next item."),
            vec!["He said \"done.\"", "Next item."]
        );
    }

    #[test]
    fn incomplete_tail_is_not_a_sentence() {
        assert_eq!(sents("First one. second still going"), vec!["First one."]);
        assert!(sents("no terminator at all").is_empty());
    }

    #[test]
    fn correction_cues_detected() {
        assert!(starts_with_correction_cue("wait, make it Tuesday"));
        assert!(starts_with_correction_cue("  Actually let's not"));
        assert!(starts_with_correction_cue("Scratch that."));
        assert!(starts_with_correction_cue("I mean the other one"));
        assert!(starts_with_correction_cue("No, the blue one"));
        assert!(!starts_with_correction_cue("The meeting is at nine"));
        assert!(!starts_with_correction_cue("Nothing else matters"));
        assert!(!starts_with_correction_cue("Waiting for the bus"));
    }
}
