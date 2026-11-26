# Agent Instructions

Read [README.md](README.md) first — it is the single source of truth for project architecture, design invariants, repository layout, Cargo commands, CodeScene pre-commit hooks, `evnx` `.env` workflow, and security/fixture hygiene.

## Agent-Specific Notes

- **Non-login shell environment:** Ensure `$HOME/.cargo/bin` and `$HOME/.local/bin` are on `PATH` when running `cargo`, `cs`, or `evnx`. When committing from a non-interactive shell, load the shell environment (or ensure `CS_ACCESS_TOKEN` / `cs auth login` is available) so `.githooks/pre-commit` can run `cs delta --git-hook --staged`.
- **CodeHealth discipline:** Keep functions small (max 4 arguments, shallow conditionals, concise methods) so CodeScene CodeHealth pre-commit checks pass without rework.
- **Target machines vs. dev host:** Development may run on a headless/separate host without desktop shells installed. Supported shell environments are DankMaterialShell (`>= 2.0` / Quickshell `0.3+`), Noctalia Shell v5 (`plugin_api = 48`), and Waybar (`0.15+`).
