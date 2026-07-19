//! Deterministic mind-change resolution: when the speaker corrects themselves
//! mid-dictation, keep only the final version and delete the earlier attempt
//! plus its cue ("at eight, no wait, nine" -> "at nine").
//!
//! Design constraints (mirrors itn.rs):
//! - Idempotent: every output is a fixed point (the fixpoint loop only fires
//!   edits that strictly remove the cue, so a second run finds nothing).
//! - Sentence-scoped: a replacement never crosses a sentence terminator, with
//!   TWO sanctioned crossings: (a) a standalone retraction sentence ("Scratch
//!   that.") deletes the sentence before it, and (b) a sentence BEGINNING
//!   with a replacement cue merges into its predecessor when the alignment
//!   rules resolve against that sentence's tail ("meet at 8. No wait, 9."
//!   -> "meet at 9."). (b) exists because real STT punctuation routinely
//!   terminates the sentence before the cue; the same no-alignment-no-edit
//!   rule applies, so "No wait, I agree with you." never merges. Live paths
//!   treat both as rewrites of committed text (injector freeze + stitch
//!   fallback handle them downstream).
//! - THE precision rule: no alignment = no edit, cue text kept. We never
//!   delete words we cannot prove redundant (that is the LLM pass's job).
//! - `protect_tail_words` shields the newest words during live ticks (2),
//!   like ITN's tail guard: an edit whose examined region touches the tail is
//!   deferred until more words arrive (or the final pass with 0).
//! - Word-boundary matching throughout ("waiting" never matches the cue
//!   "wait"); cores are compared case-insensitively with punctuation shells
//!   stripped.
//!
//! Inter-stage contract: the filler stage NEVER removes "actually" or
//! "I mean" at any level; those words are cues owned by this module (see
//! `filter_transcription_output`). Fillers run FIRST so cue detection sees
//! contiguous cues ("no, um, wait" -> "no, wait").
//!
//! Cue lexicon by level:
//! - Light: replacements "no wait", "wait no"; retractions "scratch that",
//!   "strike that", "forget that", "delete that".
//! - Medium adds replacements: "actually", "i mean", "i meant", "make that",
//!   "or rather", "rather", "sorry", "my bad", "correction".
//! - High adds comma-bounded bare "no" / "wait", and the cue-less
//!   anchor-repeat rule R3 ("at eight, at nine" -> "at nine") for the
//!   Number/Weekday/Month/TimeWord classes only (ProperCase is excluded to
//!   protect appositives like "my brother, John").

use super::itn::is_number_word;
use super::text::{complete_sentence_ranges, extract_punctuation};

/// Aggressiveness of the mind-change pass. Local to this module so the
/// toolkit layer stays independent of the settings types; the pipeline maps
/// `settings::FeatureLevel` onto it (Off = the stage is not called).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MindChangeLevel {
    Light,
    Medium,
    High,
}

/// One whitespace token: punctuation shells + lowercase core (itn.rs shape).
/// Splices keep whole `raw` tokens, so only the suffix shell is consulted
/// after tokenization (no `prefix` field needed).
struct Tok<'a> {
    raw: &'a str,
    core: String,
    suffix: &'a str,
}

impl Tok<'_> {
    fn has_comma(&self) -> bool {
        self.suffix.contains(',')
    }
    fn has_terminator(&self) -> bool {
        self.suffix
            .chars()
            .any(|c| matches!(c, '.' | '!' | '?' | '\u{3002}' | '\u{FF01}' | '\u{FF1F}'))
    }
}

