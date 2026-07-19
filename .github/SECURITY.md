# Security

Vaporly is a local-first dictation app. The design goal is simple to state and
simple to audit: **your voice and your text never leave your machine.**
Everything below exists to keep that sentence true.

## Threat model

What Vaporly protects:

- Dictated audio and transcribed text (never sent anywhere).
- Transcription history, custom dictionary, snippets, and settings on disk.
- The local LLM engine port (so other local software cannot use or probe it).

What is out of scope:

- An attacker with root or full access to your user account. Vaporly stores
  history and settings unencrypted inside your user profile, like a notes app.
- The applications you paste text into. Once text is typed into another app,
  that app owns it.

## What leaves the machine

Nothing, except downloads and update checks that you can see and disable:

| Traffic                                   | Host                       | When                                                          |
| ----------------------------------------- | -------------------------- | ------------------------------------------------------------- |
| App update check (latest.json, signed)    | github.com                 | On launch and manual check; respects the update-checks toggle |
| Speech and cleanup model downloads (GGUF) | huggingface.co and its CDN | Only when you download a model                                |

That is the whole outbound list: GitHub for update checks and update
downloads, and Hugging Face for model downloads. The bundled cleanup engine
runs entirely on your machine and is reached only over loopback
(`127.0.0.1`); it makes no outbound connections. This build has no cloud LLM
provider path at all.

There is no telemetry, no analytics, no crash reporting, and no account
system. Transcripts, audio, prompts, and settings are never uploaded.

## Local attack surface and mitigations

### Bundled LLM engine (llama-server)

- Binds `127.0.0.1` only, never a public interface.
- Requires a bearer token on every completion request (`--api-key`). The
  token is 32 bytes of OS entropy, regenerated on every engine start, held
  only in app memory, and never written to disk. Without it, another local
  process that finds the port gets `401`. (`/health` and `/v1/models` follow
  llama.cpp's stock behavior and answer without auth; they expose liveness
  and the model file name, nothing else.)
- The web UI is disabled (`--no-webui`).
- Orphan hygiene: a pidfile reaper plus a path-scoped sweep kill any stale
  llama-server left behind by a hard kill of a previous run. The sweep only
  matches processes running from Vaporly's own engine directory, so it can
  never touch a llama-server you run yourself.
- The engine is bundled inside the app and copied into the app-data directory
  on first run (or when repairing the engine). It is not downloaded at runtime.
  The bundled payload is pinned to a specific llama.cpp release with SHA256
  checks.

### Webview (settings UI)

- Strict Content Security Policy: same-origin scripts only, no remote
  origins, no frames, no objects. Tauri adds integrity hashes for its own
  bootstrap scripts automatically.
- The asset protocol is scoped to `$APPDATA/recordings/**`, exactly the files
  the history player needs. (Before the security pass this scope was `**`,
  which would have let a compromised webview read arbitrary files. No release
  shipped with that scope.)
- Filesystem permissions for the frontend are limited to the app's own data
  directory and bundled resources (`capabilities/`).

### Prompt injection

- The frontmost app's name is interpolated into the cleanup prompt for
  context-aware formatting. App names are attacker-influenceable text, so the
  value is sanitized before it reaches the prompt: control characters
  stripped, whitespace collapsed, capped at 64 characters.
- Custom phrases, custom words, and prompt templates are user-owned local
  settings; editing them is equivalent to editing your own prompt. Phrase
  text is still sanitized at the settings boundary (control characters
  stripped, length caps), and template variables are substituted in a single
  left-to-right pass that never rescans inserted values, so a phrase or a
  transcript containing something like a template token stays literal instead
  of expanding.

### Updater

- Updates are verified with a minisign signature (public key pinned in
  `tauri.conf.json`) before install; `latest.json` is fetched over HTTPS from
  the release repository. A tampered or unsigned artifact fails installation.

### Filesystem

- Everything lives under the per-user app-data directory
  (`computer.vaporly`): settings store, history database, recordings
  (with a configurable retention period), models, engine payload, logs.
- Log files stay local, rotate at 500 KB, and debug-level log streaming into
  the UI exists only in debug builds. Transcripts are not written to the logs
  by default: at debug level only a character count is recorded, and the full
  transcript text is logged only at trace level, which is off by default.

## Supply chain

- `cargo audit` runs against the Rust dependency tree. Current status: all
  fixable advisories patched (`tar`, `rustls-webpki` updated in the security
  pass). Two advisories in `quick-xml` are accepted and documented in
  `src-tauri/.cargo/audit.toml`: the vulnerable code is reachable only
  through `plist` parsing of Vaporly's own bundled property lists, never
  attacker-supplied XML; the fix requires a Tauri release that adopts
  quick-xml 0.41.
- `bun audit` covers the frontend. Remaining advisories are all in build-time
  tooling (vite, rollup, babel, eslint dependency chains); none of that code
  ships in the application bundle.
- The bundled llama.cpp payload is version-pinned with per-artifact SHA256 sums.
- Speech models fetched through hf-hub carry etag and size verification;
  the WAV fixture is checked in.

## Repository secrets

- `TAURI_SIGNING_PRIVATE_KEY` (updater signing) lives only in GitHub Actions
  secrets and a local backup the maintainer keeps offline. Losing it means
  existing installs cannot verify future updates, so back it up; leaking it
  means an attacker who can also publish releases could sign updates, so keep
  it scoped to the release repository.
- `RELEASES_TOKEN` should be a fine-grained PAT scoped to the release
  repository only (contents: read and write), not a broad classic token.
  Rotate it if it was ever created with wider scope.
- A pre-commit secret guard blocks accidental key material in source.

## macOS permissions

- Microphone: recording your dictation. Requested on first use.
- Accessibility: global hotkeys and pasting into the frontmost app.
- Input Monitoring: push-to-talk key detection.
  Each permission is requested in onboarding with an explanation, and the app
  works in a reduced mode when one is denied (see `../docs/USER_GUIDE.md`).

## Reporting a vulnerability

Please do not open a public issue for security reports. Email
`pohsuchenwork@gmail.com` with the details (a proof of concept helps). You
will get an acknowledgment within a few days. Fixes ship as a regular
release; credit is given unless you prefer otherwise.
