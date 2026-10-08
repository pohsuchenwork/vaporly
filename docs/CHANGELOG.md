# Changelog

All notable changes to Vaporly are documented here. Vaporly 3.0.0 is the first
official public release; earlier versions were development and pre-release builds
and are not documented here.

## 3.0.1 (2026-07-28)

An accuracy release. Dictations now come back complete, in one clean block,
with numbers written the way people write them.

### Nothing you said gets dropped

- The AI cleanup can no longer quietly delete part of what you dictated. Every
  reply is checked against your actual words, and anything that lost a
  sentence, a clause, or a meaningful phrase is thrown away in favor of the
  text you spoke. A reply that comes back empty, or that was cut off part-way,
  is discarded the same way.
- Cleanup now reads the whole dictation at once instead of one sentence at a
  time. Speech recognition puts full stops where you paused, not where
  sentences end, so judging each fragment on its own could never repair a
  thought that got split in half. Fragments split by a stray full stop are
  rejoined, and the meaning of the whole passage is considered together.
- The instructions given to the cleanup model now state plainly that nothing
  may be summarized, shortened, or left out, and self-corrections may only
  replace the one detail you corrected rather than a whole sentence.
- Changing your mind works again. The new check was at first rejecting the very
  corrections it was meant to allow, so "coffee at 7, no, actually tea at 10"
  came back with both halves still in it. Text you took back is now recognized
  by the correction words around it and removed as you intended.
- Tidying up is no longer mistaken for losing something. The check asks whether
  any information disappeared rather than how many words did, so collapsing a
  stutter or a repeated phrase costs nothing, while a clause that genuinely
  vanished is still caught. Writing a value out in figures counts as
  normalizing it, not deleting it.

### Filler and stutters

- Filler removal set to Medium now also clears conversational padding such as
  "bro" or "you know what I'm saying", and collapses repeated stutters. Both
  previously needed High.
- No level ever removes "actually" or "I mean", because those are how you
  signal a change of mind, and losing them loses the correction with them.

### One clean block instead of scattered line breaks

- Chat, notes, and browser dictations stay as a single block of text.
  Automatic paragraph splitting broke continuous thoughts into a blank line
  every couple of sentences, because ordinary connectives like "also" were
  treated as changes of subject.
- Email keeps its greeting, body, and sign-off layout, and it is much harder
  to trigger by accident: ordinary sentences such as "Thanks, that helped." or
  "Hi, sorry, I missed your message." are no longer reshaped into a fake
  letter, and closing punctuation is preserved.

### Numbers read like writing

- Small numbers stay as words in ordinary prose, so "I only need one thing"
  no longer becomes "I only need 1 thing".
- Digits are still used where they belong: times, dates, money, percentages,
  measurements, versions, list numbering, and anything from 10 up.
- Numbers are normalized in both directions, so a dictation reads the same way
  regardless of whether the speech model happened to produce a word or a
  digit, and numbers within one sentence always match each other.
- Version numbers and other dotted figures convert properly: saying "three
  point zero point one" gives you 3.0.1. Only the first group used to convert,
  which left the rest spelled out mid sentence.

### Punctuation and capitals

- No more stray capital after a comma. Speech recognition capitalizes after a
  pause even in the middle of a sentence, so "it's good now, So could we" came
  out with the capital intact. Names and "I" are left alone.

### For contributors

- The build documentation now covers staging the bundled cleanup engine before
  a release build. Skipping that step produced a build that transcribed
  normally but never cleaned anything up, with only a line in the log to say
  so. Official releases are built in CI, which always staged it, so no shipped
  version was affected.

## 3.0.0 (2026-07-18)

The first official release of Vaporly, a fully local dictation app for macOS,
Windows, and Linux,
under the GNU AGPL-3.0. Everything runs on-device; the bundled AI cleanup engine
starts only when a feature is set to Model.

### Licensing

- Released under the GNU AGPL-3.0 (AGPL-3.0-only): Vaporly is open source, and
  anything built on it must stay open under the same license. A separate
  commercial license is available from the copyright holder.
- Contributions are accepted under the Contributor License Agreement (CLA.md),
  which grants the maintainer the rights needed to offer commercial licensing.

### Dictation

- One dictation key (default Fn): hold to talk, double-tap to lock hands-free
  (20 minute cap), Esc cancels. Optional dedicated Hands-free and Whisper-mode
  keys can be bound too.
- Speech recognition by Parakeet TDT 0.6B v2 (English), downloaded on first run.
- Six overlay styles: Nothing, Bar, Bar with live transcript, and three
  on-textbox modes: raw words then the cleaned result, cleaned sentences as they
  complete, and Inline (live text streams underlined into the field and polishes
  itself per completed sentence while you speak; releasing the key drops the
  underline and leaves the clean final text). Guarded injection: secure-input
  fields fall back to Bar, switching apps mid-dictation freezes instead of typing
  into the wrong window, and Esc wipes every streamed character.
- Whisper mode: dictate quietly while louder sounds are ignored, with three
  strengths and optional per-microphone calibration.

### Cleanup

- Filler removal and mind-change resolution, each Off/Light/Medium/High with a
  Deterministic or Model engine. Deterministic mind-change resolves "at eight,
  no wait, nine" to "at 9".
- Custom words (with an aggressiveness level) and custom phrases (a spoken
  trigger expands to your saved text, deterministically and verbatim).
- Context awareness with per-category toggles (email, chat, code, browser,
  notes, general): emails are laid out as greeting, body, and sign-off, long
  messages break into clean paragraphs, and code stays literal.
- The cleanup engine treats your speech strictly as text to tidy, never as an
  instruction to answer.

### More

- Editable History with audio playback and re-transcribe, plus a storage
  retention setting.
- Auto-learn custom words from your History edits, repeated words, or post-paste
  corrections.
- Appearance: System, Light, or Dark theme with six accent colors.
- Sound cues for start and stop, with a choice of themes and a volume dial.
- Keep-result-on-clipboard and trailing-space toggles.

### Private by design

- Speech-to-text and cleanup run on your machine. The bundled engine binds to
  127.0.0.1 with a per-session token. The only outbound connections are app
  update checks and one-time model downloads.
