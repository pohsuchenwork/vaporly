//! Deterministic inverse text normalization: spoken number forms become
//! written ones ("eight am" -> "8AM", "one hundred and eighty" -> "180",
//! "twenty percent" -> "20%"). Runs on EVERY dictation, on every model, in
//! both streamlining modes, so the formatting is identical everywhere.
//!
//! Design constraints (see the wiring comment in transcription.rs):
//! - Idempotent: outputs are fixed points, because the live pipeline re-runs
//!   over the same growing text every tick.
//! - Never crosses a sentence terminator; context checks stay same-sentence.
//! - `protect_tail_words` skips matches touching the newest words so a number
//!   phrase still being spoken cannot rewrite already-emitted live text.
//! - Precision over recall: homophones (for/to/too/won/ate) are simply not in
//!   the vocabulary; ambiguous juxtapositions ("twenty thirty") are left
//!   verbatim; ordinals and years are the streamlining LLM's job.

use super::text::{extract_punctuation, is_terminator_suffix};

/// A matcher outcome: replace a span, or consume it verbatim (used for
/// ambiguous shapes like a context-less "eight thirty" so the cardinal
/// matcher cannot half-convert it into "eight 30").
enum Matched {
    Replace(String, usize),
    Skip(usize),
}

/// One whitespace token, split into punctuation shell and lowercase core.
struct Tok<'a> {
    raw: &'a str,
    prefix: &'a str,
    core: String,
    suffix: &'a str,
}

fn tokenize(text: &str) -> Vec<Tok<'_>> {
    text.split_whitespace()
        .map(|raw| {
            let (prefix, suffix) = extract_punctuation(raw);
            let core = raw[prefix.len()..raw.len() - suffix.len()].to_lowercase();
            Tok {
                raw,
                prefix,
                core,
                suffix,
            }
        })
        .collect()
}

fn unit_value(w: &str) -> Option<u64> {
    Some(match w {
        "zero" => 0,
        "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        _ => return None,
    })
}

fn teen_value(w: &str) -> Option<u64> {
    Some(match w {
        "ten" => 10,
        "eleven" => 11,
        "twelve" => 12,
        "thirteen" => 13,
        "fourteen" => 14,
        "fifteen" => 15,
        "sixteen" => 16,
        "seventeen" => 17,
        "eighteen" => 18,
        "nineteen" => 19,
        _ => return None,
    })
}

fn tens_value(w: &str) -> Option<u64> {
    Some(match w {
        "twenty" => 20,
        "thirty" => 30,
        "forty" => 40,
        "fifty" => 50,
        "sixty" => 60,
        "seventy" => 70,
        "eighty" => 80,
        "ninety" => 90,
        _ => return None,
    })
}

/// Whether a lowercase core is a spoken number word. Shared with the
/// mind-change resolver's Number class (`audio_toolkit::mind_change`).
pub(crate) fn is_number_word(w: &str) -> bool {
    unit_value(w).is_some()
        || teen_value(w).is_some()
        || tens_value(w).is_some()
        || w == "hundred"
        || w == "thousand"
}

/// Lone "one" is usually a pronoun, not a quantity.
fn one_is_pronoun(cores: &[String], i: usize, consumed: usize) -> bool {
    if consumed != 1 {
        return false;
    }
    let prev = i.checked_sub(1).map(|p| cores[p].as_str());
    // Time idiom: "quarter past eight", "half past nine" stay verbatim.
    if prev == Some("past") {
        return true;
    }
    if cores[i] != "one" {
        return false;
    }
    let next = cores.get(i + 1).map(|s| s.as_str());
    matches!(
        prev,
        Some("no" | "some" | "any" | "every" | "which" | "the")
    ) || matches!(next, Some("of" | "another"))
        || (prev == Some("at") && next == Some("point"))
}