fn tokenize(text: &str) -> Vec<Tok<'_>> {
    text.split_whitespace()
        .map(|raw| {
            let (prefix, suffix) = extract_punctuation(raw);
            let core = raw[prefix.len()..raw.len() - suffix.len()].to_lowercase();
            Tok { raw, core, suffix }
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CueKind {
    Replace,
    Retract,
}

/// Multi-word cues active at a level, longest-first so "or rather" wins over
/// "rather" at the same position. Retractions are active at every level.
fn cues_for(level: MindChangeLevel) -> Vec<(&'static [&'static str], CueKind)> {
    let mut cues: Vec<(&'static [&'static str], CueKind)> = vec![
        (&["scratch", "that"], CueKind::Retract),
        (&["strike", "that"], CueKind::Retract),
        (&["forget", "that"], CueKind::Retract),
        (&["delete", "that"], CueKind::Retract),
        (&["no", "wait"], CueKind::Replace),
        (&["wait", "no"], CueKind::Replace),
    ];
    if level >= MindChangeLevel::Medium {
        cues.extend([
            (&["or", "rather"] as &[_], CueKind::Replace),
            (&["i", "mean"], CueKind::Replace),
            (&["i", "meant"], CueKind::Replace),
            (&["make", "that"], CueKind::Replace),
            (&["my", "bad"], CueKind::Replace),
            (&["actually"], CueKind::Replace),
            (&["rather"], CueKind::Replace),
            (&["sorry"], CueKind::Replace),
            (&["correction"], CueKind::Replace),
        ]);
    }
    cues.sort_by_key(|(words, _)| std::cmp::Reverse(words.len()));
    cues
}

/// Word classes for R2 alignment. `Other` never class-matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WordClass {
    Number,
    Weekday,
    Month,
    TimeWord,
    Other,
}

fn digit_shape(core: &str) -> bool {
    // "9", "45", "8:30", "6.5", "8am", "9:15pm"
    let core = core
        .strip_suffix("am")
        .or_else(|| core.strip_suffix("pm"))
        .unwrap_or(core);
    !core.is_empty()
        && core.chars().any(|c| c.is_ascii_digit())
        && core
            .chars()
            .all(|c| c.is_ascii_digit() || c == ':' || c == '.')
}

fn is_number_token(core: &str) -> bool {
    if core.is_empty() {
        return false;
    }
    if digit_shape(core) {
        return true;
    }
    // "twenty" and hyphenated "twenty-five".
    core.split('-').all(|part| is_number_word(part)) && !core.contains("--")
}

fn closed_class(core: &str) -> WordClass {
    if is_number_token(core) {
        return WordClass::Number;
    }
    match core {
        "monday" | "tuesday" | "wednesday" | "thursday" | "friday" | "saturday" | "sunday" => {
            WordClass::Weekday
        }
        "january" | "february" | "march" | "april" | "may" | "june" | "july" | "august"
        | "september" | "october" | "november" | "december" => WordClass::Month,
        "noon" | "midnight" | "today" | "tomorrow" | "tonight" => WordClass::TimeWord,
        _ => WordClass::Other,
    }
}

/// ProperCase: raw starts uppercase-alphabetic and the token is NOT sentence
/// initial (every sentence-initial word is capitalized, so there is no
/// signal there).
fn is_proper_case(tok: &Tok, sentence_index: usize) -> bool {
    sentence_index > 0
        && tok
            .raw
            .chars()
            .find(|c| c.is_alphanumeric())
            .is_some_and(|c| c.is_alphabetic() && c.is_uppercase())
}

/// Pairwise ExactWord-or-closed-class match for R2.
fn class_match(a: &Tok, ai: usize, b: &Tok, bi: usize) -> bool {
    if !a.core.is_empty() && a.core == b.core {
        return true; // ExactWord
    }
    let (ca, cb) = (closed_class(&a.core), closed_class(&b.core));
    if ca != WordClass::Other && ca == cb {
        return true;
    }
    is_proper_case(a, ai) && is_proper_case(b, bi)
}

/// A resolved edit within one segment's token list.
struct Edit {
    /// Tokens [replace_from .. cue_end) are removed; B keeps its own shells.
    replace_from: usize,
    /// How many B tokens the decision examined (for the live tail guard).
    examined_b: usize,
}

/// R1 anchor alignment: B's first word re-anchors into A. Prefer the
/// occurrence where B spans exactly A's remaining suffix; otherwise take the
/// last occurrence and require B to cover A to its END ("meet at eight
/// tomorrow, no wait, at nine" has an unconsumed suffix and stays verbatim).
fn resolve_r1(a: &[Tok], b: &[Tok]) -> Option<Edit> {
    let b0 = &b[0].core;
    if b0.is_empty() {
        return None;
    }
    let candidates: Vec<usize> = (0..a.len()).filter(|&p| &a[p].core == b0).collect();
    let exact = candidates
        .iter()
        .rev()
        .find(|&&p| a.len() - p == b.len())
        .copied();
    let p = exact.or_else(|| candidates.last().copied())?;
    if b.len() < a.len() - p {
        return None; // unconsumed A suffix
    }
    Some(Edit {
        replace_from: p,
        examined_b: a.len() - p,
    })
}

