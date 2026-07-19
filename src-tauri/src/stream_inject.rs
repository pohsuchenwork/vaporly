//! F3: on-textbox streaming. TextboxRaw types the stream's committed words
//! into the frontmost app while the user speaks and repairs them to the
//! processed final text at finish; TextboxClean types cleaned sentences as
//! each completes (deterministic sentences, or LiveCleaner chunks when a
//! model plan is active); Inline (round 2) types the deterministically
//! filtered live text UNDERLINED and polishes each completed sentence in
//! place, swapping to the clean final text on release. The overlay stays a
//! compact status pill.
//!
//! Inline's underline is the Unicode combining-low-line trick: U+0332 after
//! each non-whitespace grapheme cluster. It ships now because it needs no
//! new OS machinery. PARKED alternatives, on request: (a) a plain-text
//! no-underline Inline variant, and (b) a real macOS IME input source with
//! marked text (system marked-text composition), a separate multi-week
//! project.
//!
//! Layering:
//! - [`InjectSink`]: the two injection primitives (type text, backspace n
//!   graphemes). Mocked in unit tests; the production `MainThreadSink` posts
//!   each operation to the main thread (CGEvent posting through the managed
//!   Enigo) and waits for its result.
//! - `InjectorCore`: the planner/state machine: ledger of injected text,
//!   freeze ladder (rewrite, focus change, sink error), finalize repair math
//!   (grapheme-aligned common prefix), cancel wipe. Pure except for its sink;
//!   the current focus is passed IN so tests can simulate app switches.
//! - [`StreamInjector`]: the production wrapper. One dedicated worker thread
//!   owns the core and drains a FIFO command channel, so stream commits,
//!   cleaner chunks, finalize, and cancel run strictly in arrival order and
//!   sink calls never interleave. Callers never block, except [`StreamInjector::finalize`],
//!   which waits for the worker's reply and is only ever called from the stop
//!   path's async task.
//!
//! Safety invariants:
//! - The worker thread is the ONLY driver of the sink, and it is never the
//!   main thread, so waiting on main-thread closures cannot deadlock the
//!   event loop (cancel, which the tray can trigger ON the main thread, is
//!   fire-and-forget for the same reason).
//! - Before any batch of sink operations the worker re-polls the frontmost
//!   app; when the user switched away, injection freezes: nothing more is
//!   typed, and finalize neither deletes nor pastes into the wrong app.
//! - A dictation whose preflight fails (secure event input active, no
//!   captured home app, input system not ready) gets NO injector: the caller
//!   degrades that run to Bar behavior.

use crate::input::{self, EnigoState};
use crate::pipeline::StageConfig;
use enigo::Enigo;
use log::{debug, info, warn};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use unicode_segmentation::UnicodeSegmentation;

/// App-managed slot holding the active dictation's injector (one at a time),
/// managed like `CleanerSlot`: set in `TranscribeAction::start`, taken by the
/// finalize/cancel paths, cleared by `FinishGuard`.
pub struct InjectorSlot(pub Mutex<Option<Arc<StreamInjector>>>);

/// What a textbox dictation streams into the target app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectMode {
    /// Raw committed words as they arrive; finish repairs to the final text.
    Raw,
    /// Deterministically cleaned sentences as each completes (TextboxClean
    /// with no model plan): driven from the same stream commits as Raw.
    CleanDet,
    /// LiveCleaner chunks as each cleans (TextboxClean with a model plan).
    CleanModel,
    /// The det-filtered live text streamed UNDERLINED, with LiveCleaner
    /// chunks (when a model plan is active) repairing each sentence in place
    /// with underlined polish. Consumes BOTH stream commits and chunks.
    Inline,
}

/// The two injection primitives. `backspace` counts GRAPHEMES, not bytes or
/// chars: one Backspace key event deletes one user-perceived character.
pub trait InjectSink: Send {
    fn type_text(&mut self, s: &str) -> Result<(), String>;
    fn backspace(&mut self, n: usize) -> Result<(), String>;
}

/// Why injection stopped mid-dictation. Frozen keeps the ledger: the finalize
/// path decides what recovery is safe for each reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FreezeReason {
    /// The stream rewrote already-committed text (degraded mode, jitter) or a
    /// cleaner chunk desynced. Focus is unchanged, so finalize still repairs.
    Rewrite,
    /// The frontmost app is no longer the dictation's home app. Finalize must
    /// neither delete nor paste (the wrong app would receive it).
    FocusChanged,
    /// A sink operation failed. Focus is unchanged, so finalize still tries
    /// the repair (the ledger only ever contains CONFIRMED typed text).
    SinkError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Active,
    Frozen(FreezeReason),
    Done,
}

/// What the core decided at finalize. The production wrapper turns this into
/// user-visible effects (paste, clipboard, overlay flash).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FinalizeOutcome {
    /// The target app now holds exactly the final text (plus the trailing
    /// space when enabled); everything went through the sink.
    Injected,
    /// The sink repair deleted down to the common prefix; the remainder
    /// carries a newline (typed Returns could submit chats), so it must go
    /// through one hardened clipboard paste. `deleted` is how many
    /// backspaces the repair just sent: the paste path waits for the target
    /// to drain them before the Cmd+V, or the paste keystroke lands after
    /// the clipboard restore and pastes the OLD clipboard.
    NeedsPaste { remainder: String, deleted: usize },
    /// Focus left the home app: nothing was deleted, nothing may be pasted.
    SkippedFocusChanged,
    /// A sink operation failed mid-repair: deletion aborted, nothing may be
    /// pasted (never risk duplicating text). History still has the result.
    Failed(String),
}

/// Result of a finalize as seen by the stop path (drives the overlay flash).
#[derive(Debug)]
pub enum FinalResult {
    Inserted,
    SkippedFocusChanged,
    Error(String),
}

/// Sentences of deterministic live text held back from injection while the
/// RAW committed tail is unterminated (stage-6 shaping gives the growing
/// tail a provisional period, so the newest complete-LOOKING sentence may
/// not be real). When the raw tail ends with a real terminator the holdback
/// drops to 0 and the sentence ships immediately; a correction that later
/// rewrites it repairs in place via `repair_to_screen`.
const DET_HOLDBACK_SENTENCES: usize = 1;
/// Settle time granted to the target app per just-sent backspace before the
/// finalize remainder paste, capped below. The backspace flood sits in the
/// target's event queue; the Cmd+V joins that queue and reads the clipboard
/// only when processed, so pasting too early races the clipboard restore
/// (the owner's "old clipboard pasted at finish" field report).
const PASTE_SETTLE_PER_BACKSPACE: Duration = Duration::from_millis(3);
const PASTE_SETTLE_MAX: Duration = Duration::from_millis(350);
/// Clipboard restore delay for the finalize remainder paste. The default
/// 50ms is fine for an idle target; after a repair the target can lag, so
/// the restore waits longer before putting the old clipboard back.
const REMAINDER_PASTE_RESTORE_DELAY_MS: u64 = 350;
/// Timeout for one main-thread sink operation (a typed delta or one backspace
/// batch). Generous: the main thread can be busy with window work.
const SINK_OP_TIMEOUT: Duration = Duration::from_secs(5);
/// Timeout for the finalize remainder paste (the clipboard paste path sleeps
/// internally between clipboard write, key combo, and restore).
const PASTE_OP_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the stop path waits for the worker to finish the whole finalize
/// (queued deltas, repair backspaces, remainder paste) before erroring out.
const FINALIZE_REPLY_TIMEOUT: Duration = Duration::from_secs(30);
/// Backspaces per main-thread closure: keeps each main-thread stint short so
/// a long deletion cannot freeze the UI event loop.
const BACKSPACE_BATCH: usize = 24;
/// Spacing between backspace key events (some apps drop key events posted
/// back-to-back at full speed).
const BACKSPACE_SPACING: Duration = Duration::from_millis(2);

/// Whether a textbox dictation may create an injector at all. Pure so the
/// decision is unit-testable with an injected secure-input flag: secure event
/// input (a password field is focused somewhere) means synthetic keystrokes
/// are dropped or land wrong, and no captured home app means the focus guard
/// could never verify the target.
pub(crate) fn preflight_allows(secure_input: bool, home_bundle: Option<&str>) -> bool {
    !secure_input && home_bundle.is_some_and(|b| !b.is_empty())
}

/// macOS: whether some process holds secure event input (password fields,
/// some lock/agent UIs). One Carbon symbol; links with no extra flags.
#[cfg(target_os = "macos")]
fn secure_input_active() -> bool {
    extern "C" {
        fn IsSecureEventInputEnabled() -> bool;
    }
    unsafe { IsSecureEventInputEnabled() }
}

#[cfg(not(target_os = "macos"))]
fn secure_input_active() -> bool {
    false
}

