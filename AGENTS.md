# AGENTS.md

Working agreement for coding agents (and humans) contributing to **Mica**, a Rust terminal-native IDE. This file covers *how to work*. Product requirements live in `SPEC/` — do not duplicate them here, and do not contradict them silently.

---

## 1. Documentation Map

Read the spec files relevant to your task before writing code.

| Topic | Document |
|---|---|
| Requirements source of truth | `SPEC/USER_WANTED.md` |
| Product definition, scope, milestones, acceptance | `SPEC/SPEC.md` |
| Layout, focus, input, theme & visual design | `SPEC/01_ui.md` |
| Editor, buffers, Unicode, syntax highlighting | `SPEC/02_editor.md` |
| Workspace, file tree, search, external-change safety | `SPEC/03_workspace.md` |
| Git status, diff, staging, commit, branch, push/pull | `SPEC/04_git.md` |
| Integrated terminal (real PTY) | `SPEC/05_terminal.md` |
| Diagnostics, Problems panel, minimal LSP | `SPEC/06_diagnostics_lsp.md` |
| Configuration, keymap, CLI | `SPEC/07_config_cli.md` |
| Tech stack, modules, event flow, command layer | `SPEC/08_architecture.md` |
| Performance, safety, reliability, sessions | `SPEC/09_quality.md` |
| Test requirements | `SPEC/10_testing.md` |

Precedence when documents disagree: `USER_WANTED.md` > `SPEC/*` > this file. If the spec conflicts with what you need to implement, **stop and propose a spec change** in your summary instead of quietly diverging.

---

## 2. Product Principles

1. **Terminal-native first.** Everything runs inside a standard terminal emulator. Target platforms: WSL2 (Ubuntu) and native Ubuntu, over a plain terminal, over SSH, or as a single window managed by **Herdr** (an orchestrator for coding agents and terminal windows — see `SPEC/SPEC.md` §4.3). Terminal multiplexers (tmux, screen) are explicitly out of scope — the user doesn't use them; don't spend design or testing effort there. No browser views, no GUI-only APIs.
2. **Herdr owns agents; Mica owns the workspace.** Mica never manages agents, chats with them, or shows their logs — that is Herdr's job. Herdr-managed agents appear to Mica only as external processes changing files, and integration happens through the CLI and the command layer, never through screen coordinates or synthetic key input.
3. **Keyboard-first, mouse-supported.** Every core operation must have a keyboard path *and* work naturally with the mouse. Both go through the same command layer.
4. **Never lose user text.** Unsaved edits survive external file changes, crashes, and failed saves. When in doubt, block the destructive path and ask. This outranks every convenience feature.
5. **Fast feedback.** Opening files, switching views, typing, and rendering diagnostics must feel immediate. Anything slow runs off the render loop and reports back via events.
6. **External tools over reimplementation.** Use installed language servers, compilers, linters, `git`, and shells. Prefer structured (JSON) process output over parsing human-readable text.
7. **Minimal LSP.** Diagnostics, hover, definition, completion — nothing more unless the spec says so.
8. **Visually deliberate, computationally cheap.** Visual quality is a first-class requirement, achieved through hierarchy and restraint — never through animation, compositing, or per-frame effects. Must stay responsive over SSH.
9. **Small and composable.** Editor, Git, terminal, diagnostics, and LSP are separate subsystems communicating through typed events, not a shared mutable blob.

---

## 3. Technical Baseline

Language: Rust. Stack: ratatui, crossterm, tokio, ropey, tree-sitter, portable-pty, alacritty_terminal/vte, notify, ignore, nucleo-matcher, unicode-width, unicode-segmentation, lsp-types, serde+toml, tracing. Git via the `git` CLI behind a backend abstraction (`git2` optional for read paths). Details and rationale: `SPEC/08_architecture.md`.

Do not replace these choices without documenting the reason in your change summary.

Module layout and layer boundaries are specified in `SPEC/08_architecture.md`. The non-negotiable rules:

- Unidirectional flow: `Input/Event → Command → State update / async task → Result event → State update → Render`.
- The render layer never spawns processes, touches the filesystem, runs Git, talks to language servers, or mutates state.
- Domain code (buffer, workspace, git, diagnostics, lsp, search) never depends on ratatui/crossterm or UI widgets.
- Background work (Git, search, LSP, PTY I/O, parsing, file watching, file I/O) runs off the UI thread and returns typed events. Long tasks are cancellable.
- A crashed language server, failed Git command, or dead shell must never take the editor down. Surface failures in Output / status bar / notifications.
- Every user-facing operation is a registered command with a string ID (`SPEC/08_architecture.md` §7). Keyboard, mouse, and palette all dispatch through it.

