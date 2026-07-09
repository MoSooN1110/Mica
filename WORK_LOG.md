# Mica Work Log

This file is a concise cross-agent handoff log. Agents should append entries after tasks that change code, specs, tests, configuration, or user-visible behavior.

## 2026-07-09 — Codex

- Request: improve mouse-supported terminal IDE interactions, Git staging/commit UI, sidebar activity icons, sidebar resizing, Explorer mouse behavior, editor display toggles, Search sidebar usability, and current-focus shortcut hints.
- Implemented:
  - Source Control mouse staging/unstaging buttons and a commit entry row.
  - Activity bar icons instead of letter labels.
  - Draggable sidebar width resizing.
  - Explorer mouse selection with second-click file open and folder toggle hit-testing.
  - Optional line numbers and word wrap controls.
  - Search sidebar with clearer clickable input box, placeholder text, visible toggles, and corrected mouse hit rows.
  - Status bar shortcut hint for the active focus area, hidden on narrow terminals to avoid crowding.
  - Diagnostics/LSP code analysis disabled by default while keeping syntax highlighting independent.
- Main areas changed:
  - `src/app/state.rs`
  - `src/app/update.rs`
  - `src/command/*`
  - `src/config/*`
  - `src/ui/input.rs`
  - `src/ui/rendering.rs`
  - `src/ui/icons.rs`
  - `SPEC/01_ui.md`, `SPEC/02_editor.md`, `SPEC/04_git.md`, `SPEC/06_diagnostics_lsp.md`, `SPEC/07_config_cli.md`
  - `tests/snapshots.rs` and UI snapshots
- Verification:
  - `cargo fmt --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test --all`
  - `git diff --check`
- Commit: `561900b Improve terminal IDE mouse and status UI`
- Notes:
  - Search shortcut hints are intentionally suppressed below 90 columns so existing status segments remain readable.