/// R2 class alignment without an anchor: A's last k tokens pair with B's
/// first k, all pairs ExactWord or same closed class, k from 4 down to 1.
fn resolve_r2(a: &[Tok], b: &[Tok], b_offset: usize) -> Option<Edit> {
    let kmax = 4usize.min(a.len()).min(b.len());
    for k in (1..=kmax).rev() {
        let start = a.len() - k;
        if (0..k).all(|j| class_match(&a[start + j], start + j, &b[j], b_offset + j)) {
            return Some(Edit {
                replace_from: start,
                examined_b: k,
            });
        }
    }
    None
}

/// R2b bounded frame: B replaces an equal-length A suffix whose first and
/// last words match B's exactly ("the red one, no wait, the blue one").
/// R1's exact-span preference usually subsumes this; it stays as a distinct
/// safety net per the plan.
fn resolve_r2b(a: &[Tok], b: &[Tok]) -> Option<Edit> {
    if b.len() < 2 || b.len() > a.len() {
        return None;
    }
    let start = a.len() - b.len();
    if !a[start].core.is_empty()
        && a[start].core == b[0].core
        && a[a.len() - 1].core == b[b.len() - 1].core
    {
        return Some(Edit {
            replace_from: start,
            examined_b: b.len(),
        });
    }
    None
}

/// The full pass. `protect_tail_words` shields the newest words from edits
/// (live ticks pass 2; final text passes 0).
pub fn apply_mind_change(text: &str, level: MindChangeLevel, protect_tail_words: usize) -> String {
    let mut current = text.to_string();
    // Fixpoint loop: each iteration applies at most one edit and strictly
    // shrinks the text, so the cap is a formality against pathological input.
    for _ in 0..8 {
        match rewrite_once(&current, level, protect_tail_words) {
            Some(next) => current = next,
            None => break,
        }
    }
    current
}

/// Byte ranges of segments: complete sentences plus the unterminated tail.
fn segment_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = complete_sentence_ranges(text);
    let tail_from = ranges.last().map(|r| r.end).unwrap_or(0);
    if let Some((off, _)) = text[tail_from..]
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
    {
        ranges.push(tail_from + off..text.len());
    }
    ranges
}

