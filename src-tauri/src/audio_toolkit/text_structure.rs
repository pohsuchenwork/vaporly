//! Final-text block layout (round 21): the additive structure rules that turn
//! a flat dictation into the shape the target app expects. Email gets a
//! greeting / body / sign-off layout; chat and notes get paragraphs separated
//! by blank lines. Deterministic, conservative, and idempotent by contract:
//! text that already carries blank lines, or that shows no recognizable cues,
//! comes back byte-identical (the mind_change.rs "no alignment = no edit"
//! discipline). Runs only on FINAL text; live previews never see it.

use super::text::{complete_sentence_ranges, normalize_sentence_caps};

/// Greeting openers, longest first so "hi there" wins over "hi".
const GREETING_CUES: &[&str] = &[
    "good morning",
    "good afternoon",
    "good evening",
    "hi there",
    "hey there",
    "greetings",
    "hello",
    "dear",
    "hey",
    "hi",
];

/// Sign-off phrases, longest first so "best regards" wins over "best".
const SIGNOFF_CUES: &[&str] = &[
    "thanks so much",
    "many thanks",
    "thank you",
    "best regards",
    "kind regards",
    "warm regards",
    "best wishes",
    "talk soon",
    "sincerely",
    "regards",
    "cheers",
    "warmly",
    "thanks",
    "yours",
    "best",
];

/// Discourse shifts that open a new paragraph, longest first.
const DISCOURSE_CUES: &[&str] = &[
    "on another note",
    "one more thing",
    "another thing",
    "by the way",
    "separately",
    "oh and",
    "anyway",
    "also",
    "plus",
    "btw",
];

/// A greeting or sign-off line never carries more than this many words: it
/// keeps "Hey Sarah," splitting while "Hey how are you doing today" stays put.
const MAX_CUE_HEAD_WORDS: usize = 4;
/// A sign-off name is at most this many words ("John", "John Smith", ...).
const MAX_SIGNOFF_NAME_WORDS: usize = 3;

/// Case-insensitive word-boundary cue match at the start of `s`; returns the
/// matched cue length in bytes.
fn leading_cue(s: &str, cues: &[&str]) -> Option<usize> {
    for cue in cues {
        if s.len() >= cue.len()
            && s.is_char_boundary(cue.len())
            && s[..cue.len()].eq_ignore_ascii_case(cue)
        {
            let rest = &s[cue.len()..];
            if rest.is_empty() || rest.starts_with([' ', ',', '.', '!', '?']) {
                return Some(cue.len());
            }
        }
    }
    None
}

/// Join a greeting cue with its (possibly empty) name: "Hi" + "Sarah" ->
/// "Hi Sarah"; "Hi" + "" -> "Hi".
fn join_cue_name(cue: &str, name: &str) -> String {
    if name.is_empty() {
        cue.to_string()
    } else {
        format!("{cue} {name}")
    }
}

/// Peel a greeting line off the front, returning `(greeting_line, new_body)`.
/// Handles "Hey Sarah, <body>", "Hey Sarah. <body>", and the STT-comma case
/// "Hi, Sarah <body>" where the speech model drops a comma right after the
/// greeting word (the name then follows that comma, not a new body line).
/// `new_body` is empty for a name-only greeting like "Hi, Sarah." -> "Hi Sarah,".
fn peel_greeting(body: &str) -> Option<(String, String)> {
    let cue_len = leading_cue(body, GREETING_CUES)?;
    let ranges = complete_sentence_ranges(body);
    let first_end = ranges.first().map(|r| r.end).unwrap_or(body.len());
    let cue = body[..cue_len].trim().to_string();
    let bytes = body.as_bytes();

    // Advance past the cue, skipping spaces and ONE stray comma right after it.
    let mut pos = cue_len;
    while pos < first_end && bytes[pos] == b' ' {
        pos += 1;
    }
    if pos < first_end && bytes[pos] == b',' {
        pos += 1;
        while pos < first_end && bytes[pos] == b' ' {
            pos += 1;
        }
    }

    let post = &body[pos..first_end];
    if let Some(rel) = post.find(',') {
        // Comma after the name: the body continues on this sentence.
        let name = post[..rel].trim();
        let head = join_cue_name(&cue, name);
        if head.split_whitespace().count() <= MAX_CUE_HEAD_WORDS {
            let new_body = body[pos + rel + 1..].trim_start().to_string();
            if !new_body.is_empty() {
                return Some((format!("{head},"), new_body));
            }
        }
        None
    } else {
        // No comma after the cue: the name is the rest of the first sentence;
        // the body (if any) is the later sentences.
        let name = post.trim_end_matches(['.', '!', '?']).trim();
        let head = join_cue_name(&cue, name);
        if head.split_whitespace().count() <= MAX_CUE_HEAD_WORDS {
            let later = body[first_end..].trim_start().to_string();
            return Some((format!("{head},"), later));
        }
        None
    }
}

