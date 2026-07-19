# Changelog

All notable changes to Vaporly are documented here. Vaporly 3.0.0 is the first
official public release; earlier versions were development and pre-release builds
and are not documented here.

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
