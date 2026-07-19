# Contributing to Vaporly

Thanks for your interest. Vaporly is a small, focused, fully local dictation app for macOS, Windows, and Linux. This file covers the practical basics; deeper conventions live in [AGENTS.md](../AGENTS.md).

## Getting set up

See [BUILD.md](../docs/BUILD.md) for platform prerequisites. In short:

```bash
bun install
mkdir -p src-tauri/resources/models
curl -o src-tauri/resources/models/silero_vad_v4.onnx https://blob.handy.computer/silero_vad_v4.onnx
CMAKE_POLICY_VERSION_MINIMUM=3.5 bun run tauri dev
```

## Before you open a pull request

Run the gates and make sure they pass:

```bash
bun run lint            # frontend lint
bun run build           # typecheck + build the frontend
bun run check:dashes    # dash policy (see below)
cd src-tauri && cargo fmt && cargo test    # Rust format + tests
```

## House rules

- **No em dashes or en dashes** anywhere in the repository. Use plain hyphens, commas, colons, or periods. `bun run check:dashes` enforces this and CI will fail otherwise.
- **All user-facing strings go through i18next.** Add keys to `src/i18n/locales/en/translation.json` and use `t("key.path")`; do not hardcode text in JSX.
- **Do not hand-edit `src/bindings.ts`.** It is generated from the Rust types when the debug binary runs.
- **Rust**: run `cargo fmt` and `cargo clippy`; handle errors explicitly and avoid `unwrap` in production paths.
- **TypeScript**: strict types, functional components, Tailwind for styling, `@/` path alias for `src/`.

## Commit messages

Use conventional commit prefixes and focus the message on why, not what:

```
feat: ...      a user-facing feature
fix: ...       a bug fix
docs: ...      documentation only
refactor: ...  code change with no behavior change
chore: ...     tooling, deps, housekeeping
```

## Scope

Vaporly is deliberately minimal: one hotkey, one speech model, one bundled cleanup engine, a small set of dials. Please open an issue to discuss before adding a new surface, so we can keep the app focused.

## License and the Contributor License Agreement

Vaporly is licensed under the [GNU AGPL-3.0](../LICENSE).

Every contribution requires agreeing to the [Vaporly Contributor License
Agreement](../CLA.md). By opening a pull request you accept the CLA for that
and all future contributions: you keep the copyright to your work, and you
grant the maintainer the rights described there, including the right to
relicense contributions under other terms (this is what allows Vaporly to be
offered under separate commercial licenses). A plain Developer Certificate of
Origin sign-off is not a substitute for the CLA.
