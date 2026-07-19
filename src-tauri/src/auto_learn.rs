//! F4: auto-learn custom words.
//!
//! Three signals feed ONE funnel ([`learn_words`]):
//!
//! - **HistoryEdits**: the user edits a history entry; a word-level diff of
//!   the old vs new displayed text learns substitution targets that look like
//!   spelling corrections (never rewrites, insertions, or deletions).
//! - **RepeatedWords**: unknown words are counted per dictation in the
//!   history DB's `word_candidates` table; a word dictated in
//!   [`REPEAT_N`] dictations within a [`WINDOW_DAYS`]-day window is learned
//!   (the counting lives in `HistoryManager::observe_dictation_words`).
//! - **WatchPostPaste** (macOS, experimental by contract): after a paste, the
//!   focused AX element is polled briefly; an in-field correction of the
//!   pasted span is diffed with the same filters as HistoryEdits.
//!
//! Every candidate passes [`is_learnable`]: alphabetic core of 3+ chars, no
//! digits, not covered by the existing custom words (exact case-insensitive
//! or fuzzy at the Medium threshold), and not ordinary English (embedded
//! ~25k-word list, binary search). Learned words become ordinary custom
//! words: persisted through the same settings write path the editor uses,
//! visible and removable in the Custom section, announced to the frontend
//! via the `custom-words-learned` event (toast + live list refresh).

use log::{debug, info, warn};
use once_cell::sync::Lazy;
use serde::Serialize;
use std::collections::HashSet;
use strsim::levenshtein;
use tauri::{AppHandle, Emitter, Manager};

use crate::settings::{AutoLearnMode, FeatureLevel};

/// RepeatedWords: dictations containing a word before it is learned.
pub const REPEAT_N: i64 = 3;
/// RepeatedWords: the repeats must all fall within this many days.
pub const WINDOW_DAYS: i64 = 14;
/// [`WINDOW_DAYS`] in epoch seconds (the `word_candidates` clock unit).
pub const WINDOW_SECS: i64 = WINDOW_DAYS * 24 * 60 * 60;

/// Longest core auto-learn will consider (matches the Custom Words editor's
/// input cap and `find_best_match`'s candidate cap).
const MAX_WORD_CHARS: usize = 50;

/// A substitution whose old and new word differ by MORE than this normalized
/// levenshtein ratio is a rewrite, not a spelling correction, and is ignored.
const MAX_CORRECTION_DISTANCE: f64 = 0.5;

pub fn history_edits_enabled(mode: AutoLearnMode) -> bool {
    matches!(mode, AutoLearnMode::HistoryEdits | AutoLearnMode::Both)
}

