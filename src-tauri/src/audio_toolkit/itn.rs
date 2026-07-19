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
            match try_match(&flat[..sent_end], i) {
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
    }

    out.join(" ")
}

/// Try every matcher at flat position `i`. `flat` is pre-truncated to the
/// current sentence, so no matcher can see or consume across a terminator.
fn try_match(flat: &[String], i: usize) -> Option<Matched> {
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

    // Decimal: N point d [d ...]
    let mut value_str = int_val.to_string();
    let mut used = int_used;
    if flat.get(i + used).map(|s| s.as_str()) == Some("point") {
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
        if !digits.is_empty() {
            value_str = format!("{int_val}.{digits}");
            used = j - i;
        }
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
            // cardinals
            ("nine", "9"),
            ("ninety nine", "99"),
            ("one hundred and eighty", "180"),
            ("nine hundred", "900"),
            ("twelve thousand", "12000"),
            ("a hundred", "100"),
            ("twenty-five files", "25 files"),
            ("we need nine copies", "we need 9 copies"),
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
        assert_eq!(itn("I said eight. Thirty came."), "I said 8. 30 came.");
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