/// Byte length of the longest common prefix of `a` and `b` that ends on a
/// grapheme boundary of BOTH strings (graphemes are compared whole, so a
/// combining sequence never binds against its precomposed form and an emoji
/// ZWJ family is one unit). The backspace count for a repair is then the
/// grapheme count of the ledger's tail beyond this prefix.
fn grapheme_lcp_bytes(a: &str, b: &str) -> usize {
    let mut end = 0usize;
    let mut ga = a.graphemes(true);
    let mut gb = b.graphemes(true);
    loop {
        match (ga.next(), gb.next()) {
            (Some(x), Some(y)) if x == y => end += x.len(),
            _ => break,
        }
    }
    end
}

fn grapheme_count(s: &str) -> usize {
    s.graphemes(true).count()
}

/// Whether the RAW committed text already ends with a real sentence
/// terminator (the stream actually heard the punctuation, as opposed to
/// stage-6 shaping's provisional period on a growing tail).
fn raw_tail_terminated(committed: &str) -> bool {
    matches!(
        committed.trim_end().chars().next_back(),
        Some('.' | '!' | '?')
    )
}

/// Inline's visual: U+0332 COMBINING LOW LINE after each non-whitespace
/// grapheme cluster. The mark EXTENDS its cluster, so
/// `grapheme_count(underline(s)) == grapheme_count(s)`: one backspace still
/// deletes one user-perceived character, all ledger math unchanged, and a
/// ZWJ emoji family stays one joined cluster. The final text is typed clean
/// at finalize, so no mark ever survives the dictation.
fn underline(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for g in s.graphemes(true) {
        out.push_str(g);
        if !g.chars().all(char::is_whitespace) {
            out.push('\u{0332}');
        }
    }
    out
}

/// The planner/state machine. Owns the ledger (`injected` is the EXACT text
/// confirmed typed into the target app) and the sink; focus is passed in per
/// call so the production worker polls it and tests inject it.
struct InjectorCore<S: InjectSink> {
    sink: S,
    mode: InjectMode,
    /// The dictation's live-variant stage snapshot (CleanDet and Inline): the
    /// deterministic pass over committed text must agree byte-for-byte with
    /// what the finalize path computes.
    cfg_live: Option<StageConfig>,
    append_trailing_space: bool,
    /// Bundle id of the app focused when dictation started; the focus guard
    /// compares every later poll against it.
    home_bundle: String,
    phase: Phase,
    /// Exact injected string (a byte prefix of committed text for Raw, of the
    /// deterministic live output for CleanDet, of the space-joined cleaned
    /// chunks for CleanModel, of the UNDERLINED mixed polish+raw text for
    /// Inline).
    injected: String,
    /// CleanModel: byte length of each cleaner chunk's contribution to
    /// `injected` (including its joining space; zero for empty cleans, so
    /// indices stay aligned with the LiveCleaner's chunk list).
    chunk_lens: Vec<usize>,
    /// Inline: the det-filtered live text last observed (PLAIN, un-underlined;
    /// the source of the raw underlined tail on screen).
    inline_raw: String,
    /// Inline: per LiveCleaner chunk, (trimmed cleaned text, the chunk's
    /// src_prefix). The prefix says how much of `inline_raw` the polish
    /// covers; index-aligned with the LiveCleaner's chunk list.
    inline_chunks: Vec<(String, String)>,
}

impl<S: InjectSink> InjectorCore<S> {
    /// Focus guard: every injection batch re-verifies the frontmost app is
    /// still the dictation's home app. A mismatch (or an unreadable focus)
    /// freezes injection for the rest of the dictation.
    fn focus_ok(&mut self, focus: Option<&str>) -> bool {
        if focus == Some(self.home_bundle.as_str()) {
            return true;
        }
        debug!(
            "textbox injector frozen: focus moved from '{}' to {:?}",
            self.home_bundle, focus
        );
        self.phase = Phase::Frozen(FreezeReason::FocusChanged);
        false
    }

    fn type_or_freeze(&mut self, delta: &str) -> bool {
        match self.sink.type_text(delta) {
            Ok(()) => {
                self.injected.push_str(delta);
                true
            }
            Err(e) => {
                warn!("textbox injector frozen: typing failed: {e}");
                self.phase = Phase::Frozen(FreezeReason::SinkError);
                false
            }
        }
    }

    /// Live typing must never send a Return: chat apps submit on it. A
    /// newline-bearing contribution (a multi-line phrase expansion in the
    /// deterministic live text, or a model chunk with a line break) freezes
    /// injection instead; the finalize repair then delivers the whole text
    /// through the clipboard paste branch. Returns true when it froze.
    fn freeze_on_newline(&mut self, delta: &str) -> bool {
        if !delta.contains('\n') {
            return false;
        }
        debug!("textbox injector frozen: newline in live contribution, deferred to finalize paste");
        self.phase = Phase::Frozen(FreezeReason::Rewrite);
        true
    }

    /// A new committed snapshot from the stream worker (Raw, CleanDet,
    /// Inline).
    fn on_committed(&mut self, committed: &str, focus: Option<&str>) {
        if self.phase != Phase::Active {
            return;
        }
        match self.mode {
            InjectMode::Raw => self.raw_delta(committed, focus),
            InjectMode::CleanDet => self.det_sentences(committed, focus),
            InjectMode::CleanModel => {} // driven by LiveCleaner chunks instead
            InjectMode::Inline => self.inline_stream(committed, focus),
        }
    }

    fn raw_delta(&mut self, committed: &str, focus: Option<&str>) {
        if !committed.starts_with(self.injected.as_str()) {
            // Degraded/jitter rewrite: stop typing, keep the ledger. Focus is
            // unchanged, so the finalize repair covers the difference.
            debug!("textbox injector frozen: stream rewrote committed text");
            self.phase = Phase::Frozen(FreezeReason::Rewrite);
            return;
        }
        let delta = committed[self.injected.len()..].to_string();
        if delta.is_empty() || !self.focus_ok(focus) {
            return;
        }
        self.type_or_freeze(&delta);
    }

    /// CleanDet: run the dictation's deterministic live pass over the full
    /// committed text and inject each newly COMPLETED sentence. The holdback
    /// is DYNAMIC: 0 when the raw committed tail ends with a real terminator
    /// (that sentence is done, ship it now), else 1 (stage-6 shaping's
    /// provisional period makes the growing tail LOOK complete). Cue-glued
    /// sentences still ship with their successor. When the filtered text no
    /// longer extends the ledger (a mind-change merge across a terminator, a
    /// standalone retraction), the injection REPAIRS in place instead of
    /// freezing: down to the eligible region, or down to the common prefix
    /// when nothing new is eligible yet.
    fn det_sentences(&mut self, committed: &str, focus: Option<&str>) {
        let Some(cfg) = self.cfg_live.as_ref() else {
            return;
        };
        let filtered = crate::pipeline::run_deterministic(committed, cfg);
        let holdback = if raw_tail_terminated(committed) {
            0
        } else {
            DET_HOLDBACK_SENTENCES
        };
        let end = crate::pipeline::live::eligible_sentence_end(&filtered, holdback).unwrap_or(0);

        if filtered.starts_with(self.injected.as_str()) {
            // Normal growth. `end` can sit at or before the ledger when the
            // holdback flipped back to 1: keep what was already shipped.
            if end <= self.injected.len() {
                return;
            }
            let delta = filtered[self.injected.len()..end].to_string();
            if delta.is_empty() || !self.focus_ok(focus) || self.freeze_on_newline(&delta) {
                return;
            }
            self.type_or_freeze(&delta);
            return;
        }

        // Divergence: repair to the eligible region, or only down to the
        // grapheme-aligned common prefix when nothing new is eligible (a
        // shrink never types speculative tail text).
        let lcp = grapheme_lcp_bytes(&self.injected, &filtered);
        let target = &filtered[..end.max(lcp)];
        debug!(
            "textbox injector repairing in place: deterministic live text rewrote {} ledger bytes",
            self.injected.len() - lcp
        );
        if !self.focus_ok(focus) || self.freeze_on_newline(&target[lcp..]) {
            return;
        }
        self.repair_to_screen(target);
    }

    /// Inline: mirror the det-filtered live text on screen, underlined, via
    /// minimal repairs (the common case is a pure extension: zero backspaces,
    /// one typed delta). No sentence holdback: the raw tail streams ~a beat
    /// behind speech and tail flaps simply repair in place. Polish coverage
    /// whose source no longer binds the new filtered text is dropped first
    /// (its screen region falls back to raw underline).
    fn inline_stream(&mut self, committed: &str, focus: Option<&str>) {
        let Some(cfg) = self.cfg_live.as_ref() else {
            return;
        };
        let filtered = crate::pipeline::run_deterministic(committed, cfg);
        while self
            .inline_chunks
            .last()
            .is_some_and(|(_, src)| !filtered.starts_with(src.as_str()))
        {
            self.inline_chunks.pop();
        }
        self.inline_raw = filtered;
        self.inline_repair(focus);
    }