/// Apply the FIRST resolvable edit and return the rebuilt text, or None when
/// nothing fires. Only the edited region is rebuilt (token-joined with single
/// spaces); untouched text is preserved byte-for-byte, which is what keeps
/// completed-sentence prefixes stable for the live pipeline.
fn rewrite_once(text: &str, level: MindChangeLevel, protect_tail_words: usize) -> Option<String> {
    let total_words = text.split_whitespace().count();
    let limit = total_words.saturating_sub(protect_tail_words);
    let ranges = segment_ranges(text);

    // Pass A: a standalone retraction sentence deletes its predecessor (the
    // one sanctioned terminator crossing). With no predecessor there is
    // nothing to prove redundant, so the cue is kept.
    let mut words_before = 0usize;
    let mut seg_word_starts = Vec::with_capacity(ranges.len());
    for r in &ranges {
        seg_word_starts.push(words_before);
        words_before += text[r.clone()].split_whitespace().count();
    }
    for (si, r) in ranges.iter().enumerate() {
        let seg = &text[r.clone()];
        if si == 0 || !is_standalone_retraction(seg, level) {
            continue;
        }
        let seg_words = seg.split_whitespace().count();
        if seg_word_starts[si] + seg_words > limit {
            continue; // tail guard: the cue may still be growing
        }
        let prev = &ranges[si - 1];
        let mut out = String::with_capacity(text.len());
        out.push_str(&text[..prev.start]);
        out.push_str(text[r.end..].trim_start());
        return Some(out.trim_end().to_string());
    }

    // Pass A2: a sentence that BEGINS with a replacement cue merges into its
    // predecessor when B aligns against that sentence's tail (sanctioned
    // crossing (b): STT punctuation often splits "at 8. No wait, 9."). The
    // alignment rules are identical to the in-sentence case, so precision is
    // unchanged; only the search scope differs.
    for si in 1..ranges.len() {
        let seg = &text[ranges[si].clone()];
        let prev_seg = &text[ranges[si - 1].clone()];
        if seg.contains('\n') || prev_seg.contains('\n') {
            continue; // spliced templates are never token-rejoined
        }
        let toks = tokenize(seg);
        let prev_toks = tokenize(prev_seg);
        if prev_toks.is_empty() {
            continue;
        }
        // Candidate leading cues: the level's Replace cues, plus High's bare
        // comma-bounded "no"/"wait" (which live outside the cue list; the
        // in-sentence path special-cases them the same way).
        let mut leading: Vec<usize> = Vec::new();
        for (words, kind) in &cues_for(level) {
            if *kind == CueKind::Replace
                && words.len() <= toks.len()
                && toks[..words.len()]
                    .iter()
                    .zip(words.iter())
                    .all(|(t, w)| t.core == *w)
            {
                leading.push(words.len());
            }
        }
        if level >= MindChangeLevel::High
            && !toks.is_empty()
            && (toks[0].core == "no" || toks[0].core == "wait")
            && toks[0].has_comma()
        {
            leading.push(1);
        }
        // Longest cue first, deduped.
        leading.sort_unstable_by(|x, y| y.cmp(x));
        leading.dedup();

        let mut fired: Option<String> = None;
        for cue_len in leading {
            let b = &toks[cue_len..];
            if b.is_empty() {
                continue; // "Meet at nine. No wait." keeps the trailing cue
            }
            let Some(edit) = resolve_r1(&prev_toks, b)
                .or_else(|| resolve_r2(&prev_toks, b, cue_len))
                .or_else(|| resolve_r2b(&prev_toks, b))
            else {
                continue;
            };
            // Live tail guard: the decision examined B up to this global word
            // index; defer while those words may still be growing.
            let examined_end_word = seg_word_starts[si] + cue_len + edit.examined_b;
            if examined_end_word > limit {
                continue;
            }
            let mut parts: Vec<&str> = Vec::with_capacity(edit.replace_from + b.len());
            parts.extend(prev_toks[..edit.replace_from].iter().map(|t| t.raw));
            parts.extend(b.iter().map(|t| t.raw));
            fired = Some(parts.join(" "));
            break;
        }
        if let Some(merged) = fired {
            let mut out = String::with_capacity(text.len());
            out.push_str(&text[..ranges[si - 1].start]);
            out.push_str(&merged);
            out.push_str(&text[ranges[si].end..]);
            return Some(out);
        }
    }

    // Pass B: in-sentence cues. Multi-line segments are skipped whole: those
    // come from spliced custom-phrase templates (saved text, not speech), and
    // token-rejoining would destroy their formatting.
    for (si, r) in ranges.iter().enumerate() {
        let seg = &text[r.clone()];
        if seg.contains('\n') {
            continue;
        }
        if let Some(new_seg) = rewrite_segment(seg, level, seg_word_starts[si], limit) {
            let mut out = String::with_capacity(text.len());
            out.push_str(&text[..r.start]);
            if new_seg.is_empty() {
                // The segment vanished (end-of-sentence retraction): also
                // swallow the whitespace that separated it from its successor.
                out.push_str(text[r.end..].trim_start());
                return Some(out.trim_end().to_string());
            }
            out.push_str(&new_seg);
            out.push_str(&text[r.end..]);
            return Some(out);
        }
    }
    None
}

/// Whether a whole segment is exactly one retraction cue (plus punctuation).
fn is_standalone_retraction(seg: &str, level: MindChangeLevel) -> bool {
    let toks = tokenize(seg);
    cues_for(level).iter().any(|(words, kind)| {
        *kind == CueKind::Retract
            && toks.len() == words.len()
            && toks.iter().zip(words.iter()).all(|(t, w)| t.core == *w)
    })
}