/// Parse a spoken cardinal starting at `i` (word cores, possibly hyphenated
/// words pre-split by the caller). Returns (value, tokens consumed).
/// "a" counts as one only directly before hundred/thousand. "and" is consumed
/// only inside a compound when a number word follows.
fn parse_cardinal(cores: &[String], i: usize) -> Option<(u64, usize)> {
    let mut idx = i;
    let mut total: u64 = 0;
    let mut section: u64 = 0;
    let mut any = false;

    // Leading "a hundred"/"a thousand".
    if cores.get(idx).map(|s| s.as_str()) == Some("a")
        && matches!(
            cores.get(idx + 1).map(|s| s.as_str()),
            Some("hundred" | "thousand")
        )
    {
        section = 1;
        idx += 1;
        any = true;
    }

    loop {
        let Some(w) = cores.get(idx).map(|s| s.as_str()) else {
            break;
        };
        if let Some(v) = tens_value(w) {
            section += v;
            idx += 1;
            any = true;
            if let Some(u) = cores.get(idx).and_then(|s| unit_value(s)) {
                if u > 0 {
                    section += u;
                    idx += 1;
                }
            }
        } else if let Some(v) = teen_value(w) {
            section += v;
            idx += 1;
            any = true;
        } else if let Some(v) = unit_value(w) {
            section += v;
            idx += 1;
            any = true;
        } else if w == "hundred" && any && section > 0 && section < 10 {
            section *= 100;
            idx += 1;
            // "one hundred AND eighty"
            if cores.get(idx).map(|s| s.as_str()) == Some("and")
                && cores.get(idx + 1).is_some_and(|n| {
                    tens_value(n).is_some() || teen_value(n).is_some() || unit_value(n).is_some()
                })
            {
                idx += 1;
            }
            continue;
        } else if w == "thousand" && any && section > 0 && section <= 999 {
            total += section * 1000;
            section = 0;
            idx += 1;
            if cores.get(idx).map(|s| s.as_str()) == Some("and")
                && cores.get(idx + 1).is_some_and(|n| {
                    tens_value(n).is_some() || teen_value(n).is_some() || unit_value(n).is_some()
                })
            {
                idx += 1;
            }
            continue;
        } else {
            break;
        }
        // After consuming a unit/teen/tens, a scale word may follow; loop.
        if !matches!(
            cores.get(idx).map(|s| s.as_str()),
            Some("hundred" | "thousand")
        ) {
            break;
        }
    }

    if !any {
        return None;
    }
    Some((total + section, idx - i))
}

/// am/pm in any spoken or written shape at `i`: (uppercase form, consumed).
fn match_ampm(cores: &[String], i: usize) -> Option<(&'static str, usize)> {
    match cores.get(i).map(|s| s.as_str()) {
        Some("am" | "a.m" | "a.m.") => Some(("AM", 1)),
        Some("pm" | "p.m" | "p.m.") => Some(("PM", 1)),
        Some("a") if cores.get(i + 1).map(|s| s.as_str()) == Some("m") => Some(("AM", 2)),
        Some("p") if cores.get(i + 1).map(|s| s.as_str()) == Some("m") => Some(("PM", 2)),
        _ => None,
    }
}

/// Spoken minutes right after an hour: "thirty" -> 30, "forty five" -> 45,
/// "oh five" -> 05, "fifteen" -> 15. Returns (minutes, consumed).
fn match_minutes(cores: &[String], i: usize) -> Option<(u64, usize)> {
    let w = cores.get(i)?.as_str();
    if w == "oh" || w == "o" {
        let u = cores.get(i + 1).and_then(|s| unit_value(s))?;
        return Some((u, 2));
    }
    if let Some(t) = teen_value(w) {
        return Some((t, 1));
    }
    if let Some(t) = tens_value(w) {
        if let Some(u) = cores.get(i + 1).and_then(|s| unit_value(s)) {
            if u > 0 {
                return Some((t + u, 2));
            }
        }
        return Some((t, 1));
    }
    None
}

fn spoken_hour(cores: &[String], i: usize) -> Option<u64> {
    let w = cores.get(i)?.as_str();
    unit_value(w)
        .filter(|v| (1..=9).contains(v))
        .or_else(|| teen_value(w).filter(|v| *v <= 12))
}

const TIME_CONTEXT: &[&str] = &[
    "at", "around", "by", "until", "till", "before", "after", "from",
];

/// Units and countable nouns that make a number read as a measurement, so it
/// stays a digit even when it is small ("5 minutes", "3 files").
const UNIT_WORDS: &[&str] = &[
    "second",
    "seconds",
    "minute",
    "minutes",
    "hour",
    "hours",
    "day",
    "days",
    "week",
    "weeks",
    "month",
    "months",
    "year",
    "years",
    "km",
    "kilometre",
    "kilometres",
    "kilometer",
    "kilometers",
    "m",
    "metre",
    "metres",
    "meter",
    "meters",
    "cm",
    "mm",
    "mile",
    "miles",
    "ft",
    "feet",
    "inch",
    "inches",
    "kg",
    "kilogram",
    "kilograms",
    "lb",
    "lbs",
    "pound",
    "pounds",
    "oz",
    "ounce",
    "ounces",
    "gram",
    "grams",
    "ml",
    "litre",
    "litres",
    "liter",
    "liters",
    "kb",
    "mb",
    "gb",
    "tb",
    "px",
    "pt",
    "hz",
    "khz",
    "mhz",
    "ghz",
    "degrees",
    "percent",
    "times",
    "x",
    "files",
    "file",
    "items",
    "item",
    "people",
    "copies",
    "pages",
    "page",
    "words",
    "characters",
    "chars",
];