    /// Compute Inline's desired on-screen text: the underlined polished
    /// sentences, then a space, then the underlined raw tail beyond the
    /// polish coverage.
    fn inline_desired(&self) -> String {
        let covered = self
            .inline_chunks
            .last()
            .map_or(0, |(_, src)| src.len())
            .min(self.inline_raw.len());
        let mut plain = String::new();
        for (cleaned, _) in &self.inline_chunks {
            if cleaned.is_empty() {
                continue;
            }
            if !plain.is_empty() {
                plain.push(' ');
            }
            plain.push_str(cleaned);
        }
        let tail = self.inline_raw[covered..].trim_start();
        let mut out = underline(&plain);
        if !tail.is_empty() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&underline(tail));
        }
        out
    }

    /// Repair the screen to Inline's desired text (guards shared with every
    /// other live path: focus freeze, newline freeze, sink freeze).
    fn inline_repair(&mut self, focus: Option<&str>) {
        let desired = self.inline_desired();
        if desired == self.injected {
            return;
        }
        if !self.focus_ok(focus) {
            return;
        }
        let lcp = grapheme_lcp_bytes(&self.injected, &desired);
        if self.freeze_on_newline(&desired[lcp..]) {
            return;
        }
        self.repair_to_screen(&desired);
    }

    /// CleanModel: the LiveCleaner stored chunk `index`. A repeated index
    /// means a cue-glue re-clean replaced that chunk: backspace its injected
    /// suffix and retype. Chunks join with a single space, exactly like the
    /// finalize stitch, so a bound final text extends the ledger.
    /// Inline: the chunk's polish replaces the underlined raw region its
    /// `src_prefix` covers, in place; a stale chunk (prefix no longer binds)
    /// is skipped.
    fn on_chunk(&mut self, index: usize, cleaned: &str, src_prefix: &str, focus: Option<&str>) {
        if self.phase != Phase::Active {
            return;
        }
        if self.mode == InjectMode::Inline {
            self.inline_chunk(index, cleaned, src_prefix, focus);
            return;
        }
        if self.mode != InjectMode::CleanModel {
            return;
        }
        if index > self.chunk_lens.len() {
            warn!(
                "textbox injector frozen: cleaner chunk {} skipped past {} injected chunks",
                index,
                self.chunk_lens.len()
            );
            self.phase = Phase::Frozen(FreezeReason::Rewrite);
            return;
        }
        if !self.focus_ok(focus) {
            return;
        }
        while self.chunk_lens.len() > index {
            let n_bytes = *self.chunk_lens.last().unwrap();
            let start = self.injected.len() - n_bytes;
            let n = grapheme_count(&self.injected[start..]);
            if n > 0 {
                if let Err(e) = self.sink.backspace(n) {
                    warn!("textbox injector frozen: chunk repair backspace failed: {e}");
                    self.phase = Phase::Frozen(FreezeReason::SinkError);
                    return;
                }
            }
            self.injected.truncate(start);
            self.chunk_lens.pop();
        }
        let part = cleaned.trim();
        let contribution = if part.is_empty() {
            String::new() // ledger keeps a zero-length slot to stay index-aligned
        } else if self.injected.is_empty() {
            part.to_string()
        } else {
            format!(" {part}")
        };
        if contribution.is_empty() {
            self.chunk_lens.push(0);
            return;
        }
        if self.freeze_on_newline(&contribution) {
            return;
        }
        if self.type_or_freeze(&contribution) {
            self.chunk_lens.push(contribution.len());
        }
    }

    /// Inline's chunk handler: record the polish (a repeated index means a
    /// cue-glue re-clean replaced that chunk, exactly like CleanModel) and
    /// repair the screen. Stale chunks, whose source prefix no longer binds
    /// the observed live text, are SKIPPED, not frozen: the raw underline
    /// keeps streaming and the finalize repair has the last word anyway.
    fn inline_chunk(&mut self, index: usize, cleaned: &str, src_prefix: &str, focus: Option<&str>) {
        if index > self.inline_chunks.len() {
            warn!(
                "inline: cleaner chunk {} skipped past {} recorded chunks; ignoring it",
                index,
                self.inline_chunks.len()
            );
            return;
        }
        if !self.inline_raw.starts_with(src_prefix) {
            if src_prefix.starts_with(self.inline_raw.as_str()) {
                // The cleaner's tick saw NEWER committed text than our last
                // stream commit; its filtered prefix is tomorrow's raw text,
                // adopt it (the next commit extends it anyway).
                self.inline_raw = src_prefix.to_string();
            } else {
                debug!("inline: stale cleaner chunk skipped (source prefix no longer binds)");
                return;
            }
        }
        self.inline_chunks.truncate(index);
        self.inline_chunks
            .push((cleaned.trim().to_string(), src_prefix.to_string()));
        self.inline_repair(focus);
    }

    /// Repair the target app's text in place to `target` mid-dictation:
    /// backspace the ledger's tail beyond the grapheme-aligned common
    /// prefix, type the missing suffix, and keep streaming. Freezes ONLY on
    /// a sink failure (divergence is normal business here, unlike the old
    /// freeze ladder). Shared by CleanDet's mind-change repairs and (round
    /// 2) the Inline style's tail flaps and chunk polish. Returns whether the
    /// screen now matches `target`.
    fn repair_to_screen(&mut self, target: &str) -> bool {
        let keep = grapheme_lcp_bytes(&self.injected, target);
        let to_delete = grapheme_count(&self.injected[keep..]);
        if to_delete > 0 {
            if let Err(e) = self.sink.backspace(to_delete) {
                warn!("textbox injector frozen: repair backspace failed: {e}");
                self.phase = Phase::Frozen(FreezeReason::SinkError);
                return false;
            }
        }
        self.injected.truncate(keep);
        let suffix = target[keep..].to_string();
        if suffix.is_empty() {
            return true;
        }
        self.type_or_freeze(&suffix)
    }

    /// Repair the target app's text to `final_text` (+ the trailing space
    /// when enabled): backspace the ledger down to the grapheme-aligned
    /// common prefix, then TYPE the remainder through the sink FIFO
    /// (ordering guaranteed, no clipboard involved: a long full rewrite
    /// visibly retypes, the accepted trade for killing the stale-paste
    /// race). Only a remainder with a newline (multi-line templates; typed
    /// Returns could submit chats) goes to the caller for one hardened
    /// clipboard paste. See `FinalizeOutcome` for the ladder.
    fn finalize(&mut self, final_text: &str, focus: Option<&str>) -> FinalizeOutcome {
        if self.phase == Phase::Done {
            warn!("textbox injector finalize called after it already finished");
            return FinalizeOutcome::Failed("injector already finished".to_string());
        }
        if self.phase == Phase::Frozen(FreezeReason::FocusChanged) || !self.focus_ok(focus) {
            self.phase = Phase::Done;
            return FinalizeOutcome::SkippedFocusChanged;
        }
        let mut target = final_text.to_string();
        if self.append_trailing_space {
            target.push(' ');
        }
        let keep = grapheme_lcp_bytes(&self.injected, &target);
        let to_delete = grapheme_count(&self.injected[keep..]);
        if to_delete > 0 {
            if let Err(e) = self.sink.backspace(to_delete) {
                self.phase = Phase::Done;
                return FinalizeOutcome::Failed(format!("repair backspace failed: {e}"));
            }
        }
        self.phase = Phase::Done;
        let remainder = target[keep..].to_string();
        if remainder.is_empty() {
            return FinalizeOutcome::Injected;
        }
        if remainder.contains('\n') {
            return FinalizeOutcome::NeedsPaste {
                remainder,
                deleted: to_delete,
            };
        }
        match self.sink.type_text(&remainder) {
            Ok(()) => FinalizeOutcome::Injected,
            Err(e) => FinalizeOutcome::Failed(format!("repair typing failed: {e}")),
        }
    }

    /// Cancel: wipe the entire ledger from the target app (focus-guarded so a
    /// cancel after an app switch never deletes someone else's text).
    fn cancel(&mut self, focus: Option<&str>) {
        let was = std::mem::replace(&mut self.phase, Phase::Done);
        if was == Phase::Done || was == Phase::Frozen(FreezeReason::FocusChanged) {
            return;
        }
        if focus != Some(self.home_bundle.as_str()) {
            debug!("textbox injector cancel: focus moved; leaving injected text untouched");
            return;
        }
        let n = grapheme_count(&self.injected);
        if n > 0 {
            if let Err(e) = self.sink.backspace(n) {
                warn!("textbox injector cancel wipe aborted: {e}");
            }
        }
        self.injected.clear();
        self.chunk_lens.clear();
        self.inline_chunks.clear();
        self.inline_raw.clear();
    }
}

// ---------------------------------------------------------------------------
// Production wrapper: worker thread + main-thread sink.
// ---------------------------------------------------------------------------

enum Cmd {
    Committed(String),
    Chunk {
        index: usize,
        cleaned: String,
        src_prefix: String,
    },
    Finalize {
        final_text: String,
        reply: mpsc::Sender<FinalResult>,
    },
    Cancel,
}

/// Handle to the active dictation's injection worker. Cheap to clone via the
/// slot's `Arc`; dropping the last handle ends the worker after it drains.
pub struct StreamInjector {
    tx: mpsc::Sender<Cmd>,
    mode: InjectMode,
}