pub fn repeated_words_enabled(mode: AutoLearnMode) -> bool {
    matches!(mode, AutoLearnMode::RepeatedWords | AutoLearnMode::Both)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnSource {
    HistoryEdit,
    RepeatedWord,
    PostPasteWatch,
}

impl LearnSource {
    fn as_str(self) -> &'static str {
        match self {
            LearnSource::HistoryEdit => "history_edit",
            LearnSource::RepeatedWord => "repeated_word",
            LearnSource::PostPasteWatch => "post_paste_watch",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LearnedWord {
    pub word: String,
    pub source: LearnSource,
}

// ---------------------------------------------------------------------------
// Shared filters.
// ---------------------------------------------------------------------------

/// The alphabetic core of a whitespace token: the punctuation shell is
/// trimmed (quotes, brackets, terminal punctuation) along with one trailing
/// possessive, so `"Claude's,"` yields `Claude`. Internal punctuation is NOT
/// stripped; [`is_learnable`] rejects such cores instead.
pub(crate) fn word_core(token: &str) -> &str {
    let core = token.trim_matches(|c: char| !c.is_alphanumeric());
    for possessive in ["'s", "\u{2019}s"] {
        if let Some(stripped) = core.strip_suffix(possessive) {
            return stripped;
        }
    }
    core
}

/// Embedded common-English wordlist (dolph/dictionary `popular.txt`, MIT;
/// see THIRD_PARTY_NOTICES.md), lowercased, deduped, and byte-sorted at asset
/// generation time so `binary_search` on `str` ordering is valid.
static COMMON_WORDS: Lazy<Vec<&'static str>> = Lazy::new(|| {
    include_str!("auto_learn/common_words_en.txt")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
});

fn is_common_word(word_lower: &str) -> bool {
    COMMON_WORDS.binary_search(&word_lower).is_ok()
}

/// The shared F4 gate: is this token worth learning as a custom word?
/// Alphabetic core of 3 to [`MAX_WORD_CHARS`] chars (no digits, no internal
/// punctuation), not ordinary English, and not already covered by the
/// custom-word list (exact case-insensitive, or fuzzy at the Medium
/// correction threshold: if the corrector would already rewrite it toward an
/// entry, there is nothing to learn).
pub(crate) fn is_learnable(token: &str, custom_words: &[String]) -> bool {
    let core = word_core(token);
    let chars = core.chars().count();
    if !(3..=MAX_WORD_CHARS).contains(&chars) {
        return false;
    }
    if !core.chars().all(|c| c.is_alphabetic()) {
        return false;
    }
    if is_common_word(&core.to_lowercase()) {
        return false;
    }
    !crate::audio_toolkit::covered_by_custom_words(
        core,
        custom_words,
        crate::defaults::word_threshold(FeatureLevel::Medium),
    )
}

/// Learnable unknown words of one dictation's final text, deduped
/// case-insensitively within the dictation, display casing preserved in
/// order of first appearance. The RepeatedWords tokenizer.
pub(crate) fn learnable_unknowns(text: &str, custom_words: &[String]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for token in text.split_whitespace() {
        let core = word_core(token);
        if !is_learnable(core, custom_words) {
            continue;
        }
        if seen.insert(core.to_lowercase()) {
            out.push(core.to_string());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Word-level diff (HistoryEdits and the post-paste watcher).
// ---------------------------------------------------------------------------

/// Word-level substitution pairs between two texts: LCS over whitespace
/// tokens; between two matches, the deleted and inserted runs pair up
/// position by position. Unpaired leftovers (pure insertions or deletions)
/// are dropped: F4 learns only from corrections, never from added or removed
/// words.
fn diff_substitutions<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<(&'a str, &'a str)> {
    // The O(n*m) table is fine for dictation-sized texts; a pathological
    // input (a giant document edited in History) is a rewrite, not a
    // spelling fix, so bail instead of allocating.
    if old.is_empty() || new.is_empty() || old.len() * new.len() > 1_000_000 {
        return Vec::new();
    }
    let (n, m) = (old.len(), new.len());
    let idx = |i: usize, j: usize| i * (m + 1) + j;
    let mut dp = vec![0u32; (n + 1) * (m + 1)];
    for i in 1..=n {
        for j in 1..=m {
            dp[idx(i, j)] = if old[i - 1] == new[j - 1] {
                dp[idx(i - 1, j - 1)] + 1
            } else {
                dp[idx(i - 1, j)].max(dp[idx(i, j - 1)])
            };
        }
    }

    // Backtrack (right to left), grouping the non-matching stretches between
    // matches into hunks of (deleted run, inserted run).
    let mut hunks: Vec<(Vec<&'a str>, Vec<&'a str>)> = Vec::new();
    let mut del: Vec<&'a str> = Vec::new();
    let mut ins: Vec<&'a str> = Vec::new();
    let (mut i, mut j) = (n, m);
    loop {
        if i > 0 && j > 0 && old[i - 1] == new[j - 1] {
            if !del.is_empty() || !ins.is_empty() {
                del.reverse();
                ins.reverse();
                hunks.push((std::mem::take(&mut del), std::mem::take(&mut ins)));
            }
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[idx(i, j - 1)] >= dp[idx(i - 1, j)]) {
            ins.push(new[j - 1]);
            j -= 1;
        } else if i > 0 {
            del.push(old[i - 1]);
            i -= 1;
        } else {
            break;
        }
    }
    if !del.is_empty() || !ins.is_empty() {
        del.reverse();
        ins.reverse();
        hunks.push((del, ins));
    }
    hunks.reverse();

    let mut pairs = Vec::new();
    for (deleted, inserted) in hunks {
        for (d, i) in deleted.into_iter().zip(inserted) {
            pairs.push((d, i));
        }
    }
    pairs
}

/// Words a manual correction teaches us: substitution targets that pass
/// [`is_learnable`] AND sit within [`MAX_CORRECTION_DISTANCE`] of the word
/// they replaced (a spelling fix, not a rewrite). Case-only fixes count
/// (correcting "claude" to "Claude" is a correction toward the proper form);
/// punctuation-only token changes do not.
pub(crate) fn diff_learned_words(
    old_text: &str,
    new_text: &str,
    custom_words: &[String],
) -> Vec<String> {
    let old_tokens: Vec<&str> = old_text.split_whitespace().collect();
    let new_tokens: Vec<&str> = new_text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    for (old_w, new_w) in diff_substitutions(&old_tokens, &new_tokens) {
        let old_core = word_core(old_w);
        let new_core = word_core(new_w);
        if old_core == new_core {
            continue; // punctuation-only edit; the word itself is unchanged
        }
        if !is_learnable(new_core, custom_words) {
            continue;
        }
        let old_lower = old_core.to_lowercase();
        let new_lower = new_core.to_lowercase();
        let max_len = old_lower.chars().count().max(new_lower.chars().count());
        if max_len == 0 {
            continue;
        }
        let ratio = levenshtein(&old_lower, &new_lower) as f64 / max_len as f64;
        if ratio > MAX_CORRECTION_DISTANCE {
            continue;
        }
        if !out.iter().any(|w| w.to_lowercase() == new_lower) {
            out.push(new_core.to_string());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The learn funnel.
// ---------------------------------------------------------------------------

pub const CUSTOM_WORDS_LEARNED_EVENT: &str = "custom-words-learned";

#[derive(Clone, Serialize)]
struct CustomWordsLearnedPayload {
    words: Vec<String>,
    source: &'static str,
}

/// Pure core of the funnel: fold `words` into `custom_words`, skipping any
/// word the list already covers (exact case-insensitive or fuzzy at Medium),
/// including words appended earlier in this same batch. Returns what was
/// actually added.
fn merge_learned(custom_words: &mut Vec<String>, words: Vec<LearnedWord>) -> Vec<LearnedWord> {
    let threshold = crate::defaults::word_threshold(FeatureLevel::Medium);
    let mut added = Vec::new();
    for learned in words {
        let word = word_core(&learned.word).to_string();
        if word.is_empty() {
            continue;
        }
        if crate::audio_toolkit::covered_by_custom_words(&word, custom_words, threshold) {
            continue;
        }
        custom_words.push(word.clone());
        added.push(LearnedWord {
            word,
            source: learned.source,
        });
    }
    added
}

/// The shared F4 funnel: dedup against the custom-word list, append the
/// survivors, persist through the same settings write path the Custom Words
/// editor uses, and emit [`CUSTOM_WORDS_LEARNED_EVENT`] (frontend toast plus
/// live list refresh). Learned words are ordinary custom words.
pub fn learn_words(app: &AppHandle, words: Vec<LearnedWord>) {
    if words.is_empty() {
        return;
    }
    let mut settings = crate::settings::get_settings(app);
    let added = merge_learned(&mut settings.custom_words, words);
    if added.is_empty() {
        return;
    }
    crate::settings::write_settings(app, settings);
    // One event per source; a single trigger's batch is homogeneous, so this
    // is almost always exactly one event.
    for source in [
        LearnSource::HistoryEdit,
        LearnSource::RepeatedWord,
        LearnSource::PostPasteWatch,
    ] {
        let words: Vec<String> = added
            .iter()
            .filter(|w| w.source == source)
            .map(|w| w.word.clone())
            .collect();
        if words.is_empty() {
            continue;
        }
        info!(
            "auto-learn ({}): added custom words {:?}",
            source.as_str(),
            words
        );
        if let Err(e) = app.emit(
            CUSTOM_WORDS_LEARNED_EVENT,
            CustomWordsLearnedPayload {
                words,
                source: source.as_str(),
            },
        ) {
            warn!("failed to emit {CUSTOM_WORDS_LEARNED_EVENT}: {e}");
        }
    }
}

/// HistoryEdits mode: learn from a manual edit of a history entry's
/// displayed text. Called by the `update_history_entry_text` command with
/// the pre-edit and post-edit texts.
pub fn on_history_edit(app: &AppHandle, old_text: &str, new_text: &str) {
    let settings = crate::settings::get_settings(app);
    if !history_edits_enabled(settings.auto_learn_mode) {
        return;
    }
    let words = diff_learned_words(old_text, new_text, &settings.custom_words);
    learn_words(
        app,
        words
            .into_iter()
            .map(|word| LearnedWord {
                word,
                source: LearnSource::HistoryEdit,
            })
            .collect(),
    );
}

// ---------------------------------------------------------------------------
// WatchPostPaste: pure watch machinery (platform-free, unit-tested), then
// the macOS AX plumbing.
//
// FRAGILITY, DOCUMENTED AND ACCEPTED (best-effort by contract): the watcher
// reads kAXValueAttribute of the element focused right after the paste. Many
// targets never expose a plain string there: web views (Chrome and most
// Electron apps surface AXWebArea trees), terminals, canvas editors, and
// secure fields, so the watch silently does not arm. Apps with smart quotes
// (Notes, TextEdit by default) rewrite quotes and apostrophes ON PASTE, so a
// dictation containing one never matches byte-for-byte and the watch stops
// after arming. All of that is fine: the mode is experimental, learns
// opportunistically, and must never disturb the paste itself.
// ---------------------------------------------------------------------------

/// Polls before the watch gives up (spaced [`WATCH_POLL_INTERVAL_MS`] apart).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) const WATCH_POLLS: u32 = 10;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) const WATCH_POLL_INTERVAL_MS: u64 = 2_000;
/// Fields larger than this are not watched (giant documents make span
/// tracking and diffing pointless and expensive).
pub(crate) const WATCH_MAX_VALUE_BYTES: usize = 512 * 1024;
/// Context captured on each side of the pasted span to relocate it after
/// edits shift it around.
const ANCHOR_CHARS: usize = 16;
/// Polls allowed for the pasted text to APPEAR in the AX value (some apps
/// commit the paste to their accessibility tree lazily).
const ARMING_POLLS: u32 = 2;

/// Environment kill switch (in addition to the mode enum), checked at every
/// watcher start: `VAPORLY_DISABLE_AX_WATCH=1` disables the AX watcher.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const DISABLE_ENV: &str = "VAPORLY_DISABLE_AX_WATCH";

fn tail_chars(s: &str, n: usize) -> &str {
    match s.char_indices().rev().nth(n.saturating_sub(1)) {
        Some((i, _)) => &s[i..],
        None => s,
    }
}

fn head_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Where the pasted text sits inside the field, remembered as its own bytes
/// plus up to [`ANCHOR_CHARS`] chars of surrounding context on each side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpanAnchors {
    /// Context immediately before the span; empty at the start of the field.
    prefix: String,
    /// Context immediately after the span (usually the trailing space);
    /// empty at the end of the field.
    suffix: String,
    /// The pasted text exactly as inserted.
    original: String,
}

/// Locate the paste in the field value (rfind: the LAST occurrence is the
/// fresh one) and capture its context anchors.
pub(crate) fn locate_pasted_span(value: &str, pasted: &str) -> Option<SpanAnchors> {
    if pasted.is_empty() {
        return None;
    }
    let start = value.rfind(pasted)?;
    let end = start + pasted.len();
    Some(SpanAnchors {
        prefix: tail_chars(&value[..start], ANCHOR_CHARS).to_string(),
        suffix: head_chars(&value[end..], ANCHOR_CHARS).to_string(),
        original: pasted.to_string(),
    })
}

/// Re-locate the (possibly edited) span in a later value snapshot. Anchors
/// first: after the last `prefix` occurrence, up to the last `suffix`
/// occurrence (rfind errs toward INCLUDING text the user appended, which the
/// diff then ignores as insertions, rather than truncating the span at an
/// interior match). Falls back to a direct search for the original span
/// (covers context edits around an untouched paste).
pub(crate) fn find_span<'a>(value: &'a str, anchors: &SpanAnchors) -> Option<&'a str> {
    let via_anchors = (|| {
        let start = if anchors.prefix.is_empty() {
            0
        } else {
            value.rfind(&anchors.prefix)? + anchors.prefix.len()
        };
        let end = if anchors.suffix.is_empty() {
            value.len()
        } else {
            start + value[start..].rfind(&anchors.suffix)?
        };
        Some(&value[start..end])
    })();
    via_anchors.or_else(|| {
        value
            .rfind(&anchors.original)
            .map(|i| &value[i..i + anchors.original.len()])
    })
}

/// One poll's observation, as seen by the state machine.
#[derive(Debug)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) enum PollReading {
    /// The field's current text.
    Value(String),
    /// The AX read failed in a way that may be transient (app busy).
    ReadError,
    /// The element is invalid or its app is no longer running.
    Gone,
}

#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) enum WatchVerdict {
    Continue,
    Stop(&'static str),
    Learn(Vec<String>),
}

/// The pure post-paste watch state machine: arming (waiting for the paste to
/// surface in the AX value), edit detection with a one-poll settle debounce
/// (so a mid-word correction is not diffed), and every stop condition. The
/// macOS driver feeds it one [`PollReading`] per poll.
pub(crate) struct WatchMachine {
    pasted: String,
    custom_words: Vec<String>,
    anchors: Option<SpanAnchors>,
    arming_left: u32,
    polls_left: u32,
    consecutive_errors: u32,
    /// The edited span seen last poll; a second identical sighting means the
    /// edit settled and can be diffed.
    pending: Option<String>,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl WatchMachine {
    pub(crate) fn new(
        pasted: String,
        custom_words: Vec<String>,
        initial_value: Option<&str>,
    ) -> Self {
        let anchors = initial_value.and_then(|value| locate_pasted_span(value, &pasted));
        WatchMachine {
            pasted,
            custom_words,
            anchors,
            arming_left: ARMING_POLLS,
            polls_left: WATCH_POLLS,
            consecutive_errors: 0,
            pending: None,
        }
    }

    pub(crate) fn on_poll(&mut self, reading: PollReading) -> WatchVerdict {
        self.polls_left = self.polls_left.saturating_sub(1);
        match self.classify(reading) {
            WatchVerdict::Continue if self.polls_left == 0 => {
                WatchVerdict::Stop("watch window elapsed")
            }
            verdict => verdict,
        }
    }

    fn classify(&mut self, reading: PollReading) -> WatchVerdict {
        let value = match reading {
            PollReading::Gone => return WatchVerdict::Stop("target element or app is gone"),
            PollReading::ReadError => {
                self.consecutive_errors += 1;
                if self.consecutive_errors >= 2 {
                    return WatchVerdict::Stop("two consecutive AX read errors");
                }
                return WatchVerdict::Continue;
            }
            PollReading::Value(value) => value,
        };
        self.consecutive_errors = 0;
        if value.len() > WATCH_MAX_VALUE_BYTES {
            return WatchVerdict::Stop("field value exceeds the size cap");
        }
        let Some(anchors) = self.anchors.clone() else {
            // Arming: the paste has not surfaced in the AX value yet.
            if let Some(anchors) = locate_pasted_span(&value, &self.pasted) {
                self.anchors = Some(anchors);
                return WatchVerdict::Continue;
            }
            self.arming_left = self.arming_left.saturating_sub(1);
            if self.arming_left == 0 {
                return WatchVerdict::Stop("pasted text never appeared in the field");
            }
            return WatchVerdict::Continue;
        };
        let Some(span) = find_span(&value, &anchors) else {
            return WatchVerdict::Stop("pasted span no longer locatable");
        };
        if span == anchors.original {
            self.pending = None;
            return WatchVerdict::Continue;
        }
        match self.pending.take() {
            Some(previous) if previous == span => {
                let words = diff_learned_words(&anchors.original, span, &self.custom_words);
                if words.is_empty() {
                    WatchVerdict::Stop("edit contained nothing learnable")
                } else {
                    WatchVerdict::Learn(words)
                }
            }
            _ => {
                self.pending = Some(span.to_string());
                WatchVerdict::Continue
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Watcher lifecycle (slot + entry points).
// ---------------------------------------------------------------------------

/// Slot holding the active post-paste watcher's stop flag: ONE watcher at a
/// time, managed like `CleanerSlot`. Platform-neutral so lib.rs manages it
/// unconditionally; only the macOS code ever fills it.
pub struct AxWatcherSlot(
    pub std::sync::Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>>,
);

/// Stop the active watcher, if any. Called when a NEW dictation starts (its
/// paste may rewrite the very field being watched) and when a new watcher
/// replaces the previous one.
pub fn cancel_active_watch(app: &AppHandle) {
    if let Some(slot) = app.try_state::<AxWatcherSlot>() {
        if let Some(flag) = slot.0.lock().unwrap().take() {
            flag.store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

/// Start the post-paste watch for a just-delivered dictation. Cheap and
/// non-blocking for the caller (the paste critical path): all AX work runs
/// on a spawned thread that dispatches individual reads to the main thread.
/// No-ops unless the mode is WatchPostPaste; `VAPORLY_DISABLE_AX_WATCH=1`
/// disables it regardless of the mode.
#[cfg(target_os = "macos")]
pub fn start_post_paste_watch(app: &AppHandle, final_text: String) {
    use std::sync::atomic::{AtomicBool, Ordering};

    let settings = crate::settings::get_settings(app);
    if settings.auto_learn_mode != AutoLearnMode::WatchPostPaste {
        return;
    }
    if std::env::var(DISABLE_ENV)
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        debug!("post-paste watch disabled by {DISABLE_ENV}");
        return;
    }
    if final_text.trim().is_empty() {
        return;
    }
    let Some(slot) = app.try_state::<AxWatcherSlot>() else {
        return;
    };
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    if let Some(previous) = slot.0.lock().unwrap().replace(std::sync::Arc::clone(&stop)) {
        previous.store(true, Ordering::Release);
    }
    let app = app.clone();
    let custom_words = settings.custom_words.clone();
    let spawned = std::thread::Builder::new()
        .name("ax-post-paste-watch".to_string())
        .spawn(move || watch_loop(app, final_text, custom_words, stop));
    if let Err(e) = spawned {
        warn!("post-paste watch thread failed to spawn: {e}");
    }
}

#[cfg(not(target_os = "macos"))]
pub fn start_post_paste_watch(_app: &AppHandle, _final_text: String) {}

#[cfg(target_os = "macos")]
fn release_slot(app: &AppHandle, mine: &std::sync::Arc<std::sync::atomic::AtomicBool>) {
    if let Some(slot) = app.try_state::<AxWatcherSlot>() {
        let mut guard = slot.0.lock().unwrap();
        if guard
            .as_ref()
            .is_some_and(|flag| std::sync::Arc::ptr_eq(flag, mine))
        {
            guard.take();
        }
    }
}

#[cfg(target_os = "macos")]
fn watch_loop(
    app: AppHandle,
    final_text: String,
    custom_words: Vec<String>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    const MAIN_THREAD_TIMEOUT: Duration = Duration::from_secs(5);

    // Capture ON THE MAIN THREAD: the focused element and its current value.
    // An unreadable value (web view, terminal, secure field) aborts silently:
    // the documented no-op contract for such targets.
    let captured = crate::stream_inject::run_on_main_blocking(&app, MAIN_THREAD_TIMEOUT, |_| {
        ax::capture_focused_text_element()
    });
    let Ok(Some(capture)) = captured else {
        debug!("post-paste watch: no readable focused text element; not watching");
        release_slot(&app, &stop);
        return;
    };
    if capture.value.len() > WATCH_MAX_VALUE_BYTES {
        debug!("post-paste watch: field too large at capture; not watching");
        release_slot(&app, &stop);
        return;
    }
    let element = std::sync::Arc::new(capture.element);
    let pid = capture.pid;
    let mut machine = WatchMachine::new(final_text, custom_words, Some(&capture.value));

    'watch: for _ in 0..WATCH_POLLS {
        // Sleep in small steps so a stop (new dictation, replacement watcher)
        // is honored promptly.
        let mut slept: u64 = 0;
        while slept < WATCH_POLL_INTERVAL_MS {
            if stop.load(Ordering::Acquire) {
                break 'watch;
            }
            std::thread::sleep(Duration::from_millis(200));
            slept += 200;
        }
        if stop.load(Ordering::Acquire) {
            break;
        }
        let element_for_poll = std::sync::Arc::clone(&element);
        let reading =
            match crate::stream_inject::run_on_main_blocking(&app, MAIN_THREAD_TIMEOUT, move |_| {
                ax::poll_element(&element_for_poll, pid)
            }) {
                Ok(reading) => reading,
                Err(e) => {
                    debug!("post-paste watch poll dispatch failed: {e}");
                    PollReading::ReadError
                }
            };
        match machine.on_poll(reading) {
            WatchVerdict::Continue => continue,
            WatchVerdict::Stop(reason) => {
                debug!("post-paste watch stopped: {reason}");
                break;
            }
            WatchVerdict::Learn(words) => {
                info!("post-paste watch: learning {words:?} from an in-field correction");
                learn_words(
                    &app,
                    words
                        .into_iter()
                        .map(|word| LearnedWord {
                            word,
                            source: LearnSource::PostPasteWatch,
                        })
                        .collect(),
                );
                break;
            }
        }
    }
    release_slot(&app, &stop);
}

// ---------------------------------------------------------------------------
// macOS AX FFI: minimal raw bindings over the ApplicationServices C API plus
// the few CoreFoundation calls needed to move strings across. Raw extern "C"
// was chosen over adding an AX binding crate: the two frameworks are ABI
// stable, the surface here is five symbols, and every CF object is wrapped
// in an RAII release ([`ax::CfRef`]).
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod ax {
    use super::PollReading;
    use std::ffi::c_void;

    type CFTypeRef = *const c_void;
    type CFStringRef = CFTypeRef;
    type CFIndex = isize;
    type CFTypeID = usize;
    type AXError = i32;
    type Boolean = u8;

    const K_AX_ERROR_SUCCESS: AXError = 0;
    const K_AX_ERROR_INVALID_UI_ELEMENT: AXError = -25202;
    const K_AX_ERROR_CANNOT_COMPLETE: AXError = -25204;
    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

    #[repr(C)]
    struct CFRange {
        location: CFIndex,
        length: CFIndex,
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> Boolean;
        fn AXUIElementCreateSystemWide() -> CFTypeRef;
        fn AXUIElementCopyAttributeValue(
            element: CFTypeRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> AXError;
        fn AXUIElementGetPid(element: CFTypeRef, pid: *mut i32) -> AXError;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: CFTypeRef);
        fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
        fn CFStringGetTypeID() -> CFTypeID;
        fn CFStringCreateWithBytes(
            alloc: CFTypeRef,
            bytes: *const u8,
            num_bytes: CFIndex,
            encoding: u32,
            is_external_representation: Boolean,
        ) -> CFStringRef;
        fn CFStringGetLength(s: CFStringRef) -> CFIndex;
        #[allow(clippy::too_many_arguments)]
        fn CFStringGetBytes(
            s: CFStringRef,
            range: CFRange,
            encoding: u32,
            loss_byte: u8,
            is_external_representation: Boolean,
            buffer: *mut u8,
            max_buf_len: CFIndex,
            used_buf_len: *mut CFIndex,
        ) -> CFIndex;
    }

    /// Owned CF object, released exactly once on drop. CF reference counting
    /// is thread-safe and every AX CALL is dispatched to the main thread; the
    /// wrapper only ever MOVES between threads, hence the Send/Sync.
    pub struct CfRef(CFTypeRef);
    unsafe impl Send for CfRef {}
    unsafe impl Sync for CfRef {}
    impl Drop for CfRef {
        fn drop(&mut self) {
            unsafe { CFRelease(self.0) }
        }
    }
    impl CfRef {
        fn adopt(raw: CFTypeRef) -> Option<CfRef> {
            if raw.is_null() {
                None
            } else {
                Some(CfRef(raw))
            }
        }
    }

    fn cf_string(s: &str) -> Option<CfRef> {
        let raw = unsafe {
            CFStringCreateWithBytes(
                std::ptr::null(),
                s.as_ptr(),
                s.len() as CFIndex,
                K_CF_STRING_ENCODING_UTF8,
                0,
            )
        };
        CfRef::adopt(raw)
    }

    /// Extract a Rust String from a CFString-typed CF object. `None` when the
    /// object is not a CFString, is unreasonably large, or fails conversion.
    fn cf_string_to_string(s: CFTypeRef) -> Option<String> {
        unsafe {
            if CFGetTypeID(s) != CFStringGetTypeID() {
                return None;
            }
            let len = CFStringGetLength(s); // UTF-16 units
            if len == 0 {
                return Some(String::new());
            }
            // Refuse absurd fields before allocating 3 bytes per unit (the
            // watch machine's own size cap stops anything above 512KB anyway).
            if len as usize > 2 * super::WATCH_MAX_VALUE_BYTES {
                return None;
            }
            let mut buf = vec![0u8; len as usize * 3];
            let mut used: CFIndex = 0;
            let converted = CFStringGetBytes(
                s,
                CFRange {
                    location: 0,
                    length: len,
                },
                K_CF_STRING_ENCODING_UTF8,
                0,
                0,
                buf.as_mut_ptr(),
                buf.len() as CFIndex,
                &mut used,
            );
            if converted != len {
                return None;
            }
            buf.truncate(used as usize);
            String::from_utf8(buf).ok()
        }
    }

    fn copy_attribute(element: CFTypeRef, name: &str) -> Result<CfRef, AXError> {
        let Some(attribute) = cf_string(name) else {
            return Err(K_AX_ERROR_CANNOT_COMPLETE);
        };
        let mut out: CFTypeRef = std::ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(element, attribute.0, &mut out) };
        if err != K_AX_ERROR_SUCCESS {
            return Err(err);
        }
        CfRef::adopt(out).ok_or(K_AX_ERROR_CANNOT_COMPLETE)
    }

    pub struct Capture {
        pub element: CfRef,
        pub value: String,
        pub pid: i32,
    }

    /// MAIN THREAD ONLY. The focused UI element with a readable plain-text
    /// value, or `None` (no accessibility trust, no focused element, or a
    /// value not exposed as an AX string): the watch then never starts.
    pub fn capture_focused_text_element() -> Option<Capture> {
        if unsafe { AXIsProcessTrusted() } == 0 {
            return None;
        }
        let system = CfRef::adopt(unsafe { AXUIElementCreateSystemWide() })?;
        let element = copy_attribute(system.0, "AXFocusedUIElement").ok()?;
        let value = copy_attribute(element.0, "AXValue").ok()?;
        let value = cf_string_to_string(value.0)?;
        let mut pid: i32 = 0;
        if unsafe { AXUIElementGetPid(element.0, &mut pid) } != K_AX_ERROR_SUCCESS || pid <= 0 {
            return None;
        }
        Some(Capture {
            element,
            value,
            pid,
        })
    }

    /// MAIN THREAD ONLY. One watch poll: app liveness first (a quit app can
    /// leave AX reads timing out instead of failing), then the value.
    pub fn poll_element(element: &CfRef, pid: i32) -> PollReading {
        if !pid_alive(pid) {
            return PollReading::Gone;
        }
        match copy_attribute(element.0, "AXValue") {
            Err(K_AX_ERROR_INVALID_UI_ELEMENT) => PollReading::Gone,
            Err(_) => PollReading::ReadError,
            Ok(value) => match cf_string_to_string(value.0) {
                Some(text) => PollReading::Value(text),
                None => PollReading::ReadError,
            },
        }
    }

    /// `kill(pid, 0)` probes liveness without signaling; EPERM still means
    /// the process exists.
    fn pid_alive(pid: i32) -> bool {
        if unsafe { libc::kill(pid, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn no_words() -> Vec<String> {
        Vec::new()
    }

    // ---- common-words asset ----

    #[test]
    fn common_words_list_is_sorted_deduped_lowercase() {
        let list = &*COMMON_WORDS;
        assert!(
            list.len() > 20_000,
            "expected the ~25k popular list, got {}",
            list.len()
        );
        for pair in list.windows(2) {
            assert!(
                pair[0] < pair[1],
                "list not strictly sorted at {:?} >= {:?} (binary search requires it)",
                pair[0],
                pair[1]
            );
        }
        assert!(list
            .iter()
            .all(|w| w.chars().all(|c| c.is_ascii_lowercase())));
    }

    #[test]
    fn common_words_lookup_hits_and_misses() {
        for word in ["the", "hello", "world", "because", "aardvark"] {
            assert!(is_common_word(word), "{word} should be common");
        }
        for word in ["kubernetes", "claude", "vaporly", "chargebee", "zzyzx"] {
            assert!(!is_common_word(word), "{word} should be unknown");
        }
    }

    // ---- word_core / is_learnable ----

    #[test]
    fn word_core_strips_shell_and_possessive() {
        assert_eq!(word_core("Claude"), "Claude");
        assert_eq!(word_core("Claude,"), "Claude");
        assert_eq!(word_core("(Claude's)"), "Claude");
        assert_eq!(word_core("\u{201c}Claude\u{2019}s\u{201d}"), "Claude");
        assert_eq!(word_core("don't"), "don't"); // internal punctuation stays
        assert_eq!(word_core("..."), "");
        assert_eq!(word_core("hello."), "hello");
    }

    #[test]
    fn is_learnable_table() {
        let none = no_words();
        // Learnable: unknown alphabetic names and jargon.
        assert!(is_learnable("Kubernetes", &none));
        assert!(is_learnable("ChargeBee", &none));
        assert!(is_learnable("Claude's,", &none)); // possessive + shell
        assert!(is_learnable("Zzyzx", &none));
        // Common English is never learned.
        assert!(!is_learnable("the", &none));
        assert!(!is_learnable("Hello", &none));
        assert!(!is_learnable("Because,", &none));
        // Shape filters: short cores, digits, internal punctuation, empties.
        assert!(!is_learnable("ab", &none));
        assert!(!is_learnable("2fa", &none));
        assert!(!is_learnable("42", &none));
        assert!(!is_learnable("b2b", &none));
        assert!(!is_learnable("don't", &none));
        assert!(!is_learnable("e-mail", &none));
        assert!(!is_learnable("", &none));
        assert!(!is_learnable("...", &none));
        let long = "x".repeat(60);
        assert!(!is_learnable(&long, &none));
        // Coverage: exact (case-insensitive) and fuzzy at Medium.
        let covered = vec!["Claude".to_string()];
        assert!(!is_learnable("Claude", &covered));
        assert!(!is_learnable("claude", &covered));
        assert!(!is_learnable("Claud", &covered)); // lev 1/6 = 0.17 < 0.18
        assert!(is_learnable("Vaporly", &covered));
    }

    // ---- diff learner ----

    #[test]
    fn diff_substitution_learns_the_correction() {
        assert_eq!(
            diff_learned_words(
                "I asked Cloud about the deploy.",
                "I asked Claude about the deploy.",
                &no_words()
            ),
            vec!["Claude".to_string()]
        );
    }

    #[test]
    fn diff_insertions_and_deletions_are_ignored() {
        assert_eq!(
            diff_learned_words("send the file", "send the Kubernetes file", &no_words()),
            Vec::<String>::new()
        );
        assert_eq!(
            diff_learned_words("send the Kubernetes file", "send the file", &no_words()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn diff_rewrite_fails_the_similarity_floor() {
        // "Zzyzx" is learnable in isolation; the similarity floor is what
        // rejects it as a REPLACEMENT for an unrelated word.
        assert!(is_learnable("Zzyzx", &no_words()));
        assert_eq!(
            diff_learned_words("ask Cloud today", "ask Zzyzx today", &no_words()),
            Vec::<String>::new()
        );
        // A whole-sentence rewrite learns nothing.
        assert_eq!(
            diff_learned_words(
                "please email Jon Smith about it",
                "let us message the whole team instead",
                &no_words()
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn diff_common_short_and_digit_words_are_skipped() {
        // "took" is ordinary English: a typo fix toward it teaches nothing.
        assert_eq!(
            diff_learned_words("I tok the bus", "I took the bus", &no_words()),
            Vec::<String>::new()
        );
        assert_eq!(
            diff_learned_words("go to xy", "go to ab", &no_words()),
            Vec::<String>::new()
        );
        assert_eq!(
            diff_learned_words("call 911 now", "call 912 now", &no_words()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn diff_fuzzy_covered_words_are_skipped() {
        let covered = vec!["Claude".to_string()];
        assert_eq!(
            diff_learned_words("ask Cloud", "ask Claud", &covered),
            Vec::<String>::new()
        );
    }

    #[test]
    fn diff_punctuation_only_edit_is_not_a_correction() {
        assert_eq!(
            diff_learned_words("Hello Zzyzx.", "Hello Zzyzx", &no_words()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn diff_case_fix_toward_a_proper_name_learns_it() {
        assert_eq!(
            diff_learned_words("meet zzyzx later", "meet Zzyzx later", &no_words()),
            vec!["Zzyzx".to_string()]
        );
    }

    #[test]
    fn diff_learns_multiple_corrections_in_one_edit() {
        let old = "the cloud api and the vaporli app";
        let new = "the Claude api and the Vaporly app";
        assert_eq!(
            diff_learned_words(old, new, &no_words()),
            vec!["Claude".to_string(), "Vaporly".to_string()]
        );
    }

    // ---- learn funnel (pure core) ----

    fn lw(word: &str, source: LearnSource) -> LearnedWord {
        LearnedWord {
            word: word.to_string(),
            source,
        }
    }

    #[test]
    fn merge_learned_dedups_exact_fuzzy_and_within_batch() {
        let mut list = vec!["Claude".to_string()];
        let added = merge_learned(
            &mut list,
            vec![
                lw("Kubernetes", LearnSource::RepeatedWord),
                lw("claude", LearnSource::HistoryEdit), // exact, case-insensitive
                lw("Claud", LearnSource::HistoryEdit),  // fuzzy at Medium
                lw("Kubernetes,", LearnSource::PostPasteWatch), // within-batch dup
                lw("", LearnSource::HistoryEdit),
            ],
        );
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].word, "Kubernetes");
        assert_eq!(added[0].source, LearnSource::RepeatedWord);
        assert_eq!(
            list,
            vec!["Claude".to_string(), "Kubernetes".to_string()],
            "survivors are APPENDED, never reordered"
        );
    }

    #[test]
    fn learnable_unknowns_dedup_and_coverage() {
        let covered = vec!["Claude".to_string()];
        assert_eq!(
            learnable_unknowns(
                "Deploy Kubernetes now, kubernetes later. Claude helps with ChargeBee.",
                &covered
            ),
            vec!["Kubernetes".to_string(), "ChargeBee".to_string()]
        );
    }

    // ---- span anchors ----

    #[test]
    fn locate_and_find_the_untouched_span() {
        let pasted = "Hello Zzyzx today.";
        let value = format!("Notes before. {pasted} ");
        let anchors = locate_pasted_span(&value, pasted).expect("span located");
        assert_eq!(find_span(&value, &anchors), Some(pasted));
        // Paste at the very start of an empty field (the common case).
        let value2 = format!("{pasted} ");
        let anchors2 = locate_pasted_span(&value2, pasted).expect("span located");
        assert_eq!(find_span(&value2, &anchors2), Some(pasted));
    }

    #[test]
    fn find_span_tracks_an_in_span_edit() {
        let pasted = "Hello Cloud today.";
        let value = format!("Intro line here. {pasted} ");
        let anchors = locate_pasted_span(&value, pasted).unwrap();
        let edited = "Intro line here. Hello Claude today. ";
        assert_eq!(find_span(edited, &anchors), Some("Hello Claude today."));
    }

    #[test]
    fn find_span_survives_context_edits_via_direct_fallback() {
        let pasted = "Hello Zzyzx today.";
        let value = format!("Intro line here. {pasted} ");
        let anchors = locate_pasted_span(&value, pasted).unwrap();
        // The user rewrote the CONTEXT before the span: the prefix anchor is
        // gone, the direct search still finds the untouched span.
        let edited = format!("Completely new opening: {pasted} ");
        assert_eq!(find_span(&edited, &anchors), Some(pasted));
    }

    #[test]
    fn find_span_includes_appended_text_as_insertions() {
        let pasted = "Hello Zzyzx today.";
        let value = format!("{pasted} ");
        let anchors = locate_pasted_span(&value, pasted).unwrap();
        // Text typed after the span is absorbed into the located span (rfind
        // suffix) and diffs as pure insertions, which learn nothing.
        let appended = format!("{pasted} more words after ");
        let span = find_span(&appended, &anchors).expect("span still found");
        assert!(span.starts_with(pasted));
        assert_eq!(
            diff_learned_words(pasted, span, &no_words()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn find_span_reports_a_deleted_span_as_lost() {
        let pasted = "Hello Zzyzx today.";
        let value = format!("Notes before. {pasted} ");
        let anchors = locate_pasted_span(&value, pasted).unwrap();
        assert_eq!(find_span("Notes before. ", &anchors), None);
    }

    // ---- watch machine ----

    fn armed_machine(pasted: &str, custom_words: Vec<String>) -> (WatchMachine, String) {
        let value = format!("{pasted} ");
        let machine = WatchMachine::new(pasted.to_string(), custom_words, Some(&value));
        assert!(machine.anchors.is_some(), "machine should arm on capture");
        (machine, value)
    }

    #[test]
    fn machine_learns_after_the_edit_settles() {
        let (mut m, _) = armed_machine("Hello Cloud today.", no_words());
        let edited = "Hello Claude today. ".to_string();
        // First sighting of the edit: settle debounce, no learn yet.
        assert_eq!(
            m.on_poll(PollReading::Value(edited.clone())),
            WatchVerdict::Continue
        );
        // Second identical sighting: settled, diffed, learned.
        assert_eq!(
            m.on_poll(PollReading::Value(edited)),
            WatchVerdict::Learn(vec!["Claude".to_string()])
        );
    }

    #[test]
    fn machine_keeps_waiting_while_the_edit_churns() {
        let (mut m, _) = armed_machine("Hello Cloud today.", no_words());
        assert_eq!(
            m.on_poll(PollReading::Value("Hello Cl today. ".into())),
            WatchVerdict::Continue
        );
        assert_eq!(
            m.on_poll(PollReading::Value("Hello Clau today. ".into())),
            WatchVerdict::Continue
        );
        assert_eq!(
            m.on_poll(PollReading::Value("Hello Claude today. ".into())),
            WatchVerdict::Continue
        );
        assert_eq!(
            m.on_poll(PollReading::Value("Hello Claude today. ".into())),
            WatchVerdict::Learn(vec!["Claude".to_string()])
        );
    }

    #[test]
    fn machine_stops_when_a_settled_edit_teaches_nothing() {
        let (mut m, _) = armed_machine("Hello Cloud today.", no_words());
        let edited = "Hello there today. ".to_string(); // rewrite, floor fails
        assert_eq!(
            m.on_poll(PollReading::Value(edited.clone())),
            WatchVerdict::Continue
        );
        assert_eq!(
            m.on_poll(PollReading::Value(edited)),
            WatchVerdict::Stop("edit contained nothing learnable")
        );
    }

    #[test]
    fn machine_untouched_span_runs_out_the_window() {
        let (mut m, value) = armed_machine("Hello Zzyzx today.", no_words());
        for _ in 0..(WATCH_POLLS - 1) {
            assert_eq!(
                m.on_poll(PollReading::Value(value.clone())),
                WatchVerdict::Continue
            );
        }
        assert_eq!(
            m.on_poll(PollReading::Value(value)),
            WatchVerdict::Stop("watch window elapsed")
        );
    }

    #[test]
    fn machine_arms_late_when_the_paste_surfaces_slowly() {
        let pasted = "Hello Zzyzx today.";
        let mut m = WatchMachine::new(pasted.to_string(), no_words(), Some("old content"));
        assert!(m.anchors.is_none());
        assert_eq!(
            m.on_poll(PollReading::Value(format!("{pasted} "))),
            WatchVerdict::Continue
        );
        assert!(m.anchors.is_some(), "armed on the first poll that shows it");
    }

    #[test]
    fn machine_gives_up_arming_when_the_paste_never_appears() {
        let mut m = WatchMachine::new("Hello Zzyzx today.".to_string(), no_words(), Some(""));
        assert_eq!(
            m.on_poll(PollReading::Value("still unrelated".into())),
            WatchVerdict::Continue
        );
        assert_eq!(
            m.on_poll(PollReading::Value("still unrelated".into())),
            WatchVerdict::Stop("pasted text never appeared in the field")
        );
    }

    #[test]
    fn machine_stops_after_two_consecutive_errors_only() {
        let (mut m, value) = armed_machine("Hello Zzyzx today.", no_words());
        assert_eq!(m.on_poll(PollReading::ReadError), WatchVerdict::Continue);
        assert_eq!(
            m.on_poll(PollReading::Value(value)),
            WatchVerdict::Continue,
            "a good read resets the error streak"
        );
        assert_eq!(m.on_poll(PollReading::ReadError), WatchVerdict::Continue);
        assert_eq!(
            m.on_poll(PollReading::ReadError),
            WatchVerdict::Stop("two consecutive AX read errors")
        );
    }

    #[test]
    fn machine_stops_on_gone_and_oversize_and_lost_span() {
        let (mut m, _) = armed_machine("Hello Zzyzx today.", no_words());
        assert_eq!(
            m.on_poll(PollReading::Gone),
            WatchVerdict::Stop("target element or app is gone")
        );

        let (mut m, _) = armed_machine("Hello Zzyzx today.", no_words());
        let huge = "x".repeat(WATCH_MAX_VALUE_BYTES + 1);
        assert_eq!(
            m.on_poll(PollReading::Value(huge)),
            WatchVerdict::Stop("field value exceeds the size cap")
        );

        let (mut m, _) = armed_machine("Hello Zzyzx today.", no_words());
        assert_eq!(
            m.on_poll(PollReading::Value(String::new())),
            WatchVerdict::Stop("pasted span no longer locatable")
        );
    }

    #[test]
    fn machine_respects_the_custom_word_snapshot() {
        // The corrected word is already covered: the settled edit teaches
        // nothing, so the watch stops without learning.
        let (mut m, _) = armed_machine("Hello Cloud today.", vec!["Claude".to_string()]);
        let edited = "Hello Claude today. ".to_string();
        assert_eq!(
            m.on_poll(PollReading::Value(edited.clone())),
            WatchVerdict::Continue
        );
        assert_eq!(
            m.on_poll(PollReading::Value(edited)),
            WatchVerdict::Stop("edit contained nothing learnable")
        );
    }
}