/// Nouns that turn a following number into an identifier ("version 3",
/// "step 2"), where a digit is the natural form.
const IDENTIFIER_NOUNS: &[&str] = &[
    "version", "v", "chapter", "step", "page", "figure", "table", "room", "part", "phase", "round",
    "level", "tier", "line", "item", "number", "no", "issue", "pr", "ticket", "build", "grade",
    "week", "day", "q", "quarter",
];

fn is_unit_word(w: &str) -> bool {
    UNIT_WORDS.contains(&w)
}

fn is_identifier_noun(w: &str) -> bool {
    IDENTIFIER_NOUNS.contains(&w)
}

/// Spell a single digit back out for prose.
fn digit_to_word(d: char) -> Option<&'static str> {
    Some(match d {
        '1' => "one",
        '2' => "two",
        '3' => "three",
        '4' => "four",
        '5' => "five",
        '6' => "six",
        '7' => "seven",
        '8' => "eight",
        '9' => "nine",
        _ => return None,
    })
}

fn digit_time(core: &str) -> Option<String> {
    // "8", "12", "6:45" (1-2 digit hour, optional :MM)
    let (h, rest) = match core.find(':') {
        Some(pos) => (&core[..pos], Some(&core[pos + 1..])),
        None => (core, None),
    };
    if h.is_empty() || h.len() > 2 || !h.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let hv: u64 = h.parse().ok()?;
    if !(1..=12).contains(&hv) {
        return None;
    }
    if let Some(m) = rest {
        if m.len() != 2 || !m.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        return Some(format!("{hv}:{m}"));
    }
    Some(hv.to_string())
}