/// Find and apply the first resolvable cue edit inside one sentence segment.
/// `seg_word_start`/`limit` implement the global live tail guard.
fn rewrite_segment(
    seg: &str,
    level: MindChangeLevel,
    seg_word_start: usize,
    limit: usize,
) -> Option<String> {
    let toks = tokenize(seg);
    let cues = cues_for(level);

    for pos in 0..toks.len() {
        // Longest cue first at each position.
        for (words, kind) in &cues {
            let end = pos + words.len();
            if end > toks.len() {
                continue;
            }
            if !toks[pos..end]
                .iter()
                .zip(words.iter())
                .all(|(t, w)| t.core == *w)
            {
                continue;
            }
            // No token inside the cue except the last may end the sentence
            // (segmentation makes this unreachable, but keep it explicit).
            if toks[pos..end - 1].iter().any(|t| t.has_terminator()) {
                continue;
            }
            if let Some(out) = try_cue(&toks, pos, end, *kind, seg_word_start, limit) {
                return Some(out);
            }
        }
        // High only: comma-bounded bare "no" / "wait".
        if level >= MindChangeLevel::High
            && (toks[pos].core == "no" || toks[pos].core == "wait")
            && toks[pos].has_comma()
            && pos > 0
            && toks[pos - 1].has_comma()
        {
            if let Some(out) = try_cue(&toks, pos, pos + 1, CueKind::Replace, seg_word_start, limit)
            {
                return Some(out);
            }
        }
    }

    if level >= MindChangeLevel::High {
        if let Some(out) = try_anchor_repeat(&toks, seg_word_start, limit) {
            return Some(out);
        }
    }
    None
}

/// Resolve one cue occurrence. Returns the rebuilt segment on success.
fn try_cue(
    toks: &[Tok],
    cue_start: usize,
    cue_end: usize,
    kind: CueKind,
    seg_word_start: usize,
    limit: usize,
) -> Option<String> {
    let a = &toks[..cue_start];
    let b = &toks[cue_end..];

    // Empty-A never fires ("No, I don't think so" stays whole).
    if a.is_empty() {
        return None;
    }

    if kind == CueKind::Retract {
        // Mid-sentence retraction needs bounding: a comma right before the
        // cue, and a comma (replacement follows) or terminator (the sentence
        // ends on the retraction) right after it. "I couldn't scratch that
        // off the list" has neither and stays verbatim.
        if !toks[cue_start - 1].has_comma() {
            return None;
        }
        let last = &toks[cue_end - 1];
        if b.is_empty() {
            if !last.has_terminator() && !last.has_comma() {
                return None;
            }
            // "we meet at eight, scratch that." deletes the whole sentence.
            let cue_end_word = seg_word_start + cue_end;
            if cue_end_word > limit {
                return None;
            }
            return Some(String::new());
        }
        if !last.has_comma() {
            return None;
        }
        // Retraction with a replacement resolves exactly like a replacement
        // cue below ("send it to Bob, scratch that, to Alice").
    } else if b.is_empty() {
        return None; // replacement with nothing after the cue: keep it
    }

    let edit = resolve_r1(a, b)
        .or_else(|| resolve_r2(a, b, cue_end))
        .or_else(|| resolve_r2b(a, b))?;

    // Live tail guard: the decision examined B up to this global word index.
    let examined_end_word = seg_word_start + cue_end + edit.examined_b;
    if examined_end_word > limit {
        return None;
    }

    let mut parts: Vec<&str> = Vec::with_capacity(edit.replace_from + b.len());
    parts.extend(toks[..edit.replace_from].iter().map(|t| t.raw));
    parts.extend(b.iter().map(|t| t.raw));
    Some(parts.join(" "))
}

