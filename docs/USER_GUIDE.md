# Vaporly User Guide

This guide covers every screen and setting in Vaporly, in plain language. If you just want to get started, the [README](../README.md) has the five-step setup. Open **Settings** from the Vaporly icon in your menu bar (top of the screen) to follow along.

Vaporly has five sections in the left sidebar: **Dictation**, **Custom**, **History**, **Appearance**, and **About**.

## The basics: how a dictation works

1. You hold the dictation key and speak.
2. Vaporly records only while you talk (a voice detector trims silence).
3. The speech model turns your audio into text on your Mac.
4. Optional cleanup runs (filler removal, mind-change fixes, your custom words and phrases, context formatting).
5. The finished text is pasted where your cursor is.

Nothing in this loop leaves your machine.

## Dictation

### Dictation key

- **Hold to talk** (default: **Fn**): hold the key, speak, release. The clean text pastes.
- **Hands-free**: **double-tap** the dictation key to lock recording on, so you can speak without holding anything. It stops automatically after 20 minutes. Double-tap again or press **Esc** to stop sooner.
- **Cancel**: **Esc** discards the current dictation with no paste.
- **Rebind**: click the key pill to record a new shortcut. You can also bind a dedicated **Hands-free** key and a **Whisper-mode** key here; both are unbound by default and show "None" until you set them.

### Overlay

The overlay is the on-screen feedback while you dictate. Styles:

- **Nothing**: no overlay at all.
- **Bar**: a minimal recording bar.
- **Bar with live transcript** (default): the bar shows your words as they are recognized.
- **On textbox, raw then polished**: your raw words type into the field live, then get replaced by the cleaned version when you finish.
- **On textbox, cleaned sentences**: each sentence is cleaned and typed as you complete it.
- **On textbox, Inline**: your live words stream into the field underlined, and each sentence is polished in place while you keep talking. Releasing the key removes the underline and leaves the final clean text.

**Position** puts the overlay at the top or bottom of the screen. Hover the small `?` next to the style picker for a one-line summary of every style.

### Whisper mode

Whisper mode lets you dictate quietly while Vaporly ignores louder sounds around you. Turn it on, pick a strength (Light, Medium, High), and optionally bind a key to toggle it. For best results run **Mic calibration** (the short wizard below the strength dial) and run it again whenever you move to a new environment; the room is part of the measurement. Full detail: [Whisper Mode](WHISPER_MODE.md).

### Cleanup dials

Two dials shape how much Vaporly rewrites what you said. Each has a level and an engine.

- **Filler removal**: strips "um", "uh", "you know", and similar. Off / Light / Medium / High. Default: Medium.
- **Mind-change**: resolves spoken corrections, for example "meet at eight, no wait, nine" becomes "meet at 9". Off / Light / Medium / High. Default: High.

The **engine** for each dial is either:

- **Deterministic**: rule-based, instant, fully offline, no model needed.
- **Model**: the bundled local AI. A little slower, better at messy or unusual speech.

Out of the box, filler runs Deterministic and mind-change runs Model, which is why first-run setup offers a cleanup-model download. Set both to Deterministic (or the dials to Off) if you never want the AI model to run.

### Context awareness

Vaporly detects the category of the app you are dictating into and adjusts formatting to fit:

- **email, chat, code, browser, notes, general**.

For example, chat drops a trailing period and splits long dictations into short paragraphs, email arranges the final text into greeting, body, and sign-off blocks, and code skips number formatting and automatic capitalization. In a browser, well-known web apps are recognized from the window title, so Gmail behaves like email and Google Docs like notes. Toggle each category on or off. Its handling follows your mind-change engine choice.

## Custom

### Custom words

Teach Vaporly the names, brands, and jargon you use so similar-sounding words snap to your spelling. Click **Manage** to add or remove words. The **correction level** (Off / Light / Medium / High, default Medium) controls how aggressively a misheard word is matched to your list; higher levels match looser near-misses.

### Custom phrases

A custom phrase maps a spoken trigger to saved text. Say the trigger and Vaporly pastes the saved text verbatim. Example: trigger "my address", text your full mailing address. This is fully deterministic; the saved text is pasted exactly, newlines and all.

Custom phrases have their own **correction level** so a slightly misheard trigger still fires. Single-word triggers always require an exact match, to avoid accidental expansions.

### Auto-learn

Optionally grow your custom words automatically:

- **From History edits**: when you correct a transcription in History, the fix is learned.
- **From repeated words**: words you say often but that are not recognized get suggested.
- **Both** of the above.
- **Watch after paste** (experimental): notices corrections you make right after a paste.

Off by default. This never sends anything anywhere; it only updates your local word list.

## History

Every dictation is saved locally so you can find, replay, and reuse it.

- **Search** across past dictations.
- **Edit** the text of any entry in place.
- **Play** the original audio, or **re-transcribe** it (useful after you change models or settings).
- **Storage**: the number sets the maximum number of dictations kept (default 100). Older entries beyond that limit are removed. A limit of 0 keeps nothing, so to keep everything, use the recording-retention **Never** option instead of a number.

## Appearance

- **Theme**: System (follow macOS), Light, or Dark.
- **Accent**: six contrast-tuned colors, Sakura (default), Rose, Amber, Green, Blue, Violet. The choice recolors buttons, toggles, and highlights in both light and dark.

## About

The About page holds app and update info along with Vaporly's general behavior and device settings.

- The installed **version**.
- **Check for updates**: a toggle for automatic update checks, plus a button to check right now. When a newer version is available, Vaporly downloads and installs it in place, keeping your data.
- **Start at login**: launch Vaporly automatically when you sign in to your Mac. Off by default.
- **Open log directory**: opens the folder with Vaporly's local log files, handy for troubleshooting.
- **Reset all settings**: a button that returns every setting to its defaults.
- **Reset onboarding**: replay the first-run flow the next time Vaporly starts.
- Links to the project on GitHub and to these docs.

### Output options

- **Keep result on clipboard**: when on, your dictation stays on the clipboard after pasting (so Cmd+V pastes it again). When off, your previous clipboard is restored. Default: off.
- **Trailing space**: add exactly one space after each paste so words do not run together. Default: on.

## Microphone and sounds

These device and sound controls also live on the About page.

- **Microphone**: choose which input device Vaporly records from.
- **Sound feedback**: gentle start and stop cues so you know when recording begins and ends. On by default.
- **Sound theme**: the cue sound set. Pick Marimba, Pop, Chime, Bubble, or Breeze. A Custom option appears once you add your own sound files.
- **Volume**: how loudly the cues play.
- **Output device**: which device the cues play through.

## Tips

- If a dictation ever pastes into the wrong window because you switched apps mid-sentence, Vaporly freezes the stream rather than typing into the new app. Press Esc and try again.
- Secure fields (password boxes) are respected: the on-textbox overlay styles fall back to a plain bar there.
- To go fully offline with zero AI model, set both cleanup dials to Deterministic or Off; only the speech model is then used, and it is already on your machine.