impl StreamInjector {
    /// Preflight and spawn the worker. `None` = this dictation degrades to
    /// Bar behavior (secure input active, no captured home app, or the input
    /// system is not initialized yet).
    pub fn try_create(
        app: &AppHandle,
        mode: InjectMode,
        snapshot: &crate::pipeline::DictationSnapshot,
        settings: &crate::settings::AppSettings,
    ) -> Option<Arc<Self>> {
        let home = snapshot
            .ctx
            .as_ref()
            .map(|c| c.bundle_id.clone())
            .filter(|b| !b.is_empty());
        if !preflight_allows(secure_input_active(), home.as_deref()) {
            info!(
                "textbox streaming refused by preflight (secure input, or no captured home app); \
                 Bar behavior for this dictation"
            );
            return None;
        }
        if app.try_state::<EnigoState>().is_none() {
            warn!("textbox streaming refused: input system not initialized; Bar behavior");
            return None;
        }
        let home_bundle = home.expect("preflight_allows verified a home bundle");
        info!("textbox streaming active ({mode:?}) into '{home_bundle}'");
        let core = InjectorCore {
            sink: MainThreadSink { app: app.clone() },
            mode,
            cfg_live: matches!(mode, InjectMode::CleanDet | InjectMode::Inline)
                .then(|| snapshot.cfg_live.clone()),
            append_trailing_space: settings.append_trailing_space,
            home_bundle,
            phase: Phase::Active,
            injected: String::new(),
            chunk_lens: Vec::new(),
            inline_raw: String::new(),
            inline_chunks: Vec::new(),
        };
        let (tx, rx) = mpsc::channel();
        let worker_app = app.clone();
        let keep_result_on_clipboard = settings.keep_result_on_clipboard;
        std::thread::Builder::new()
            .name("textbox-injector".to_string())
            .spawn(move || worker_loop(worker_app, core, rx, keep_result_on_clipboard))
            .ok()?;
        Some(Arc::new(StreamInjector { tx, mode }))
    }

    /// Whether this injector consumes stream commits directly (Raw,
    /// CleanDet, and Inline); CleanModel is fed by the LiveCleaner instead
    /// (Inline consumes BOTH: commits for the raw underline, chunks for the
    /// in-place polish).
    pub fn wants_committed(&self) -> bool {
        self.mode != InjectMode::CleanModel
    }

    pub fn on_committed(&self, committed: &str) {
        let _ = self.tx.send(Cmd::Committed(committed.to_string()));
    }

    pub fn on_chunk(&self, index: usize, cleaned: &str, src_prefix: &str) {
        let _ = self.tx.send(Cmd::Chunk {
            index,
            cleaned: cleaned.to_string(),
            src_prefix: src_prefix.to_string(),
        });
    }

    /// Fire-and-forget wipe (queued FIFO behind in-flight deltas). Never
    /// blocks: cancel can be triggered from the main thread (tray menu).
    pub fn cancel(&self) {
        let _ = self.tx.send(Cmd::Cancel);
    }

    /// Repair the injected text to `final_text` and report how it went.
    /// Blocks the CALLING thread on the worker's reply, so it must never be
    /// called on the main thread (the stop path calls it from its async task
    /// via `spawn_blocking`).
    pub fn finalize(&self, final_text: &str) -> FinalResult {
        let (reply_tx, reply_rx) = mpsc::channel();
        if self
            .tx
            .send(Cmd::Finalize {
                final_text: final_text.to_string(),
                reply: reply_tx,
            })
            .is_err()
        {
            return FinalResult::Error("injection worker is gone".to_string());
        }
        match reply_rx.recv_timeout(FINALIZE_REPLY_TIMEOUT) {
            Ok(result) => result,
            Err(e) => FinalResult::Error(format!("injection finalize timed out: {e}")),
        }
    }
}

fn current_focus() -> Option<String> {
    crate::context::capture_foreground_app().map(|c| c.bundle_id)
}

fn worker_loop(
    app: AppHandle,
    mut core: InjectorCore<MainThreadSink>,
    rx: mpsc::Receiver<Cmd>,
    keep_result_on_clipboard: bool,
) {
    use std::collections::VecDeque;
    let mut queue: VecDeque<Cmd> = VecDeque::new();
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(cmd) => queue.push_back(cmd),
                Err(_) => break, // every handle dropped; dictation is over
            }
        }
        queue.extend(rx.try_iter());
        // Consecutive committed snapshots supersede each other (each carries
        // the FULL committed text), so a backlog collapses to its newest.
        while queue.len() >= 2
            && matches!(queue[0], Cmd::Committed(_))
            && matches!(queue[1], Cmd::Committed(_))
        {
            queue.pop_front();
        }
        let cmd = queue.pop_front().expect("queue refilled above");
        let focus = current_focus();
        match cmd {
            Cmd::Committed(committed) => core.on_committed(&committed, focus.as_deref()),
            Cmd::Chunk {
                index,
                cleaned,
                src_prefix,
            } => core.on_chunk(index, &cleaned, &src_prefix, focus.as_deref()),
            Cmd::Cancel => core.cancel(focus.as_deref()),
            Cmd::Finalize { final_text, reply } => {
                let outcome = core.finalize(&final_text, focus.as_deref());
                let result = conclude(
                    &app,
                    outcome,
                    &final_text,
                    core.append_trailing_space,
                    keep_result_on_clipboard,
                );
                let _ = reply.send(result);
            }
        }
    }
}

/// Turn the core's finalize outcome into user-visible effects: the remainder
/// paste when the repair tail was too long to type, and the
/// keep-result-on-clipboard write (always the FULL final text, with the same
/// trailing-space semantics as the normal paste path). A focus-changed skip
/// still honors keep-on-clipboard, WITHOUT pasting.
fn conclude(
    app: &AppHandle,
    outcome: FinalizeOutcome,
    final_text: &str,
    append_trailing_space: bool,
    keep_result_on_clipboard: bool,
) -> FinalResult {
    let full = if append_trailing_space {
        format!("{final_text} ")
    } else {
        final_text.to_string()
    };
    let keep = |app: &AppHandle| {
        if keep_result_on_clipboard {
            write_clipboard_on_main(app, full.clone());
        }
    };
    match outcome {
        FinalizeOutcome::Injected => {
            keep(app);
            FinalResult::Inserted
        }
        FinalizeOutcome::NeedsPaste {
            mut remainder,
            deleted,
        } => {
            // Let the target drain the repair's backspace flood before the
            // paste keystroke joins its queue (we are on the worker thread,
            // never the main thread, so sleeping here is safe).
            let settle = PASTE_SETTLE_PER_BACKSPACE
                .saturating_mul(u32::try_from(deleted).unwrap_or(u32::MAX))
                .min(PASTE_SETTLE_MAX);
            if !settle.is_zero() {
                std::thread::sleep(settle);
            }
            // The trailing space was folded into the repair target for the
            // TYPED path; a clipboard paste must deliver it as a keystroke
            // instead (rich-text editors trim pasted trailing whitespace),
            // so strip the folded copy and let the paste options re-add it
            // as a keypress after the paste. Keep-on-clipboard is handled
            // here with the FULL text instead of the remainder (and makes
            // the clipboard restore pointless, so it is skipped outright).
            // The restore that does happen waits longer than the normal
            // paste path's: see REMAINDER_PASTE_RESTORE_DELAY_MS.
            if append_trailing_space && remainder.ends_with(' ') {
                remainder.pop();
            }
            let opts = crate::clipboard::PasteOptions {
                append_trailing_space,
                keep_result_on_clipboard: false,
                restore_delay_ms: REMAINDER_PASTE_RESTORE_DELAY_MS,
                skip_restore: keep_result_on_clipboard,
            };
            match paste_on_main(app, remainder, opts) {
                Ok(()) => {
                    keep(app);
                    FinalResult::Inserted
                }
                Err(e) => FinalResult::Error(format!("remainder paste failed: {e}")),
            }
        }
        FinalizeOutcome::SkippedFocusChanged => {
            keep(app);
            FinalResult::SkippedFocusChanged
        }
        FinalizeOutcome::Failed(e) => FinalResult::Error(e),
    }
}

/// Run `f` on the main thread and wait (bounded) for its result. Must only be
/// called from a worker thread, never from the main thread itself (the
/// injection worker and the F4 post-paste AX watcher both use it).
pub(crate) fn run_on_main_blocking<T: Send + 'static>(
    app: &AppHandle,
    timeout: Duration,
    f: impl FnOnce(&AppHandle) -> T + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    let ah = app.clone();
    app.run_on_main_thread(move || {
        let _ = tx.send(f(&ah));
    })
    .map_err(|e| format!("main-thread dispatch failed: {e}"))?;
    rx.recv_timeout(timeout)
        .map_err(|e| format!("main-thread operation timed out: {e}"))
}

