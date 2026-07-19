# Architecture

A tour of how Vaporly is built, front to back. For working conventions and build commands, see [AGENTS.md](../AGENTS.md) and [BUILD.md](BUILD.md).

## Shape

Vaporly is a [Tauri 2](https://tauri.app/) desktop app: a Rust backend does the real work (audio, recognition, cleanup, pasting, system integration), and a React + TypeScript frontend renders the settings window and the recording overlay. The two talk over Tauri's command and event bridge, with types generated from Rust so the frontend stays in sync.

```
microphone -> voice activity detection -> speech model -> cleanup pipeline -> paste
                                                              |
                                          (optional) bundled local AI engine
```

## Backend (src-tauri/src)

- **lib.rs**: startup, manager initialization, window creation, the Tauri command registry. A fatal init error shows a native dialog instead of a silent crash.
- **managers/**: the core services, created once at startup and held in Tauri state.
  - **audio**: microphone capture and device selection.
  - **model**: the one speech model (download, load, lifecycle).
  - **transcription**: the recognition pipeline.
  - **history**: local storage of past dictations.
  - **llm_engine** / llm_client: the bundled cleanup engine and how requests reach it.
- **audio_toolkit/**: low-level audio. Device enumeration, recording, resampling, the Silero voice activity detector, the whisper-mode loudness gate and auto-gain, and text utilities (custom phrase matching, inverse text normalization).
- **pipeline/**: the deterministic and model cleanup stages (filler removal, mind-change resolution, context formatting, custom words and phrases) composed into one pass.
- **commands/**: Tauri command handlers the frontend calls.
- **settings.rs**: the settings schema (version 1) and the store-backed persistence. Unreadable stores are quarantined and replaced with defaults rather than silently wiped.
- **defaults.rs**: fixed internal values for behavior no longer exposed as settings, plus the per-strength whisper tuning.
- **shortcut.rs** and the transcription coordinator: the global hotkey backend and the hold / double-tap / cancel state machine.
- **overlay.rs**: the recording overlay window.
- **tray.rs**, **cli.rs**, **signal_handle.rs**: the menu-bar tray, command-line flags, and the shared "start a dictation" entry point used by both signals and the CLI.

## Speech recognition

One fixed model: **Parakeet TDT 0.6B v2** (English) in GGUF form, run through transcribe.cpp (whisper.cpp family, GGML). It is downloaded on first run. Token timestamps from this model drive the live on-textbox transcript. There is no model picker; the catalog contains exactly one model by design.

The live preview is a tail-window pseudo-stream that commits stable text as you speak, so on-textbox styles can type words before you finish the sentence.

## Cleanup engine

When a dial is set to **Model**, Vaporly starts a bundled `llama-server` (from llama.cpp) lazily. It binds only to `127.0.0.1` on an ephemeral port, is protected by a per-session bearer token, and runs a Qwen2.5 model chosen for your hardware (a 7B variant on machines with 14 GB or more, smaller on less). One cleanup request is in flight at a time. When every dial is Deterministic or Off, the engine never starts and everything is rule-based.

Custom phrases never travel through the model: they are applied deterministically and, for in-sentence expansions, are protected by inert sentinels across the model pass so the AI cannot rewrite them.

## Frontend (src)

- **App.tsx**: the settings shell and section routing.
- **components/**: settings screens (Dictation, Custom, History, Appearance, About), the model onboarding flow, the recording overlay UI, the update checker, and shared UI primitives.
- **stores/settingsStore.ts** and **hooks/useSettings.ts**: settings state, synced to the Rust store through commands.
- **bindings.ts**: auto-generated Tauri type bindings (via tauri-specta). It regenerates when the debug binary runs; do not edit it by hand.
- **overlay/**: the separate overlay window entry point.
- **i18n/**: strings live in `src/i18n/locales/en/translation.json`. The UI is English-only, but all user-facing text goes through i18next.
- **styles/**: a three-layer design token system (primitives, semantics, component), a house motion layer, and bundled offline fonts (Manrope for the UI, Sora for the wordmark).

## Settings and state

Settings persist through Tauri's store plugin as a single schema-version-1 struct, under the app's own bundle id (`computer.vaporly`) and data directory. There are no settings migrations; a new install starts from clean defaults.

## Updates and releases

- **Updater**: Vaporly uses the Tauri updater. It checks a `latest.json` feed on the project's GitHub Releases and installs signed updates in place. Update artifacts are signed with a minisign key regardless of code-signing.
- **CI**: GitHub Actions build the app per target and, on a manual release run, assemble `latest.json` and publish the installers and feed to the repository's own Releases using the built-in workflow token. A `platforms` input selects what to build: `macos` alone, the non-mac `others` matrix, or `all` for the full macOS, Windows, and Linux set.

## Privacy posture

Audio, transcripts, and the cleanup engine all stay on your machine. The only network activity is the one-time model downloads and the update check, both to well-known hosts. See [SECURITY.md](../.github/SECURITY.md) for the full outbound-traffic picture and threat model.