/// R3 (High): cue-less anchor repeat, "at eight, at nine" -> "at nine".
/// Requires two adjacent comma-separated groups sharing the anchor word with
/// same-class Number/Weekday/Month/TimeWord payloads. Three or more groups in
/// a row, or a following "and"/"or" + anchor, read as an enumeration and are
/// left alone.
fn try_anchor_repeat(toks: &[Tok], seg_word_start: usize, limit: usize) -> Option<String> {
    // parse a group at i: anchor word (not itself classy), then 1..=3 tokens
    // of one class; returns (end_exclusive, class, comma_terminated).
    let parse_group = |i: usize, anchor: Option<&str>| -> Option<(usize, WordClass, bool)> {
        let t = toks.get(i)?;
        if closed_class(&t.core) != WordClass::Other || t.core.is_empty() {
            return None;
        }
        if let Some(a) = anchor {
            if t.core != a {
                return None;
            }
        }
        if t.has_comma() || t.has_terminator() {
            return None;
        }
        let first = toks.get(i + 1)?;
        let class = closed_class(&first.core);
        if class == WordClass::Other {
            return None;
        }
        let mut j = i + 1;
        while j < toks.len() && closed_class(&toks[j].core) == class {
            let done = toks[j].has_comma() || toks[j].has_terminator() || j - i >= 3;
            j += 1;
            if done {
                break;
            }
        }
        let last = &toks[j - 1];
        Some((j, class, last.has_comma()))
    };

    for i in 0..toks.len() {
        let Some((g1_end, class, g1_comma)) = parse_group(i, None) else {
            continue;
        };
        if !g1_comma {
            continue; // groups are comma-separated
        }
        let anchor = toks[i].core.clone();
        let Some((g2_end, class2, _)) = parse_group(g1_end, Some(&anchor)) else {
            continue;
        };
        if class2 != class {
            continue;
        }
        // Look-behind enumeration guard: a comma'd same-anchor group ending
        // right where this pair starts means 3+ groups in a row.
        if (0..i).any(|j| matches!(parse_group(j, Some(&anchor)), Some((end, _, true)) if end == i))
        {
            continue;
        }
        // group 2 must close the pattern: segment end or its own boundary.
        let g2_last = &toks[g2_end - 1];
        if g2_end < toks.len() && !g2_last.has_comma() && !g2_last.has_terminator() {
            continue;
        }
        // Enumeration guards: a third anchor group, or "and"/"or" + anchor.
        if parse_group(g2_end, Some(&anchor)).is_some() {
            continue;
        }
        if let Some(next) = toks.get(g2_end) {
            if (next.core == "and" || next.core == "or")
                && toks.get(g2_end + 1).is_some_and(|t| t.core == anchor)
            {
                continue;
            }
        }
        // Tail guard covers through group 2.
        if seg_word_start + g2_end > limit {
            continue;
        }
        let mut parts: Vec<&str> = Vec::with_capacity(toks.len() - (g1_end - i));
        parts.extend(toks[..i].iter().map(|t| t.raw));
        parts.extend(toks[g1_end..].iter().map(|t| t.raw));
        return Some(parts.join(" "));
    }
    None
}

#[cfg(test)]
mod mind_change_tests {
    use super::{apply_mind_change, MindChangeLevel};

    use MindChangeLevel::{High, Light, Medium};