fn paste_on_main(
    app: &AppHandle,
    text: String,
    opts: crate::clipboard::PasteOptions,
) -> Result<(), String> {
    run_on_main_blocking(app, PASTE_OP_TIMEOUT, move |ah| {
        crate::clipboard::paste_with_options(text, ah.clone(), opts)
    })?
}

fn write_clipboard_on_main(app: &AppHandle, text: String) {
    let result = run_on_main_blocking(app, SINK_OP_TIMEOUT, move |ah| {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        ah.clipboard()
            .write_text(&text)
            .map_err(|e| format!("Failed to copy to clipboard: {e}"))
    });
    if let Ok(Err(e)) | Err(e) = result {
        warn!("keep-result-on-clipboard write failed: {e}");
    }
}

/// Production sink: every operation is posted to the main thread (CGEvent
/// posting through the managed Enigo must happen there) and awaited, so the
/// worker's FIFO order is also the order text reaches the target app.
/// Backspaces go in small batches with tiny spacing so a long deletion never
/// monopolizes the event loop and target apps do not drop key events.
struct MainThreadSink {
    app: AppHandle,
}

impl MainThreadSink {
    fn with_enigo<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Enigo) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        run_on_main_blocking(&self.app, SINK_OP_TIMEOUT, move |ah| {
            let enigo_state = ah
                .try_state::<EnigoState>()
                .ok_or_else(|| "Enigo state not initialized".to_string())?;
            let mut enigo = enigo_state
                .0
                .lock()
                .map_err(|e| format!("Failed to lock Enigo: {e}"))?;
            f(&mut enigo)
        })?
    }
}

impl InjectSink for MainThreadSink {
    fn type_text(&mut self, s: &str) -> Result<(), String> {
        let s = s.to_string();
        self.with_enigo(move |enigo| input::paste_text_direct(enigo, &s))
    }