/// Uppercase the first letter of `s` (ASCII-simple; names and sign-off words).
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The last sentence of `text` (or its unterminated tail) with its byte start.
fn last_sentence(text: &str) -> (usize, String) {
    let ranges = complete_sentence_ranges(text);
    match ranges.last() {
        Some(r) if text[r.end..].trim().is_empty() => (r.start, text[r.start..].trim().to_string()),
        Some(r) => (r.end, text[r.end..].trim().to_string()),
        None => (0, text.trim().to_string()),
    }
}

/// Reshape a flat email into greeting / body / sign-off blocks:
/// "Hey Sarah, today was a great day. Thanks, John." becomes
/// "Hey Sarah,\n\nToday was a great day.\n\nThanks,\nJohn".
/// Greeting and sign-off are detected independently; no cues means no edit.
pub fn apply_email_structure(text: &str) -> String {
    if text.contains("\n\n") || text.trim().is_empty() {
        return text.to_string();
    }
    let mut body = text.trim().to_string();
    let mut greeting: Option<String> = None;
    let mut signoff: Option<String> = None;

    // ---- greeting: peel "Hey Sarah," / "Hey Sarah." / "Hi, Sarah" off front ----
    if let Some((line, rest)) = peel_greeting(&body) {
        greeting = Some(line);
        body = rest;
    }

    // ---- sign-off: peel "Thanks, John." off the end ----
    let (last_start, last) = last_sentence(&body);
    if last_start > 0 {
        let bare = last.trim_end_matches(['.', '!', '?']).trim();
        if let Some(cue_len) = leading_cue(bare, SIGNOFF_CUES) {
            let after = bare[cue_len..].trim();
            let phrase = capitalize_first(&bare[..cue_len]);
            if after.is_empty() {
                signoff = Some(phrase);
                body = body[..last_start].trim_end().to_string();
            } else if let Some(name) = after.strip_prefix(',') {
                let name = name.trim();
                if !name.is_empty() && name.split_whitespace().count() <= MAX_SIGNOFF_NAME_WORDS {
                    signoff = Some(format!("{},\n{}", phrase, capitalize_first(name)));
                    body = body[..last_start].trim_end().to_string();
                }
            }
        }
    }

    // No cue anywhere means no edit. A name-only greeting ("Hi, Sarah.") is
    // allowed to stand alone, so an empty body no longer bails.
    if greeting.is_none() && signoff.is_none() {
        return text.to_string();
    }

    let body = normalize_sentence_caps(body.trim());
    let mut parts: Vec<String> = Vec::new();
    if let Some(g) = greeting {
        parts.push(g);
    }
    if !body.is_empty() {
        parts.push(body);
    }
    if let Some(s) = signoff {
        parts.push(s);
    }
    parts.join("\n\n")
}

