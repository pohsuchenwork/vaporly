Read @AGENTS.md

# Vaporly, project rules

Vaporly is a fully local, on-device dictation app for macOS. AGENTS.md (imported above) has the architecture, commands, and
hard rules (no em/en dashes; i18next for all strings; engine-only cleanup).

Dev on this machine: it is a VM (paravirtual GPU is slower than CPU), so both
STT and the LLM engine bind CPU here; real Apple Silicon gets Metal. Build
with CMAKE_POLICY_VERSION_MINIMUM=3.5.