    fn backspace(&mut self, n: usize) -> Result<(), String> {
        let mut left = n;
        while left > 0 {
            let batch = left.min(BACKSPACE_BATCH);
            self.with_enigo(move |enigo| {
                for _ in 0..batch {
                    input::send_backspace(enigo)?;
                    std::thread::sleep(BACKSPACE_SPACING);
                }
                Ok(())
            })?;
            left -= batch;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests (mock sink; the plan's F5 stream_inject rows).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: Option<&str> = Some("com.apple.textedit");
    const ELSEWHERE: Option<&str> = Some("com.apple.safari");

    #[derive(Debug, PartialEq, Eq, Clone)]
    enum Op {
        Type(String),
        Backspace(usize),
    }

    /// Mock sink that also SIMULATES the target textbox (`typed`), so tests
    /// assert both the operation stream and the resulting text.
    #[derive(Default)]
    struct MockSink {
        ops: Vec<Op>,
        typed: String,
        fail_type: bool,
        fail_backspace: bool,
    }

    impl InjectSink for MockSink {
        fn type_text(&mut self, s: &str) -> Result<(), String> {
            if self.fail_type {
                return Err("type refused".to_string());
            }
            self.ops.push(Op::Type(s.to_string()));
            self.typed.push_str(s);
            Ok(())
        }

        fn backspace(&mut self, n: usize) -> Result<(), String> {
            if self.fail_backspace {
                return Err("backspace refused".to_string());
            }
            self.ops.push(Op::Backspace(n));
            let graphemes: Vec<&str> = self.typed.graphemes(true).collect();
            let keep = graphemes.len().saturating_sub(n);
            let keep_bytes: usize = graphemes[..keep].iter().map(|g| g.len()).sum();
            self.typed.truncate(keep_bytes);
            Ok(())
        }
    }

    fn core(mode: InjectMode, trailing_space: bool) -> InjectorCore<MockSink> {
        let cfg_live = matches!(mode, InjectMode::CleanDet | InjectMode::Inline).then(|| {
            // These rows exercise the DETERMINISTIC live pass; round-2
            // defaults put mind-change on the Model engine, so pin it back.
            let mut settings = crate::settings::get_default_settings();
            settings.mind_change_engine = crate::settings::StageEngine::Deterministic;
            settings.mind_change_level = crate::settings::FeatureLevel::Medium;
            StageConfig::from_settings(&settings, true, false, None)
        });
        InjectorCore {
            sink: MockSink::default(),
            mode,
            cfg_live,
            append_trailing_space: trailing_space,
            home_bundle: "com.apple.textedit".to_string(),
            phase: Phase::Active,
            injected: String::new(),
            chunk_lens: Vec::new(),
            inline_raw: String::new(),
            inline_chunks: Vec::new(),
        }
    }

    // ---- preflight ----

    #[test]
    fn preflight_short_circuits_on_secure_input_or_missing_home() {
        assert!(preflight_allows(false, Some("com.apple.notes")));
        assert!(!preflight_allows(true, Some("com.apple.notes")));
        assert!(!preflight_allows(false, None));
        assert!(!preflight_allows(false, Some("")));
        assert!(!preflight_allows(true, None));
    }

    // ---- grapheme math ----

    #[test]
    fn grapheme_counts_handle_combining_marks_and_zwj() {
        assert_eq!(grapheme_count("cafe\u{301}"), 4); // e + combining acute = one grapheme
        assert_eq!(grapheme_count("caf\u{e9}"), 4); // precomposed
        assert_eq!(
            grapheme_count("hi \u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467}"),
            4
        ); // ZWJ family = one
    }

    #[test]
    fn lcp_is_grapheme_aligned_across_forms() {
        // Combining form vs precomposed: the final grapheme differs as a
        // STRING, so the prefix must stop before it, never inside it.
        assert_eq!(grapheme_lcp_bytes("cafe\u{301}", "caf\u{e9}"), 3);
        assert_eq!(grapheme_lcp_bytes("hello", "hello"), 5);
        assert_eq!(grapheme_lcp_bytes("hello world", "hello would"), 8);
        assert_eq!(grapheme_lcp_bytes("", "anything"), 0);
    }

    // ---- Raw mode ----

    #[test]
    fn raw_types_append_only_deltas() {
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("hello", HOME);
        c.on_committed("hello", HOME); // no growth, no op
        c.on_committed("hello there", HOME);
        assert_eq!(
            c.sink.ops,
            vec![
                Op::Type("hello".to_string()),
                Op::Type(" there".to_string())
            ]
        );
        assert_eq!(c.sink.typed, "hello there");
        assert_eq!(c.phase, Phase::Active);
    }

    #[test]
    fn raw_skips_empty_commits() {
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("", HOME);
        assert!(c.sink.ops.is_empty());
        assert_eq!(c.phase, Phase::Active);
    }

    #[test]
    fn raw_freezes_on_rewrite_and_ignores_later_commits() {
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("hello there", HOME);
        c.on_committed("help there friend", HOME); // not an extension
        assert_eq!(c.phase, Phase::Frozen(FreezeReason::Rewrite));
        c.on_committed("help there friend more", HOME);
        assert_eq!(c.sink.ops.len(), 1);
        assert_eq!(c.sink.typed, "hello there");
    }

    #[test]
    fn focus_change_freezes_before_typing() {
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("hello", HOME);
        c.on_committed("hello there", ELSEWHERE);
        assert_eq!(c.phase, Phase::Frozen(FreezeReason::FocusChanged));
        assert_eq!(c.sink.typed, "hello"); // the delta was never typed
                                           // An unreadable focus is treated the same way.
        let mut c2 = core(InjectMode::Raw, true);
        c2.on_committed("hi", None);
        assert_eq!(c2.phase, Phase::Frozen(FreezeReason::FocusChanged));
    }

    #[test]
    fn failed_delta_freezes_and_keeps_ledger_confirmed_only() {
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("hello", HOME);
        c.sink.fail_type = true;
        c.on_committed("hello there", HOME);
        assert_eq!(c.phase, Phase::Frozen(FreezeReason::SinkError));
        assert_eq!(c.injected, "hello"); // failed delta never entered the ledger
                                         // Finalize still repairs (focus unchanged): heal the sink first.
        c.sink.fail_type = false;
        let out = c.finalize("Hello there.", HOME);
        assert_eq!(out, FinalizeOutcome::Injected);
        assert_eq!(c.sink.typed, "Hello there. ");
    }

    // ---- finalize repair ----

    #[test]
    fn finalize_repairs_via_common_prefix() {
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("hello wrold and", HOME);
        let out = c.finalize("Hello world and more.", HOME);
        assert_eq!(out, FinalizeOutcome::Injected);
        // LCP("hello wrold and", "Hello world and more. ") is empty (case
        // differs at byte 0): wipe and retype.
        assert_eq!(c.sink.ops[1], Op::Backspace(15));
        assert_eq!(c.sink.typed, "Hello world and more. ");
        assert_eq!(c.phase, Phase::Done);
    }

    #[test]
    fn finalize_extends_matching_prefix_without_deleting() {
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("hello there", HOME);
        // Repair only the un-typed tail: no backspaces at all.
        let out = c.finalize("hello there friend.", HOME);
        assert_eq!(out, FinalizeOutcome::Injected);
        assert!(c.sink.ops.iter().all(|op| !matches!(op, Op::Backspace(_))));
        assert_eq!(c.sink.typed, "hello there friend. ");
    }

    #[test]
    fn finalize_backspace_counts_are_graphemes_not_bytes() {
        let mut c = core(InjectMode::Raw, false);
        // "cafe" + combining acute: 5 chars, 4 graphemes.
        c.on_committed("cafe\u{301} au lait", HOME);
        let out = c.finalize("caf\u{e9} au lait", HOME);
        assert_eq!(out, FinalizeOutcome::Injected);
        // Ledger tail beyond "caf" is "e\u{301} au lait" = 9 graphemes.
        assert_eq!(c.sink.ops[1], Op::Backspace(9));
        assert_eq!(c.sink.typed, "caf\u{e9} au lait");
    }

    #[test]
    fn finalize_replaces_a_zwj_emoji_as_one_grapheme() {
        let mut c = core(InjectMode::Raw, false);
        c.on_committed("hi \u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467}", HOME);
        let out = c.finalize("hi \u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F466}", HOME);
        assert_eq!(out, FinalizeOutcome::Injected);
        assert_eq!(c.sink.ops[1], Op::Backspace(1)); // whole family, one backspace
        assert_eq!(
            c.sink.typed,
            "hi \u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F466}"
        );
    }

    #[test]
    fn finalize_types_any_length_remainder_without_newlines() {
        // Short tail: typed through the sink.
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("hello", HOME);
        let out = c.finalize("hello plus a short tail", HOME);
        assert_eq!(out, FinalizeOutcome::Injected);
        assert_eq!(
            c.sink.ops.last(),
            Some(&Op::Type(" plus a short tail ".to_string()))
        );

        // LONG tail: still typed (the FIFO guarantees ordering; a visible
        // retype beats the clipboard race that pasted stale contents).
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("hello", HOME);
        let long_tail = "x".repeat(200);
        let out = c.finalize(&format!("hello {long_tail}"), HOME);
        assert_eq!(out, FinalizeOutcome::Injected);
        assert_eq!(c.sink.typed, format!("hello {long_tail} "));
    }

    #[test]
    fn finalize_pastes_newline_remainders_with_the_deleted_count() {
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("hello wrold", HOME);
        // The final text repairs "wrold" AND carries a template's newlines:
        // deletion count rides along so the paste can wait out the flood.
        match c.finalize("hello world.\nSecond line.", HOME) {
            FinalizeOutcome::NeedsPaste { remainder, deleted } => {
                assert_eq!(remainder, "orld.\nSecond line. ");
                assert_eq!(deleted, 4, "backspaces for the 'rold' tail");
            }
            other => panic!("expected NeedsPaste, got {other:?}"),
        }
        // Nothing was typed after the deletion: the caller pastes.
        assert_eq!(c.sink.typed, "hello w");
        assert_eq!(c.sink.ops.last(), Some(&Op::Backspace(4)));
    }

    #[test]
    fn finalize_with_empty_ledger_pastes_multi_line_text() {
        // No commits ever arrived; a multi-line final (phrase template) must
        // still go through the paste branch, zero deletions.
        let mut c = core(InjectMode::Raw, true);
        match c.finalize("Hi team,\n\nStatus below.", HOME) {
            FinalizeOutcome::NeedsPaste { remainder, deleted } => {
                assert_eq!(remainder, "Hi team,\n\nStatus below. ");
                assert_eq!(deleted, 0);
            }
            other => panic!("expected NeedsPaste, got {other:?}"),
        }
        assert!(c.sink.ops.is_empty());
    }

    #[test]
    fn finalize_trailing_space_rides_both_branches() {
        // Typed branch: the trailing space is folded into the typed tail.
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("hello", HOME);
        assert_eq!(c.finalize("hello there", HOME), FinalizeOutcome::Injected);
        assert_eq!(c.sink.typed, "hello there ");

        // Paste branch: folded into the remainder, never added twice.
        let mut c = core(InjectMode::Raw, true);
        match c.finalize("a\nb", HOME) {
            FinalizeOutcome::NeedsPaste { remainder, .. } => {
                assert_eq!(remainder, "a\nb ");
            }
            other => panic!("expected NeedsPaste, got {other:?}"),
        }

        // Trailing space off: the remainder is the bare text.
        let mut c = core(InjectMode::Raw, false);
        match c.finalize("a\nb", HOME) {
            FinalizeOutcome::NeedsPaste { remainder, .. } => {
                assert_eq!(remainder, "a\nb");
            }
            other => panic!("expected NeedsPaste, got {other:?}"),
        }
    }

    #[test]
    fn trailing_space_is_applied_exactly_once() {
        // Ledger equals the final text: the only op is the one space.
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("hello there.", HOME);
        let out = c.finalize("hello there.", HOME);
        assert_eq!(out, FinalizeOutcome::Injected);
        assert_eq!(c.sink.ops.last(), Some(&Op::Type(" ".to_string())));
        assert_eq!(c.sink.typed, "hello there. ");

        // Trailing space disabled: ledger == target, zero repair ops.
        let mut c = core(InjectMode::Raw, false);
        c.on_committed("hello there.", HOME);
        let before = c.sink.ops.len();
        assert_eq!(c.finalize("hello there.", HOME), FinalizeOutcome::Injected);
        assert_eq!(c.sink.ops.len(), before);
        assert_eq!(c.sink.typed, "hello there.");
    }

    #[test]
    fn finalize_with_empty_ledger_still_delivers_the_text() {
        // No commits ever arrived (e.g. instant stop): behave like a paste.
        let mut c = core(InjectMode::Raw, true);
        assert_eq!(c.finalize("Hi.", HOME), FinalizeOutcome::Injected);
        assert_eq!(c.sink.typed, "Hi. ");
    }

    // ---- freeze ladders at finalize ----

    #[test]
    fn frozen_focus_finalize_is_a_sink_noop() {
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("hello there", HOME);
        c.on_committed("hello there friend", ELSEWHERE); // freezes
        let ops_before = c.sink.ops.len();
        let out = c.finalize("Hello there friend.", HOME);
        assert_eq!(out, FinalizeOutcome::SkippedFocusChanged);
        assert_eq!(c.sink.ops.len(), ops_before, "no deletes, no typing");
        assert_eq!(c.sink.typed, "hello there", "streamed text left in place");
        assert_eq!(c.phase, Phase::Done);
    }

    #[test]
    fn focus_change_at_finalize_time_skips_repair() {
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("hello", HOME);
        let out = c.finalize("Hello.", ELSEWHERE);
        assert_eq!(out, FinalizeOutcome::SkippedFocusChanged);
        assert_eq!(c.sink.typed, "hello");
    }

    #[test]
    fn rewrite_freeze_still_repairs_at_finalize() {
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("hello there", HOME);
        c.on_committed("hxllo there more", HOME); // rewrite -> freeze
        assert_eq!(c.phase, Phase::Frozen(FreezeReason::Rewrite));
        let out = c.finalize("Hello there more.", HOME);
        assert_eq!(out, FinalizeOutcome::Injected);
        assert_eq!(c.sink.typed, "Hello there more. ");
    }

    #[test]
    fn failed_backspace_during_repair_aborts_without_pasting() {
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("hello wrold", HOME);
        c.sink.fail_backspace = true;
        let out = c.finalize("hello world.", HOME);
        assert!(matches!(out, FinalizeOutcome::Failed(_)));
        // Nothing typed after the failed deletion: never duplicate text.
        assert_eq!(c.sink.ops, vec![Op::Type("hello wrold".to_string())]);
        assert_eq!(c.sink.typed, "hello wrold");
        assert_eq!(c.phase, Phase::Done);
    }

    // ---- cancel ----

    #[test]
    fn cancel_wipes_the_entire_ledger() {
        let mut c = core(InjectMode::Raw, true);
        c.on_committed(
            "hello \u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467} there",
            HOME,
        );
        c.cancel(HOME);
        assert_eq!(c.sink.ops.last(), Some(&Op::Backspace(13)));
        assert_eq!(c.sink.typed, "");
        assert_eq!(c.phase, Phase::Done);
        // Idempotent: a second cancel is a no-op.
        let ops = c.sink.ops.len();
        c.cancel(HOME);
        assert_eq!(c.sink.ops.len(), ops);
    }

    #[test]
    fn cancel_after_focus_change_leaves_the_other_app_alone() {
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("hello", HOME);
        c.cancel(ELSEWHERE);
        assert_eq!(c.sink.typed, "hello", "no deletes outside the home app");
        assert_eq!(c.phase, Phase::Done);
    }

    // ---- CleanModel chunks ----

    #[test]
    fn clean_chunks_join_with_single_spaces() {
        let mut c = core(InjectMode::CleanModel, true);
        c.on_chunk(0, "First sentence.", "", HOME);
        c.on_chunk(1, "Second one.", "", HOME);
        assert_eq!(c.sink.typed, "First sentence. Second one.");
        assert_eq!(c.chunk_lens, vec![15, 12]);
    }

    #[test]
    fn clean_chunk_repair_backspaces_and_retypes() {
        let mut c = core(InjectMode::CleanModel, true);
        c.on_chunk(0, "First sentence.", "", HOME);
        c.on_chunk(1, "Send it to John.", "", HOME);
        // Cue-glue re-clean replaced chunk 1 with a joint result (grows).
        c.on_chunk(1, "Send it to Joan from accounting.", "", HOME);
        assert_eq!(
            c.sink.ops[2..],
            vec![
                Op::Backspace(17), // " Send it to John." is 17 graphemes
                Op::Type(" Send it to Joan from accounting.".to_string()),
            ]
        );
        assert_eq!(
            c.sink.typed,
            "First sentence. Send it to Joan from accounting."
        );
        // And a re-clean that SHRINKS the chunk.
        c.on_chunk(1, "Joan.", "", HOME);
        assert_eq!(c.sink.typed, "First sentence. Joan.");
        assert_eq!(c.chunk_lens, vec![15, 6]);
    }

    #[test]
    fn clean_empty_chunk_keeps_indices_aligned() {
        let mut c = core(InjectMode::CleanModel, true);
        c.on_chunk(0, "   ", "", HOME); // model returned whitespace
        c.on_chunk(1, "Real text.", "", HOME);
        assert_eq!(c.sink.typed, "Real text.");
        assert_eq!(c.chunk_lens, vec![0, 10]);
    }

    #[test]
    fn clean_chunk_index_gap_freezes() {
        let mut c = core(InjectMode::CleanModel, true);
        c.on_chunk(1, "Skipped ahead.", "", HOME);
        assert_eq!(c.phase, Phase::Frozen(FreezeReason::Rewrite));
        assert!(c.sink.ops.is_empty());
    }

    #[test]
    fn clean_finalize_types_only_the_residual_tail() {
        let mut c = core(InjectMode::CleanModel, true);
        c.on_chunk(0, "First sentence.", "", HOME);
        c.on_chunk(1, "Second one.", "", HOME);
        // Stitched final extends the injected join: no deletes, tail only.
        let out = c.finalize("First sentence. Second one. Tail.", HOME);
        assert_eq!(out, FinalizeOutcome::Injected);
        assert!(c.sink.ops.iter().all(|op| !matches!(op, Op::Backspace(_))));
        assert_eq!(c.sink.typed, "First sentence. Second one. Tail. ");
    }

    #[test]
    fn clean_finalize_falls_back_to_prefix_repair_when_shaping_differs() {
        let mut c = core(InjectMode::CleanModel, false);
        c.on_chunk(0, "Sounds good.", "", HOME);
        // Chat context dropped the final period in the shaped final text.
        let out = c.finalize("Sounds good", HOME);
        assert_eq!(out, FinalizeOutcome::Injected);
        assert_eq!(c.sink.ops.last(), Some(&Op::Backspace(1)));
        assert_eq!(c.sink.typed, "Sounds good");
    }

    // ---- CleanDet (deterministic sentence streaming) ----

    #[test]
    fn det_injects_each_newly_completed_sentence_with_holdback() {
        let mut c = core(InjectMode::CleanDet, true);
        // One complete sentence + tail: holdback keeps everything back
        // (the provisional period makes the tail LOOK complete, so it is the
        // held-back sentence and the real first sentence ships).
        c.on_committed("hello there. second part", HOME);
        assert_eq!(c.sink.typed, "Hello there.");
        // More speech completes the second sentence; it ships next.
        c.on_committed("hello there. second part is done. third bit", HOME);
        assert_eq!(c.sink.typed, "Hello there. Second part is done.");
        assert_eq!(c.phase, Phase::Active);
    }

    #[test]
    fn det_applies_the_deterministic_pass_to_injected_sentences() {
        let mut c = core(InjectMode::CleanDet, true);
        // Fillers stripped, mind-change resolved, ITN applied, caps fixed:
        // the injected sentence is the PIPELINE output, not the raw words.
        c.on_committed(
            "so um, at eight, no wait, nine works for me. and then some more words here",
            HOME,
        );
        assert_eq!(c.sink.typed, "So at 9 works for me.");
    }

    #[test]
    fn det_holds_back_cue_opening_sentences_with_their_successor() {
        let mut c = core(InjectMode::CleanDet, true);
        // Second sentence opens with a correction cue that mind-change does
        // not resolve deterministically; the cue walk keeps sentence 1 and
        // the cue sentence together until a successor completes.
        c.on_committed(
            "send the file to john. actually hold on. let me check",
            HOME,
        );
        assert_eq!(c.sink.typed, "Send the file to john.");
    }

    #[test]
    fn det_terminated_tail_ships_immediately() {
        // The raw tail carries a REAL terminator: holdback drops to 0 and
        // the sentence lands without waiting for the next one to begin.
        let mut c = core(InjectMode::CleanDet, true);
        c.on_committed("hello there.", HOME);
        assert_eq!(c.sink.typed, "Hello there.");
        // And the next sentence ships the moment ITS terminator arrives.
        c.on_committed("hello there. second bit is done.", HOME);
        assert_eq!(c.sink.typed, "Hello there. Second bit is done.");
        assert_eq!(c.phase, Phase::Active);
    }

    #[test]
    fn det_unterminated_tail_holds_the_newest_sentence() {
        // Stage-6 shaping gives the growing tail a provisional period, so it
        // LOOKS complete; the raw tail says it is not, so it stays held.
        let mut c = core(InjectMode::CleanDet, true);
        c.on_committed("hello there. second part still going", HOME);
        assert_eq!(c.sink.typed, "Hello there.");
        assert_eq!(c.phase, Phase::Active);
    }

    #[test]
    fn det_retraction_repairs_in_place_instead_of_freezing() {
        let mut c = core(InjectMode::CleanDet, true);
        c.on_committed("first sentence here. second thought comes next. more", HOME);
        assert_eq!(
            c.sink.typed,
            "First sentence here. Second thought comes next."
        );
        // A standalone retraction deletes the already-injected sentence from
        // the deterministic output: the mismatch now repairs LIVE (backspace
        // the dead sentence, type its replacement) and streaming continues.
        c.on_committed(
            "first sentence here. second thought comes next. scratch that. replacement text arrives. end",
            HOME,
        );
        assert_eq!(c.phase, Phase::Active);
        assert_eq!(
            c.sink.typed,
            "First sentence here. Replacement text arrives."
        );
        // Finalize only appends the tail.
        let out = c.finalize("First sentence here. Replacement text arrives. End.", HOME);
        assert_eq!(out, FinalizeOutcome::Injected);
        assert_eq!(
            c.sink.typed,
            "First sentence here. Replacement text arrives. End. "
        );
    }

    #[test]
    fn det_shrink_only_repair_deletes_the_stale_tail() {
        let mut c = core(InjectMode::CleanDet, true);
        c.on_committed("first part done.", HOME); // holdback 0: ships now
        assert_eq!(c.sink.typed, "First part done.");
        // The retraction lands while its replacement is still incomplete:
        // nothing new is eligible, so the repair deletes down to the common
        // prefix and types NOTHING speculative.
        c.on_committed(
            "first part done. scratch that. the replacement is still",
            HOME,
        );
        assert_eq!(c.phase, Phase::Active);
        assert_eq!(c.sink.typed, "");
        // The replacement completes and streams from the repaired prefix.
        c.on_committed(
            "first part done. scratch that. the replacement is still better here. so done",
            HOME,
        );
        assert_eq!(c.sink.typed, "The replacement is still better here.");
    }

    // ---- newline guards (live typing must never send a Return) ----

    #[test]
    fn det_newline_delta_freezes_for_the_finalize_paste() {
        // A multi-line phrase expansion lands INSIDE the deterministic live
        // text (word-joining stages skipped: filler and mind-change ride the
        // Model engine here, and ITN is line-aware): the sentence delta
        // carries a newline, so nothing is typed and the whole delivery
        // defers to the finalize paste branch.
        let mut settings = crate::settings::get_default_settings();
        settings.filler_engine = crate::settings::StageEngine::Model;
        settings.mind_change_engine = crate::settings::StageEngine::Model;
        settings.custom_phrases = vec![crate::settings::CustomPhrase {
            say: "team sign off".to_string(),
            write: "Best,\nPo".to_string(),
        }];
        let cfg = StageConfig::from_settings(&settings, true, false, None);
        let mut c = InjectorCore {
            sink: MockSink::default(),
            mode: InjectMode::CleanDet,
            cfg_live: Some(cfg),
            append_trailing_space: true,
            home_bundle: "com.apple.textedit".to_string(),
            phase: Phase::Active,
            injected: String::new(),
            chunk_lens: Vec::new(),
            inline_raw: String::new(),
            inline_chunks: Vec::new(),
        };
        c.on_committed(
            "team sign off is what i always write. and then more words",
            HOME,
        );
        assert_eq!(c.phase, Phase::Frozen(FreezeReason::Rewrite));
        assert!(c.sink.ops.is_empty(), "the newline delta was never typed");
        // Finalize delivers the whole multi-line text via the paste branch.
        match c.finalize("Best,\nPo is what I always write.", HOME) {
            FinalizeOutcome::NeedsPaste { remainder, deleted } => {
                assert_eq!(remainder, "Best,\nPo is what I always write. ");
                assert_eq!(deleted, 0);
            }
            other => panic!("expected NeedsPaste, got {other:?}"),
        }
    }

    #[test]
    fn chunk_newline_contribution_freezes_for_the_finalize_paste() {
        let mut c = core(InjectMode::CleanModel, true);
        c.on_chunk(0, "First sentence.", "", HOME);
        c.on_chunk(1, "Line one.\nLine two.", "", HOME);
        assert_eq!(c.phase, Phase::Frozen(FreezeReason::Rewrite));
        assert_eq!(c.sink.typed, "First sentence.", "newline chunk not typed");
        assert_eq!(c.chunk_lens, vec![15], "the frozen chunk claimed no slot");
    }

    // ---- Inline (underlined live stream, in-place polish) ----

    #[test]
    fn underline_round_trips_grapheme_counts_incl_zwj() {
        for s in [
            "hello world.",
            "cafe\u{301} au lait",
            "hi \u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467} there",
            "tabs\tand  spaces",
            "",
        ] {
            let u = underline(s);
            assert_eq!(
                grapheme_count(&u),
                grapheme_count(s),
                "underline changed the grapheme count of {s:?}"
            );
        }
        // The mark rides each non-whitespace cluster; whitespace stays bare.
        assert_eq!(underline("ab c"), "a\u{332}b\u{332} c\u{332}");
        // A ZWJ family takes ONE mark, after the whole cluster.
        let fam = "\u{1F469}\u{200D}\u{1F467}";
        assert_eq!(underline(fam), format!("{fam}\u{332}"));
    }

    #[test]
    fn inline_commits_extend_with_underlined_deltas() {
        let mut c = core(InjectMode::Inline, true);
        c.on_committed("hello there", HOME);
        assert_eq!(c.sink.typed, underline("Hello there."));
        // Extension: the provisional period flaps (one backspace), the new
        // words arrive underlined; no freeze anywhere.
        c.on_committed("hello there my friend", HOME);
        assert_eq!(c.sink.typed, underline("Hello there my friend."));
        assert!(c.sink.ops.contains(&Op::Backspace(1)));
        assert_eq!(c.phase, Phase::Active);
    }

    #[test]
    fn inline_tail_flap_repairs_in_place() {
        let mut c = core(InjectMode::Inline, true);
        c.on_committed("hello world", HOME);
        assert_eq!(c.sink.typed, underline("Hello world."));
        // The stream rewrote its tail (degraded mode): Inline repairs live
        // instead of freezing like Raw does.
        c.on_committed("hello word yes", HOME);
        assert_eq!(c.sink.typed, underline("Hello word yes."));
        assert_eq!(c.phase, Phase::Active);
    }

    #[test]
    fn inline_chunk_polish_repairs_the_sentence_in_place() {
        let mut c = core(InjectMode::Inline, true);
        c.on_committed("send it to jhon. and more coming", HOME);
        assert_eq!(c.sink.typed, underline("Send it to jhon. And more coming."));
        // The LiveCleaner's polish for sentence 1 lands while the tail is
        // still raw: the polished sentence replaces its underlined source,
        // still underlined (only the FINAL text drops the marks).
        c.on_chunk(0, "Send it to John.", "Send it to jhon.", HOME);
        assert_eq!(c.sink.typed, underline("Send it to John. And more coming."));
        assert_eq!(c.phase, Phase::Active);
        // The raw tail keeps streaming after the polish.
        c.on_committed("send it to jhon. and more coming now", HOME);
        assert_eq!(
            c.sink.typed,
            underline("Send it to John. And more coming now.")
        );
    }

    #[test]
    fn inline_stale_chunk_is_skipped_not_frozen() {
        let mut c = core(InjectMode::Inline, true);
        c.on_committed("hello there my friend", HOME);
        let before = c.sink.typed.clone();
        // A chunk whose source prefix binds NOTHING we observed: ignore it.
        c.on_chunk(0, "Polished nonsense.", "Entirely different text.", HOME);
        assert_eq!(c.sink.typed, before);
        assert!(c.inline_chunks.is_empty());
        assert_eq!(c.phase, Phase::Active);
        // An index beyond the recorded chunks is likewise ignored.
        c.on_chunk(3, "Orphan.", "Hello there my friend.", HOME);
        assert_eq!(c.sink.typed, before);
        assert_eq!(c.phase, Phase::Active);
    }

    #[test]
    fn inline_adopts_a_newer_cleaner_prefix() {
        let mut c = core(InjectMode::Inline, true);
        c.on_committed("send it to john.", HOME);
        // The cleaner's tick ran on NEWER committed text than our last
        // commit: its prefix extends ours, so it is adopted as the raw
        // mirror rather than skipped.
        c.on_chunk(
            0,
            "Send it to John. And more.",
            "Send it to john. And more coming.",
            HOME,
        );
        assert_eq!(c.sink.typed, underline("Send it to John. And more."));
        assert_eq!(c.phase, Phase::Active);
    }

    #[test]
    fn inline_finalize_wipes_the_underline_and_types_clean() {
        let mut c = core(InjectMode::Inline, true);
        c.on_committed("hello there", HOME);
        let streamed = grapheme_count(&c.sink.typed);
        let out = c.finalize("Hello there.", HOME);
        assert_eq!(out, FinalizeOutcome::Injected);
        // Underlined ledger vs clean final text: LCP 0, full wipe, retype.
        assert!(c.sink.ops.contains(&Op::Backspace(streamed)));
        assert_eq!(c.sink.typed, "Hello there. ");
        assert!(
            !c.sink.typed.contains('\u{332}'),
            "no combining mark survives the finalize"
        );
    }

    #[test]
    fn inline_multi_line_final_pastes_after_the_wipe() {
        let mut c = core(InjectMode::Inline, true);
        c.on_committed("team sign off", HOME);
        let streamed = grapheme_count(&c.sink.typed);
        match c.finalize("Best,\nPo", HOME) {
            FinalizeOutcome::NeedsPaste { remainder, deleted } => {
                assert_eq!(remainder, "Best,\nPo ");
                assert_eq!(deleted, streamed, "the whole underlined ledger");
            }
            other => panic!("expected NeedsPaste, got {other:?}"),
        }
    }

    #[test]
    fn inline_focus_change_freezes_and_finalize_skips() {
        let mut c = core(InjectMode::Inline, true);
        c.on_committed("hello", HOME);
        let before = c.sink.typed.clone();
        c.on_committed("hello there", ELSEWHERE);
        assert_eq!(c.phase, Phase::Frozen(FreezeReason::FocusChanged));
        assert_eq!(c.sink.typed, before, "nothing typed into the other app");
        let out = c.finalize("Hello there.", HOME);
        assert_eq!(out, FinalizeOutcome::SkippedFocusChanged);
        assert_eq!(c.sink.typed, before, "no deletes either");
    }

    #[test]
    fn inline_sink_failure_freezes() {
        let mut c = core(InjectMode::Inline, true);
        c.on_committed("hello", HOME);
        c.sink.fail_type = true;
        c.on_committed("hello there my friend", HOME);
        assert_eq!(c.phase, Phase::Frozen(FreezeReason::SinkError));
        // Heal the sink: finalize still repairs (focus unchanged).
        c.sink.fail_type = false;
        let out = c.finalize("Hello there my friend.", HOME);
        assert_eq!(out, FinalizeOutcome::Injected);
        assert_eq!(c.sink.typed, "Hello there my friend. ");
    }

    #[test]
    fn inline_cancel_wipes_every_underlined_grapheme() {
        let mut c = core(InjectMode::Inline, true);
        c.on_committed("hello there my friend", HOME);
        let streamed = grapheme_count(&c.sink.typed);
        c.cancel(HOME);
        assert_eq!(c.sink.ops.last(), Some(&Op::Backspace(streamed)));
        assert_eq!(c.sink.typed, "");
        assert_eq!(c.phase, Phase::Done);
    }

    // ---- state machine odds and ends ----

    #[test]
    fn finalize_after_done_reports_failure_without_ops() {
        let mut c = core(InjectMode::Raw, true);
        c.on_committed("hi", HOME);
        c.cancel(HOME);
        let ops = c.sink.ops.len();
        assert!(matches!(c.finalize("hi", HOME), FinalizeOutcome::Failed(_)));
        assert_eq!(c.sink.ops.len(), ops);
    }

    #[test]
    fn clean_model_ignores_raw_commits() {
        let mut c = core(InjectMode::CleanModel, true);
        c.on_committed("raw words flowing", HOME);
        assert!(c.sink.ops.is_empty());
        assert_eq!(c.phase, Phase::Active);
    }
}