/// The full scanner. `protect_tail_words` shields the newest words from
/// conversion (live ticks pass 2; final text passes 0).
pub fn apply_itn(text: &str, protect_tail_words: usize) -> String {
    // Multi-line templates (a spliced-in custom phrase) must keep their line
    // breaks: the scanner tokenizes with split_whitespace and rejoins with a
    // single space, so a newline fed straight in would collapse. Process each
    // line on its own and rejoin with '\n'. Only the LAST line carries words
    // that may still be growing, so protect_tail_words applies there alone;
    // earlier lines are already terminated by their break and convert fully.
    if !text.contains('\n') {
        return apply_itn_line(text, protect_tail_words);
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let last = lines.len() - 1;
    lines
        .iter()
        .enumerate()
        .map(|(idx, line)| apply_itn_line(line, if idx == last { protect_tail_words } else { 0 }))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Spell standalone small digits back out as words, so prose reads the same
/// no matter which spelling the recognizer happened to emit.
///
/// The speech model is inconsistent: the same dictation can produce "one one
/// problem" and "if you say 1". `apply_itn` fixes the word-to-digit direction;
/// this fixes digit-to-word, and together they make the output stable. Only a
/// bare digit 1-9 in plain prose is touched. Anything that reads as data keeps
/// its digits: times, money, percentages, decimals, fractions, measurements,
/// identifiers ("version 3"), list numbering, and digit runs.
pub fn apply_number_prose(text: &str, protect_tail_words: usize) -> String {
    if !text.contains('\n') {
        return number_prose_line(text, protect_tail_words);
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let last = lines.len() - 1;
    lines
        .iter()
        .enumerate()
        .map(|(idx, line)| {
            number_prose_line(line, if idx == last { protect_tail_words } else { 0 })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn number_prose_line(text: &str, protect_tail_words: usize) -> String {
    let toks = tokenize(text);
    if toks.is_empty() {
        return text.to_string();
    }
    let limit = toks.len().saturating_sub(protect_tail_words);
    // Sentence spans, so a number can be judged against its own sentence only.
    let mut sentence_of: Vec<usize> = Vec::with_capacity(toks.len());
    let mut s = 0usize;
    for tok in toks.iter() {
        sentence_of.push(s);
        if is_terminator_suffix(tok.raw) {
            s += 1;
        }
    }
    // Whether this token, judged on its own, is a lone small number in prose.
    let spellable = |i: usize| -> bool {
        let tok = &toks[i];
        let is_lone_digit = tok.core.len() == 1
            && tok
                .core
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit() && c != '0');
        if !is_lone_digit {
            return false;
        }
        // A currency or percent shell, a decimal point, a colon: all mean the
        // token is data even though its core is one digit.
        let bare_shell = tok.prefix.is_empty()
            && tok
                .suffix
                .chars()
                .all(|c| matches!(c, '.' | ',' | '!' | '?' | ';' | ':' | ')' | '"' | '\''));
        let prev = i.checked_sub(1).map(|p| toks[p].core.as_str());
        let next = toks.get(i + 1).map(|t| t.core.as_str());
        let neighbours_numeric = prev.is_some_and(|w| w.chars().any(|c| c.is_ascii_digit()))
            || next.is_some_and(|w| w.chars().any(|c| c.is_ascii_digit()));
        bare_shell
            && !neighbours_numeric
            && !next.is_some_and(is_unit_word)
            && !prev.is_some_and(is_identifier_noun)
            && !prev.is_some_and(|w| TIME_CONTEXT.contains(&w))
    };
    // A sentence that carries a DATA number ("at 8, no wait, 9") keeps digits
    // throughout: spelling only some of them out is the very inconsistency
    // this pass exists to remove. Sentences whose only numbers are lone small
    // ones ("say 1, it becomes 1") spell all of them out together.
    let has_data_number = |i: usize| -> bool {
        toks.iter().enumerate().any(|(j, t)| {
            j != i
                && sentence_of[j] == sentence_of[i]
                && t.core.chars().any(|c| c.is_ascii_digit())
                && !spellable(j)
        })
    };
    let mut out: Vec<String> = Vec::with_capacity(toks.len());
    let mut sentence_start = true;
    for (i, tok) in toks.iter().enumerate() {
        let keep = tok.raw.to_string();
        // A leading "1." or "1)" is list numbering, not prose.
        let listish = sentence_start && tok.suffix.starts_with(['.', ')']);

        let convert = i < limit && spellable(i) && !listish && !has_data_number(i);

        if convert {
            let word = tok
                .core
                .chars()
                .next()
                .and_then(digit_to_word)
                .unwrap_or_default();
            let word = if sentence_start {
                let mut c = word.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            } else {
                word.to_string()
            };
            out.push(format!("{}{}{}", tok.prefix, word, tok.suffix));
        } else {
            out.push(keep);
        }
        sentence_start = is_terminator_suffix(tok.suffix);
    }
    out.join(" ")
}

fn apply_itn_line(text: &str, protect_tail_words: usize) -> String {
    let toks = tokenize(text);
    if toks.is_empty() {
        return text.to_string();
    }
    // Hyphenated number words split for matching ("twenty-five"): build a
    // parallel core list where such tokens expand, tracking the mapping back.
    // Simpler: treat the hyphen case inside parse by normalizing the core.
    let cores: Vec<String> = toks
        .iter()
        .map(|t| {
            t.core
                .replace('-', " ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    // Cores with internal spaces (from hyphens) are re-split into a flat list
    // with an index map so spans still land on token boundaries.
    let mut flat: Vec<String> = Vec::new();
    let mut tok_of_flat: Vec<usize> = Vec::new();
    for (ti, c) in cores.iter().enumerate() {
        if c.contains(' ') {
            for part in c.split(' ') {
                flat.push(part.to_string());
                tok_of_flat.push(ti);
            }
        } else {
            flat.push(c.clone());
            tok_of_flat.push(ti);
        }
    }

    let limit = flat.len().saturating_sub(protect_tail_words);
    let terminator_before = |flat_start: usize, flat_end: usize| -> bool {
        // Any SOURCE token wholly inside the span except the last may not
        // carry a sentence terminator.
        let last_tok = tok_of_flat[flat_end - 1];
        (flat_start..flat_end - 1)
            .map(|f| tok_of_flat[f])
            .any(|ti| ti != last_tok && is_terminator_suffix(toks[ti].raw))
    };

    let mut out: Vec<String> = Vec::new();
    let mut i = 0; // flat index
    let mut emitted_tok = usize::MAX; // last source token already emitted
                                      // Once a sentence has written one number as digits, later numbers in that
                                      // same sentence follow suit: "at eight, no wait, nine" must not come out
                                      // as "at 8, no wait, nine". Reset at every sentence terminator.
    let mut digits_in_sentence = false;

    let emit_verbatim = |out: &mut Vec<String>, emitted_tok: &mut usize, ti: usize| {
        if *emitted_tok == ti {
            return; // hyphen-split parts share one source token
        }
        out.push(toks[ti].raw.to_string());
        *emitted_tok = ti;
    };

    while i < flat.len() {
        let ti = tok_of_flat[i];
        // A replacement span must start at a token boundary (not mid-hyphen).
        let at_token_start = i == 0 || tok_of_flat[i - 1] != ti;

        let mut replaced = false;
        if at_token_start && i < limit {
            // Matchers only ever see the current sentence: truncate at the
            // first terminator-carrying token (inclusive).
            let sent_end = (i..flat.len())
                .find(|&j| is_terminator_suffix(toks[tok_of_flat[j]].raw))
                .map(|j| j + 1)
                .unwrap_or(flat.len());
            match try_match_with(&flat[..sent_end], i, digits_in_sentence) {
                Some(Matched::Replace(rep, consumed)) => {
                    let end = i + consumed;
                    let end_tok = tok_of_flat[end - 1];
                    let ends_at_boundary = end == flat.len() || tok_of_flat[end] != end_tok;
                    if end <= limit && ends_at_boundary && !terminator_before(i, end) {
                        let prefix = toks[ti].prefix;
                        let suffix = toks[end_tok].suffix;
                        out.push(format!("{prefix}{rep}{suffix}"));
                        emitted_tok = end_tok;
                        i = end;
                        replaced = true;
                        digits_in_sentence = true;
                    }
                }
                Some(Matched::Skip(consumed)) => {
                    let end = (i + consumed).min(flat.len());
                    if end <= limit {
                        let mut j = i;
                        while j < end {
                            let tj = tok_of_flat[j];
                            emit_verbatim(&mut out, &mut emitted_tok, tj);
                            j += 1;
                            while j < flat.len() && tok_of_flat[j] == tj {
                                j += 1;
                            }
                        }
                        i = end;
                        replaced = true;
                    }
                }
                None => {}
            }
        }
        if !replaced {
            emit_verbatim(&mut out, &mut emitted_tok, ti);
            i += 1;
            // skip the rest of this source token's flat parts
            while i < flat.len() && tok_of_flat[i] == ti {
                i += 1;
            }
        }
        // A new sentence starts fresh: its own numbers decide its own form.
        if is_terminator_suffix(toks[tok_of_flat[i.min(flat.len()) - 1]].raw) {
            digits_in_sentence = false;
        }
    }

    out.join(" ")
}

#[cfg(test)]
fn try_match(flat: &[String], i: usize) -> Option<Matched> {
    try_match_with(flat, i, false)
}

/// Try every matcher at flat position `i`. `flat` is pre-truncated to the
/// current sentence, so no matcher can see or consume across a terminator.
/// `digits_in_sentence` records whether an earlier number in THIS sentence was
/// already written as digits, so the sentence stays internally consistent.
fn try_match_with(flat: &[String], i: usize, digits_in_sentence: bool) -> Option<Matched> {
    let w = flat[i].as_str();

    // ---- M1 time: digit-led normalization ----
    if let Some(t) = digit_time(w) {
        if let Some((ap, used)) = match_ampm(flat, i + 1) {
            return Some(Matched::Replace(format!("{t}{ap}"), 1 + used));
        }
    }
    // Single token "8am"/"6:45pm" (any case) normalizes.
    if let Some(pos) = w.find(|c: char| c.is_ascii_alphabetic()) {
        let (num, ap) = w.split_at(pos);
        if matches!(ap, "am" | "pm") {
            if let Some(t) = digit_time(num) {
                return Some(Matched::Replace(format!("{t}{}", ap.to_uppercase()), 1));
            }
        }
    }

    // ---- M1 time: spoken hour ----
    if let Some(h) = spoken_hour(flat, i) {
        // hour + am/pm
        if let Some((ap, used)) = match_ampm(flat, i + 1) {
            return Some(Matched::Replace(format!("{h}{ap}"), 1 + used));
        }
        // hour + minutes [+ am/pm]
        if let Some((m, mused)) = match_minutes(flat, i + 1) {
            if let Some((ap, aused)) = match_ampm(flat, i + 1 + mused) {
                return Some(Matched::Replace(
                    format!("{h}:{m:02}{ap}"),
                    1 + mused + aused,
                ));
            }
            // Bare pair: only with same-sentence time context; otherwise
            // consume BOTH words verbatim so the cardinal matcher cannot
            // half-convert the pair into "eight 30".
            let prev_ok = i
                .checked_sub(1)
                .map(|p| TIME_CONTEXT.contains(&flat[p].as_str()))
                .unwrap_or(false);
            let next_ok = flat
                .get(i + 1 + mused)
                .is_some_and(|n| TIME_CONTEXT.contains(&n.as_str()));
            if prev_ok || next_ok {
                return Some(Matched::Replace(format!("{h}:{m:02}"), 1 + mused));
            }
            return Some(Matched::Skip(1 + mused));
        }
        // hour + o'clock
        if matches!(
            flat.get(i + 1).map(|s| s.as_str()),
            Some("oclock" | "o'clock")
        ) {
            return Some(Matched::Replace(format!("{h}:00"), 2));
        }
    }

    // ---- M2/M3/M4/M5/M6: number-led ----
    let (int_val, int_used) = parse_cardinal(flat, i)?;
    if one_is_pronoun(flat, i, int_used) {
        return None;
    }

    // Decimal: N point d [d ...], repeated for dotted forms like a version
    // number ("three point zero point one" -> "3.0.1"). Without the repeat the
    // second "point" fell through to the juxtaposition guard, which consumed
    // the whole run verbatim and left the version spelled out.
    let mut value_str = int_val.to_string();
    let mut used = int_used;
    loop {
        if flat.get(i + used).map(|s| s.as_str()) != Some("point") {
            break;
        }
        let mut digits = String::new();
        let mut j = i + used + 1;
        while let Some(d) = flat.get(j).and_then(|s| {
            if s == "oh" || s == "o" {
                Some(0)
            } else {
                unit_value(s)
            }
        }) {
            digits.push_str(&d.to_string());
            j += 1;
        }
        if digits.is_empty() {
            break;
        }
        value_str = format!("{value_str}.{digits}");
        used = j - i;
    }

    // Percent
    if matches!(flat.get(i + used).map(|s| s.as_str()), Some("percent")) {
        return Some(Matched::Replace(format!("{value_str}%"), used + 1));
    }

    // Money
    match flat.get(i + used).map(|s| s.as_str()) {
        Some("dollars" | "dollar" | "bucks" | "buck") => {
            let mut total_used = used + 1;
            // "and fifty cents"
            if flat.get(i + total_used).map(|s| s.as_str()) == Some("and") {
                if let Some((cents, cused)) = parse_cardinal(flat, i + total_used + 1) {
                    if matches!(
                        flat.get(i + total_used + 1 + cused).map(|s| s.as_str()),
                        Some("cents" | "cent")
                    ) && cents < 100
                        && !value_str.contains('.')
                    {
                        return Some(Matched::Replace(
                            format!("${value_str}.{cents:02}"),
                            total_used + 1 + cused + 1,
                        ));
                    }
                }
            }
            let _ = &mut total_used;
            return Some(Matched::Replace(format!("${value_str}"), used + 1));
        }
        Some("cents" | "cent") if !value_str.contains('.') => {
            return Some(Matched::Replace(format!("{value_str} cents"), used + 1));
        }
        _ => {}
    }

    // Fractions: numeric numerator 1-9 + denominator word.
    if int_used == 1 && !value_str.contains('.') && (1..=9).contains(&int_val) {
        if let Some(den) = flat.get(i + 1).map(|s| s.as_str()) {
            let denom = match den {
                "half" | "halves" => Some(2),
                "third" | "thirds" => Some(3),
                "quarter" | "quarters" => Some(4),
                "fifth" | "fifths" => Some(5),
                "sixth" | "sixths" => Some(6),
                "seventh" | "sevenths" => Some(7),
                "eighth" | "eighths" => Some(8),
                "ninth" | "ninths" => Some(9),
                "tenth" | "tenths" => Some(10),
                _ => None,
            };
            if let Some(d) = denom {
                let plural_ok = if int_val > 1 {
                    den.ends_with('s')
                } else {
                    !den.ends_with('s')
                };
                if plural_ok {
                    return Some(Matched::Replace(format!("{int_val}/{d}"), 2));
                }
            }
        }
    }

    // Plain cardinal or decimal. Juxtaposition guard: a following standalone
    // number word that could not merge ("twenty thirty", "five five") reads
    // as a year or digit string, which stays the LLM's job. `flat` is
    // sentence-truncated, so cross-sentence numbers never trigger this.
    if flat
        .get(i + used)
        .is_some_and(|n| is_number_word(n) || n == "point")
    {
        // Consume the whole ambiguous run verbatim so a later position
        // cannot half-convert its tail ("twenty thirty" -> "twenty 30").
        let mut j = i + used;
        while flat
            .get(j)
            .is_some_and(|n| is_number_word(n) || n == "point")
        {
            j += 1;
        }
        return Some(Matched::Skip(j - i));
    }

    // Prose convention: small numbers are spelled out ("I only need one
    // thing"), digits are for quantities that read as data. Everything with a
    // real numeric context already returned above (times, money, percent,
    // fractions, decimals), so what reaches here is a bare cardinal. Skip it
    // when it is a lone small value with no measuring context, leaving the
    // spoken word untouched.
    if used == 1 && int_used == 1 && value_str.len() == 1 && !digits_in_sentence {
        let follows_time_word = i
            .checked_sub(1)
            .and_then(|p| flat.get(p))
            .is_some_and(|w| TIME_CONTEXT.contains(&w.as_str()));
        let precedes_unit = flat.get(i + used).is_some_and(|w| is_unit_word(w));
        if !follows_time_word && !precedes_unit {
            return Some(Matched::Skip(used));
        }
    }
    Some(Matched::Replace(value_str, used))
}

#[cfg(test)]
mod itn_tests {
    use super::apply_itn;

    fn itn(s: &str) -> String {
        apply_itn(s, 0)
    }

    #[test]
    fn conversion_table() {
        let cases = [
            // times
            ("meet at eight am", "meet at 8AM"),
            ("meet at eight a m", "meet at 8AM"),
            ("meet at eight a.m.", "meet at 8AM."),
            ("eight thirty pm works", "8:30PM works"),
            ("six forty five pm", "6:45PM"),
            ("eight oh five am", "8:05AM"),
            ("8 am", "8AM"),
            ("8 AM", "8AM"),
            ("8 a.m.", "8AM."),
            ("6:45 p.m.", "6:45PM."),
            ("8am sharp", "8AM sharp"),
            ("6:45pm sharp", "6:45PM sharp"),
            ("eight oclock", "8:00"),
            ("eight o'clock", "8:00"),
            ("at eight thirty", "at 8:30"),
            ("by eight thirty tonight", "by 8:30 tonight"),
            ("eight thirty until nine", "8:30 until 9"),
            // money
            ("nine hundred dollars", "$900"),
            ("twenty five bucks", "$25"),
            ("one dollar", "$1"),
            ("nine dollars and fifty cents", "$9.50"),
            ("fifty cents", "50 cents"),
            // percent
            ("twenty percent", "20%"),
            ("six point five percent", "6.5%"),
            // fractions
            ("one half", "1/2"),
            ("two thirds", "2/3"),
            ("three quarters", "3/4"),
            // decimals
            ("six point five", "6.5"),
            ("three point one four", "3.14"),
            // Dotted forms: a version number keeps every group.
            ("version three point zero point one", "version 3.0.1"),
            ("three point zero point one", "3.0.1"),
            // cardinals: 10 and up are digits, and a small number keeps its
            // digits when it measures something or follows a time word.
            ("ninety nine", "99"),
            ("one hundred and eighty", "180"),
            ("nine hundred", "900"),
            ("twelve thousand", "12000"),
            ("a hundred", "100"),
            ("twenty-five files", "25 files"),
            ("we need nine copies", "we need 9 copies"),
            ("five minutes", "5 minutes"),
            ("meet at eight", "meet at 8"),
        ];
        for (input, want) in cases {
            assert_eq!(itn(input), want, "input: {input}");
        }
    }

    #[test]
    fn guard_table() {
        let cases = [
            "this is for you",
            "give it to me",
            "that is too much",
            "we won the game",
            "they ate lunch",
            "no one showed up",
            "one of the best",
            "which one is it",
            "the one thing that matters",
            "one another",
            "at one point I left",
            "a couple of days",
            "a few things",
            "several people",
            "hundreds of files",
            "thousands of users",
            "half an hour",
            "a quarter past",
            "quarter past eight",
            "eight thirty",
            "twenty thirty was the deadline year",
            "five five five one two one two",
        ];
        for input in cases {
            assert_eq!(itn(input), input, "must stay verbatim: {input}");
        }
    }

    #[test]
    fn never_crosses_sentence_terminators() {
        assert_eq!(
            itn("The price is one hundred. Fifty people came."),
            "The price is 100. 50 people came."
        );
        // "eight" is a lone small number in prose so it stays spelled, while
        // "Thirty" is 10 or more and still converts: the sentence terminator
        // between them is never crossed either way.
        assert_eq!(itn("I said eight. Thirty came."), "I said eight. 30 came.");
    }

    #[test]
    fn small_numbers_stay_words_in_prose() {
        // The complaint: every spoken number became a digit.
        assert_eq!(itn("I only need one thing"), "I only need one thing");
        assert_eq!(itn("give me five"), "give me five");
        assert_eq!(itn("nine"), "nine");
        // ...but a number that measures, times, or identifies keeps digits.
        assert_eq!(itn("five minutes"), "5 minutes");
        assert_eq!(itn("we need nine copies"), "we need 9 copies");
        assert_eq!(itn("call me at nine"), "call me at 9");
        assert_eq!(itn("by eight tonight"), "by 8 tonight");
    }

    #[test]
    fn number_prose_spells_lone_small_digits() {
        let prose = |s: &str| super::apply_number_prose(s, 0);
        // The recognizer emits digits inconsistently; normalize them back.
        assert_eq!(
            prose("if you say 1 it turns into 1"),
            "if you say one it turns into one"
        );
        assert_eq!(prose("I need 3 of them"), "I need three of them");
        // Sentence-initial digits get capitalized like any other word.
        assert_eq!(prose("3 seemed fine."), "Three seemed fine.");
    }

    #[test]
    fn number_prose_keeps_data_as_digits() {
        let prose = |s: &str| super::apply_number_prose(s, 0);
        for text in [
            "meet at 9:30",       // time
            "it costs $5",        // money
            "about 5% of them",   // percent
            "roughly 6.5 hours",  // decimal
            "5 minutes late",     // measurement
            "version 3 shipped",  // identifier
            "call 5 5 5 1 2 1 2", // digit run
            "we need 12 copies",  // 10 and up
            "meet at 8 tonight",  // time context
        ] {
            assert_eq!(prose(text), text, "must keep digits: {text}");
        }
    }

    #[test]
    fn number_prose_is_idempotent_and_guards_the_live_tail() {
        let prose = |s: &str| super::apply_number_prose(s, 0);
        let once = prose("I need 3 of them");
        assert_eq!(prose(&once), once, "must be a fixed point");
        // The newest words are still growing: leave them alone so the live
        // preview does not flicker between "3" and "three".
        assert_eq!(super::apply_number_prose("I need 3", 2), "I need 3");
    }

    #[test]
    fn punctuation_reattaches() {
        assert_eq!(itn("(eight am)"), "(8AM)");
        assert_eq!(itn("really, nine hundred dollars!"), "really, $900!");
    }

    #[test]
    fn tail_guard_protects_live_text() {
        assert_eq!(apply_itn("costs nine hundred", 2), "costs nine hundred");
        assert_eq!(apply_itn("costs nine hundred", 0), "costs 900");
        assert_eq!(
            apply_itn("meet at eight am ok then", 2),
            "meet at 8AM ok then",
            "protected words are the LAST two only"
        );
    }

    #[test]
    fn idempotent_over_all_outputs() {
        let inputs = [
            "meet at six forty five pm to review twenty percent of one hundred and eighty files",
            "nine dollars and fifty cents for three quarters of it at 8 am",
        ];
        for input in inputs {
            let once = itn(input);
            assert_eq!(itn(&once), once, "not a fixed point: {once}");
        }
    }

    #[test]
    fn mixed_sentence() {
        assert_eq!(
            itn("meet at six forty five pm to review twenty percent of one hundred and eighty files"),
            "meet at 6:45PM to review 20% of 180 files"
        );
    }

    #[test]
    fn newlines_survive() {
        // A multi-line custom-phrase template keeps its breaks; each line still
        // gets ITN applied.
        assert_eq!(
            itn("Hi team,\n\nMeeting at eight am.\n\nThanks"),
            "Hi team,\n\nMeeting at 8AM.\n\nThanks"
        );
        // The tail guard only protects the LAST line's tail, so earlier lines
        // convert fully even under a live protect window.
        assert_eq!(
            apply_itn("costs nine hundred\nthen eight am", 2),
            "costs 900\nthen eight am"
        );
        // A trailing newline is preserved (empty final segment).
        assert_eq!(itn("eight am\n"), "8AM\n");
    }
}