    /// (input, level, expected). Every row also runs the idempotence property.
    fn table() -> Vec<(&'static str, MindChangeLevel, &'static str)> {
        vec![
            // --- "at eight, no wait, nine" variants (canonical) ---
            ("at eight, no wait, nine", Light, "at nine"),
            ("at eight no wait nine", Light, "at nine"),
            ("at 8, no wait, 9", Light, "at 9"),
            ("at 8:30, no wait, 9:15", Light, "at 9:15"),
            ("at eight, wait no, nine", Light, "at nine"),
            ("At eight, no wait, nine.", Light, "At nine."),
            // R2 with B longer than the matched span
            ("at eight, no wait, nine fifteen", Light, "at nine fifteen"),
            (
                "we need five, no wait, six copies",
                Light,
                "we need six copies",
            ),
            // --- R1 anchor alignment ---
            ("meet at eight, no wait, at nine", Light, "meet at nine"),
            (
                "meet at eight, no wait, at nine thirty",
                Light,
                "meet at nine thirty",
            ),
            // unconsumed-suffix rejection: cannot prove "tomorrow" redundant
            (
                "meet at eight tomorrow, no wait, at nine",
                High,
                "meet at eight tomorrow, no wait, at nine",
            ),
            // --- classes ---
            ("send it to John, no wait, Joan", Light, "send it to Joan"),
            (
                "we leave Tuesday, actually, Wednesday",
                Medium,
                "we leave Wednesday",
            ),
            ("in March, no wait, April", Light, "in April"),
            ("do it today, actually, tomorrow", Medium, "do it tomorrow"),
            ("eight thirty, I mean, nine fifteen", Medium, "nine fifteen"),
            ("Tuesday, my bad, Wednesday", Medium, "Wednesday"),
            // --- level gating ---
            (
                "we leave Tuesday, actually, Wednesday",
                Light,
                "we leave Tuesday, actually, Wednesday",
            ),
            ("at eight, no, nine", High, "at nine"),
            ("at eight, no, nine", Medium, "at eight, no, nine"),
            ("at eight, wait, nine", High, "at nine"),
            ("at eight, wait, nine", Medium, "at eight, wait, nine"),
            // --- Medium cue coverage ---
            ("meet at eight, make that nine", Medium, "meet at nine"),
            (
                "send it Monday, or rather, Tuesday",
                Medium,
                "send it Tuesday",
            ),
            ("at eight, sorry, nine", Medium, "at nine"),
            ("at eight, correction, nine", Medium, "at nine"),
            // --- empty-A guards ---
            ("No, I don't think so", High, "No, I don't think so"),
            ("No wait, nine is fine", High, "No wait, nine is fine"),
            (
                "Actually, the demo moved",
                Medium,
                "Actually, the demo moved",
            ),
            // --- no-alignment guard: cue kept, nothing deleted ---
            ("say no wait for it", High, "say no wait for it"),
            (
                "go home, no wait, stay put",
                High,
                "go home, no wait, stay put",
            ),
            ("I'm sorry about that", Medium, "I'm sorry about that"),
            (
                "make it blue rather than red",
                Medium,
                "make it blue rather than red",
            ),
            ("I'd rather stay", Medium, "I'd rather stay"),
            // word boundary: "waiting" is not "wait"
            ("we are waiting, no rush", High, "we are waiting, no rush"),
            // --- bounded frame ---
            ("the red one, no wait, the blue one", Light, "the blue one"),
            // --- cross-sentence leading replacement cue (sanctioned crossing b) ---
            // Real STT punctuation splits the flagship pattern into sentences.
            (
                "So let's meet at 8. No wait, 9.",
                Light,
                "So let's meet at 9.",
            ),
            (
                "Send the invite to John. No wait, Joan.",
                Light,
                "Send the invite to Joan.",
            ),
            (
                "Let's do Tuesday. Actually, Wednesday.",
                Medium,
                "Let's do Wednesday.",
            ),
            // "actually" is a Medium cue: Light leaves the split form alone
            (
                "Let's do Tuesday. Actually, Wednesday.",
                Light,
                "Let's do Tuesday. Actually, Wednesday.",
            ),
            // no alignment = no merge (the cue opens a genuine new thought)
            (
                "No wait, I agree with you.",
                High,
                "No wait, I agree with you.",
            ),
            (
                "Let's do Tuesday. Actually, that sounds great.",
                High,
                "Let's do Tuesday. Actually, that sounds great.",
            ),
            // empty B keeps the trailing cue sentence
            ("Meet at nine. No wait.", High, "Meet at nine. No wait."),
            // chains resolve over fixpoint iterations
            (
                "So let's meet at 8. No wait, 9. Send it to John. No wait, Joan.",
                Light,
                "So let's meet at 9. Send it to Joan.",
            ),
            // High's bare "no" needs its comma bound even at sentence start
            ("Take five. No, six.", High, "Take six."),
            ("Take five. No six.", High, "Take five. No six."),
            // --- retractions ---
            ("Send the report to Bob. Scratch that.", Light, ""),
            (
                "We meet at nine. Scratch that. Let's do ten.",
                Light,
                "Let's do ten.",
            ),
            (
                "send it to Bob, scratch that, to Alice",
                Light,
                "send it to Alice",
            ),
            (
                "we meet at eight, scratch that. Nine works.",
                Light,
                "Nine works.",
            ),
            (
                "I couldn't scratch that off the list",
                High,
                "I couldn't scratch that off the list",
            ),
            // leading retraction has nothing to retract: kept
            (
                "Scratch that. Hello there.",
                High,
                "Scratch that. Hello there.",
            ),
            // --- R3 anchor repeat (High only) ---
            ("at eight, at nine", High, "at nine"),
            ("at eight, at nine", Medium, "at eight, at nine"),
            ("Meet at eight, at nine.", High, "Meet at nine."),
            ("on Monday, on Tuesday", High, "on Tuesday"),
            // enumerations stay whole
            (
                "at eight, at nine, and at ten",
                High,
                "at eight, at nine, and at ten",
            ),
            (
                "at eight, at nine, at ten",
                High,
                "at eight, at nine, at ten",
            ),
            // appositive protection: ProperCase is not an R3 class
            (
                "with my brother, with John",
                High,
                "with my brother, with John",
            ),
            // --- chains: rightmost wins through the fixpoint loop ---
            ("at eight, no wait, nine, actually, ten", Medium, "at ten"),
        ]
    }

