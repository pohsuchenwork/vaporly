# AGENTS.md

Guidance for AI coding assistants working in this repository.

## What this is

Vaporly: a fully local dictation app for macOS (Tauri 2.x, Rust backend +
React/TypeScript frontend). One
hotkey, one fixed STT model (Parakeet TDT 0.6B v2 GGUF via transcribe-cpp),
one cleanup provider (bundled llama-server with Qwen2.5, lazy-started), a
fresh 33-key settings schema (schema v1), and a small set of stage dials
(custom words, custom phrases, filler fix up, mind-change check, context
awareness) each with Deterministic or Model engines.

## Hard rules

- NO em dashes or en dashes anywhere: code, comments, docs, prompts, commit
  messages. `bun run check:dashes` enforces.
- All user-facing strings go through i18next (`src/i18n/locales/en/translation.json`);
  ESLint's no-literal-string rule enforces. v2 is English-only but the
  plumbing stays.
- Conventional commit prefixes (feat:, fix:, refactor:, docs:, chore:).
- Never re-add a custom-phrases block to any LLM prompt (v1 lesson: models
  hallucinated templates and blanked pastes). Phrases are deterministic-only.
- One LLM request in flight at a time (concurrent 7B calls thrash the CPU).

## Commands

```bash
bun install
CMAKE_POLICY_VERSION_MINIMUM=3.5 bun run tauri dev    # run (also regenerates src/bindings.ts)
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test           # in src-tauri/
bun run build                                         # tsc + vite (typecheck gate)
bun run lint && bun run check:dashes && cargo fmt
```

bindings.ts regenerates only when the debug binary RUNS with cwd=src-tauri
(a headless run like `./target/debug/vaporly --list-devices` suffices).
Keep `@tauri-apps/api` on the same major.minor as the tauri crate (2.10.x);
a blind `bun update` breaks `tauri build`.

## Architecture map (src-tauri/src/)

- `lib.rs`: setup, manager init, command registration (collect_commands!)
- `settings.rs`: AppSettings (33 keys, schema v1, fresh store; quarantine on corrupt)
- `defaults.rs`: consts for behaviors v1 exposed as settings (paste method, VAD, etc.)
- `actions.rs`: TranscribeAction pipeline orchestration + LiveCleaner (sentence-incremental cleanup)
- `managers/`: model (fixed Parakeet, HF download), transcription (batch + pseudo-stream with LocalAgreement-2 live preview), audio (cpal + VAD), history (SQLite), llm_engine (bundled llama-server: ephemeral port, bearer token, lazy start), hardware (RAM ladder, VM detection)
- `transcription_coordinator.rs`: pure state machine for the one hotkey (Hold/TapPending/Latched; hold to talk, double-tap latches, 20 min cap)
- `audio_toolkit/`: pure text stages (custom words fuzzy, custom phrases, filler filter, ITN) + VAD + recorder
- `prompts.rs`: internal cleanup prompt (v2 has no user prompt editor)
- `context.rs`: frontmost-app capture + category classification (macOS)
- `overlay.rs`: recording overlay window (six styles: none, bar, bar_live, textbox_raw, textbox_clean, inline)

Frontend (src/): `App.tsx` (onboarding + toasts), `components/settings/*`
(sections listed in `components/Sidebar.tsx`), `stores/settingsStore.ts`
(settingUpdaters maps setting key to Tauri command), `bindings.ts`
(generated, never hand-edit).

## Testing

`cargo test` in src-tauri must stay green (coordinator machine, text/itn
stages, settings defaults, history, engine chain live test that soft-skips
without the payload). Headless E2E:
`./target/debug/vaporly --transcribe-file tests/fixtures/stt_fixture.wav --json`.