/// Split a flat multi-sentence dictation into paragraphs separated by blank
/// lines: a new paragraph starts at a discourse cue or once the current one
/// holds `max_sentences` sentences. Single-paragraph results come back
/// byte-identical.
pub fn apply_paragraphs(text: &str, max_sentences: usize) -> String {
    if text.contains("\n\n") || max_sentences == 0 {
        return text.to_string();
    }
    let ranges = complete_sentence_ranges(text);
    let mut sentences: Vec<&str> = ranges.iter().map(|r| text[r.clone()].trim()).collect();
    let tail_start = ranges.last().map(|r| r.end).unwrap_or(0);
    let tail = text[tail_start..].trim();
    if !tail.is_empty() {
        sentences.push(tail);
    }
    if sentences.len() < 2 {
        return text.to_string();
    }

    let mut paragraphs: Vec<Vec<&str>> = vec![Vec::new()];
    for sentence in sentences {
        let current = paragraphs.last_mut().expect("always one paragraph");
        let shifts = leading_cue(sentence, DISCOURSE_CUES).is_some();
        if !current.is_empty() && (shifts || current.len() >= max_sentences) {
            paragraphs.push(vec![sentence]);
        } else {
            current.push(sentence);
        }
    }
    if paragraphs.len() < 2 {
        return text.to_string();
    }
    paragraphs
        .iter()
        .map(|p| p.join(" "))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_email(input: &str, expected: &str) {
        let once = apply_email_structure(input);
        assert_eq!(once, expected, "input: {input:?}");
        assert_eq!(
            apply_email_structure(&once),
            once,
            "must be idempotent: {input:?}"
        );
    }

    #[test]
    fn email_table() {
        assert_email(
            "Hey Sarah, today was a great day. Thanks, John.",
            "Hey Sarah,\n\nToday was a great day.\n\nThanks,\nJohn",
        );
        assert_email(
            "Hi Sarah, the report is ready. Thanks.",
            "Hi Sarah,\n\nThe report is ready.\n\nThanks",
        );
        assert_email(
            "Hello team, please review the plan. Best regards, Po.",
            "Hello team,\n\nPlease review the plan.\n\nBest regards,\nPo",
        );
        assert_email(
            "Dear John, welcome aboard. Cheers, Sarah.",
            "Dear John,\n\nWelcome aboard.\n\nCheers,\nSarah",
        );
        // Greeting only.
        assert_email(
            "Hey Sarah, the meeting is at 9.",
            "Hey Sarah,\n\nThe meeting is at 9.",
        );
        // STT drops a comma right after the greeting word: the name must stay
        // on the greeting line, not become the body.
        assert_email("Hi, Sarah.", "Hi Sarah,");
        assert_email(
            "Hi, Sarah. The report is ready.",
            "Hi Sarah,\n\nThe report is ready.",
        );
        // A bare greeting with a name and no body still gets its comma.
        assert_email("Hi Sarah.", "Hi Sarah,");
        // Sign-off only.
        assert_email(
            "The meeting is at 9. Thanks, John.",
            "The meeting is at 9.\n\nThanks,\nJohn",
        );
        // The greeting dictated as its own sentence still peels.
        assert_email(
            "Hey Sarah. Today was a great day. Thanks, John.",
            "Hey Sarah,\n\nToday was a great day.\n\nThanks,\nJohn",
        );
    }

    #[test]
    fn email_no_ops() {
        for text in [
            "The meeting is at 9.",
            // "Regardless" must never read as "regards".
            "Regardless of the plan, we ship Friday.",
            // A long first sentence with a greeting word is not a greeting line.
            "Hey how are you doing today. I hope all is well.",
            // "Thanks for everything" is a body sentence, not a sign-off.
            "The report is done. Thanks for everything you did there.",
            "",
        ] {
            assert_eq!(apply_email_structure(text), text, "must no-op: {text:?}");
        }
        // Already structured text is untouched.
        let done = "Hey Sarah,\n\nAll good.\n\nThanks,\nJohn";
        assert_eq!(apply_email_structure(done), done);
    }

    fn assert_paragraphs(input: &str, max: usize, expected: &str) {
        let once = apply_paragraphs(input, max);
        assert_eq!(once, expected, "input: {input:?}");
        assert_eq!(
            apply_paragraphs(&once, max),
            once,
            "must be idempotent: {input:?}"
        );
    }

    #[test]
    fn paragraph_table() {
        // A discourse cue opens a new paragraph.
        assert_paragraphs(
            "We shipped it. Also the docs are updated.",
            4,
            "We shipped it.\n\nAlso the docs are updated.",
        );
        // The sentence cap opens one too.
        assert_paragraphs(
            "One thing. Two things. Three things. Four things.",
            2,
            "One thing. Two things.\n\nThree things. Four things.",
        );
        // Chat's unterminated tail (dropped period) still groups correctly.
        assert_paragraphs(
            "First thought. Second thought. Third thing no period",
            2,
            "First thought. Second thought.\n\nThird thing no period",
        );
    }

    #[test]
    fn paragraph_no_ops() {
        // Under the cap with no cues: byte-identical.
        for (text, max) in [
            ("One thing. Two things. Three things.", 4usize),
            ("Just one sentence.", 2),
            ("Also short.", 2),
            ("", 2),
        ] {
            assert_eq!(apply_paragraphs(text, max), text, "must no-op: {text:?}");
        }
        // Already structured text is untouched.
        let done = "A block.\n\nAnother block.";
        assert_eq!(apply_paragraphs(done, 2), done);
    }
}