---

## 4. Scope Control

The 1.0 scope is defined by `SPEC/SPEC.md` §3 and §5 (milestones M1–M4). Non-goals (debugger, plugins, collaborative editing, multi-cursor, full Vim, advanced Git UIs, …) are listed there.

- Do not add features outside the current milestone without explicit request.
- Do not silently expand a task into neighboring features. Note the temptation in your summary instead.
- Prefer the smallest coherent slice that leaves the codebase consistent and tested.

---

## 5. Coding Standards

- Use `Result` with structured error types for recoverable failures. No `unwrap()`/`expect()` outside tests and startup invariants.
- No global mutable singletons. State changes happen in the update path only.
- Distinguish byte offsets, chars, grapheme clusters, terminal display columns, and screen coordinates — by type or by naming, never implicitly (`SPEC/02_editor.md` §3).
- Execute external commands with argument arrays, never shell string concatenation.
- Save files via temp-file + rename where practical; never leave a truncated file on failure.
- Make async cancellation explicit; reap child processes (LSP, Git, shells) on shutdown.
- Restore the terminal (raw mode, alternate screen, mouse capture) on every exit path, including panic — use RAII guards plus a panic hook.
- Use `tracing` for diagnostics. Never print to the terminal screen outside the TUI.
- Never hard-code colors in widgets; go through semantic theme tokens (`SPEC/01_ui.md` §6).
- Keep public interfaces documented. Prefer small modules with explicit responsibilities.

---

## 6. Quality Gates

Run before considering any task complete:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

Every significant change includes tests per `SPEC/10_testing.md`: unit tests for logic, integration tests for subsystem behavior (temp Git repos, mock LSP, real PTY), snapshot tests only for stable UI components. Data-safety paths (unsaved-buffer conflicts, destructive confirmations) always get integration coverage.

Performance targets are in `SPEC/09_quality.md`. Treat "no blocking work on the UI thread" as a review-blocking defect, not a nice-to-have.

---

## 7. Agent Workflow

For each task:

1. Read the relevant `SPEC/` documents (use the map in §1).
2. Inspect the existing architecture and neighboring code before adding modules or dependencies.
3. Identify the smallest coherent implementation slice.
4. State assumptions and any spec ambiguities in the change summary.
5. Implement, then add or update tests.
6. Run the quality gates (§6).
7. Report: files changed, behavior added, tests run, known limitations, and any deviation from the spec.

Do not claim completion while required tests are failing. Do not mark a spec item done if you implemented a reduced version — say exactly what is missing.

---

## 8. Definition of Done

A feature is complete only when:

- behavior matches the relevant `SPEC/` documents (or an agreed spec change is written down)
- failure states are handled and surfaced; no data-loss path was introduced
- the UI stays responsive (no new blocking work on the render/input path)
- both keyboard and mouse paths exist and dispatch through the command layer
- tests cover the core behavior, including failure cases
- configuration examples / docs are updated when user-visible behavior changed
- no unrelated scope was added

---

## 9. Known Pitfalls

Hard-won constraints; check your change against them.

- **Unicode widths:** CJK is 2 columns; emoji ZWJ sequences are single graphemes; cursor movement, mouse hit-testing, and selection rendering must agree on width math.
- **LSP positions are UTF-16** code units; convert correctly at the boundary.
- **Terminal keys are lossy:** `Ctrl+Shift+<letter>` may be swallowed by the emulator (Windows Terminal). Every binding needs a reachable fallback (palette, `Alt+N`).
- **File watchers lie:** events get dropped, coalesced, or reordered; renames may arrive as delete+create. Always keep manual refresh working; debounce storms (branch switches touch thousands of files).
- **PTY output floods:** `yes` or a build log must not freeze the UI; batch output events, don't re-render full scrollback per frame.
- **Git state is external:** another process (agent, `git` in the terminal) can change it at any moment — re-verify before destructive operations instead of trusting cached status.
- **Herdr detach/reattach must not lose state:** Herdr is a real PTY multiplexer (persistent sessions, detach/reattach, SSH `--remote` reattach). Mica runs as the program inside one Herdr pane — treat every reattach like a terminal resize/reconnect and redraw cleanly (`SPEC/SPEC.md` §4.3.4, `SPEC/09_quality.md` §3). Don't build Mica's own multi-pane/session features — Herdr already owns that layer; Mica's integrated terminal (`SPEC/05_terminal.md`) is a separate, inner PTY, not a Herdr pane.