    #[test]
    fn resolution_table() {
        for (input, level, want) in table() {
            let got = apply_mind_change(input, level, 0);
            assert_eq!(got, want, "input: {input:?} at {level:?}");
        }
    }

    #[test]
    fn idempotent_over_all_rows() {
        for (input, level, _) in table() {
            let once = apply_mind_change(input, level, 0);
            let twice = apply_mind_change(&once, level, 0);
            assert_eq!(twice, once, "not a fixed point: {input:?} at {level:?}");
        }
    }

    #[test]
    fn live_tail_guard_defers_edits_near_the_end() {
        // The newest words are still volatile during live ticks: an edit whose
        // decision examined them is deferred (ITN has the same behavior).
        assert_eq!(
            apply_mind_change("at eight, no wait, nine", Light, 2),
            "at eight, no wait, nine"
        );
        assert_eq!(
            apply_mind_change("at eight, no wait, nine", Light, 0),
            "at nine"
        );
        // With two more words after the correction the edit fires even live.
        assert_eq!(
            apply_mind_change("at eight, no wait, nine works fine", Light, 2),
            "at nine works fine"
        );
        // A standalone retraction at the very tail is also held back.
        assert_eq!(
            apply_mind_change("We meet at nine. Scratch that", Light, 2),
            "We meet at nine. Scratch that"
        );
        assert_eq!(
            apply_mind_change("We meet at nine. Scratch that", Light, 0),
            ""
        );
        // The cross-sentence merge is held back while B sits in the tail...
        assert_eq!(
            apply_mind_change("So let's meet at 8. No wait, 9", Light, 2),
            "So let's meet at 8. No wait, 9"
        );
        // ...and fires once enough words follow (a live rewrite of committed
        // text; the injector freeze + stitch fallback own that downstream).
        assert_eq!(
            apply_mind_change("So let's meet at 8. No wait, 9 works fine", Light, 2),
            "So let's meet at 9 works fine"
        );
    }

    #[test]
    fn multiline_segments_are_never_edited() {
        // A spliced custom-phrase template is saved text, not speech; even if
        // it contains cue words it stays byte-identical.
        let template = "Hi team,\n\nno wait, nine\n\nThanks";
        assert_eq!(apply_mind_change(template, High, 0), template);
    }

    #[test]
    fn sentence_scope_blocks_cross_terminator_replacements() {
        // Sanctioned crossing (b) covers a cue that OPENS a sentence; a cue
        // buried mid-sentence still resolves only within its own sentence and
        // never reaches backwards across the terminator.
        assert_eq!(
            apply_mind_change("We meet at eight. Come at seven, no wait, six.", High, 0),
            "We meet at eight. Come at six."
        );
        // And the leading-cue merge itself fires only through the alignment
        // rules; an unresolvable B leaves both sentences byte-identical.
        let text = "We meet at eight. No wait, everyone is busy.";
        assert_eq!(apply_mind_change(text, High, 0), text);
    }

    #[test]
    fn fixpoint_never_loops() {
        // Dense cue soup must terminate within the cap and stay stable.
        let soup = "no wait, no wait, no wait, actually, sorry, at eight, no wait, nine";
        let out = apply_mind_change(soup, High, 0);
        assert_eq!(apply_mind_change(&out, High, 0), out);
    }
}
