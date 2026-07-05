# AGENTS.md

## 1. Purpose

This repository contains a terminal-native IDE focused on:

- fast file navigation
- VS Code-like sidebar switching between Explorer and Source Control
- integrated terminal sessions
- Git status, diff, staging, commit, branch, and history workflows
- lightweight diagnostics from language servers, compilers, and linters
- support for C/C++, Rust, Python, JSON, and Markdown

The product is intentionally smaller than a general-purpose IDE. Prefer a coherent, reliable core over broad feature coverage.

---

## 2. Product Principles

1. **Terminal-native first**
   - Everything must remain usable inside a standard terminal emulator.
   - Do not depend on browser views, Electron, or GUI-only APIs.

2. **Keyboard-first, mouse-compatible**
   - Every core operation must have a keyboard path.
   - Mouse support is optional for the first implementation but architecture must not prevent it.

3. **Small and composable**
   - Keep editor, Git, diagnostics, terminal, and language services as separate subsystems.
   - Avoid large cross-module state mutations.

4. **Fast feedback**
   - Opening files, switching sidebar views, moving the cursor, and rendering diagnostics must feel immediate.
   - Slow work must run outside the render loop.

5. **Minimal LSP**
   - Implement only the capabilities required by the specification.
   - Do not expand scope into a full VS Code-compatible LSP client unless explicitly requested.

6. **External tools over reimplementation**
   - Use installed language servers, compilers, linters, Git, and shells.
   - Prefer structured process output over parsing human-readable text when available.

7. **Lightweight, but visually deliberate**
   - Visual quality is a first-class product requirement, not optional polish.
   - Prefer restrained spacing, consistent hierarchy, readable contrast, and stable layouts over decorative effects.
   - Do not add expensive animation, transparency simulation, background textures, or render-loop effects.
   - A visually refined interface must still remain responsive over SSH and in WSL.

8. **Typography-aware terminal UI**
   - The application cannot control the terminal emulator's font directly, so it must render cleanly with high-quality monospace fonts.
   - Layout, icons, separators, underlines, and fallback glyphs must remain legible with common coding fonts.
   - Never require bundled proprietary fonts or patched fonts for basic operation.

---

## 3. Technical Baseline

Preferred stack:

- Language: Rust
- TUI: Ratatui
- Terminal backend: Crossterm
- Async runtime: Tokio
- Text storage: Ropey
- Parsing and highlighting: Tree-sitter
- PTY: portable-pty
- Terminal emulation: alacritty_terminal, vte, or an equivalent VT-compatible implementation
- File watching: notify
- Git:
  - prefer the `git` executable for behavior parity
  - use `git2` only where it clearly simplifies read-only operations
- Serialization and configuration: Serde, TOML
- LSP types: lsp-types

Do not replace these choices without documenting the reason in the pull request or change summary.

---

## 4. Repository Structure

Use this structure unless there is a strong implementation reason not to:

```text
src/
  main.rs
  app/
    mod.rs
    state.rs
    actions.rs
    commands.rs
    event_loop.rs
  ui/
    mod.rs
    layout.rs
    activity_bar.rs
    sidebar/
      mod.rs
      explorer.rs
      source_control.rs
      search.rs
    editor/
      mod.rs
      viewport.rs
      gutter.rs
      decorations.rs
      diff_view.rs
    bottom_panel/
      mod.rs
      terminal.rs
      problems.rs
      output.rs
    status_bar.rs
    command_palette.rs
  editor/
    mod.rs
    buffer.rs
    cursor.rs
    selection.rs
    undo.rs
    document.rs
  workspace/
    mod.rs
    project.rs
    filesystem.rs
    watcher.rs
    tree.rs
  git/
    mod.rs
    repository.rs
    status.rs
    diff.rs
    stage.rs
    commit.rs
    branch.rs
  terminal/
    mod.rs
    manager.rs
    session.rs
    screen.rs
    input.rs
  diagnostics/
    mod.rs
    store.rs
    model.rs
    dedupe.rs
    parsers/
      mod.rs
      cargo.rs
      clang.rs
      python.rs
      json.rs
      markdown.rs
  lsp/
    mod.rs
    client.rs
    transport.rs
    manager.rs
    capabilities.rs
    root.rs
    servers.rs
  syntax/
    mod.rs
    registry.rs
    highlight.rs
  tasks/
    mod.rs
    runner.rs
    definitions.rs
  theme/
    mod.rs
    palette.rs
    tokens.rs
    loader.rs
    contrast.rs
  config/
    mod.rs
    settings.rs
    keymap.rs
    languages.rs
tests/
```

Keep UI rendering free from direct process execution and blocking filesystem work.

---

## 5. Architecture Rules

### 5.1 Event and action flow

Use a unidirectional flow:

```text
Input/Event
  -> Action
  -> State update or async command
  -> Result event
  -> State update
  -> Render
```

The render layer must not:

- spawn processes
- read files
- execute Git commands
- contact language servers
- mutate global state

### 5.2 Shared state

Avoid a monolithic mutable application object accessed from every module.

Prefer:

- explicit subsystem state
- message passing
- narrow interfaces
- immutable render inputs where practical

### 5.3 Background work

Run these outside the main render loop:

- Git status refresh
- Git diff generation
- file tree scanning
- file watching
- language server communication
- compiler and linter tasks
- terminal PTY I/O
- syntax parsing for large documents

Results must return through typed events.

### 5.4 Failure isolation

A crashed language server, failed Git command, or terminated shell must not crash the editor.

Represent subsystem failures visibly in:

- Output panel
- status bar
- non-blocking notification area

---

## 6. Scope Control

The initial product must support:

- Explorer and Source Control sidebar views
- editor tabs
- file open, save, create, rename, and delete
- integrated terminal
- Git status, diff, stage, unstage, discard, commit, branch switch, pull, and push
- diagnostics and Problems panel
- minimal LSP for C/C++, Rust, Python, JSON, and Markdown
- Tree-sitter syntax highlighting
- command palette
- configurable keybindings

The initial product must not include unless explicitly added to the specification:

- debugger protocol support
- extension marketplace
- remote SSH transport built into the editor
- collaborative editing
- notebooks
- multi-cursor editing
- refactoring suite
- semantic token support
- full snippet engine
- GUI window management
- plugin ABI

Do not silently add large features.

---


## 7. Visual Design and Theme Rules

### 7.1 Default appearance

The default appearance must be a refined dark theme designed for long coding and reading sessions.

Use a low-glare, slightly cool dark background rather than pure black. The default visual direction should resemble a restrained modern editor, not a retro terminal.

Recommended default palette:

```toml
[theme.colors]
background = "#10131A"
surface = "#151923"
surface_raised = "#1B202C"
border = "#2A3140"
text = "#D8DEE9"
text_muted = "#8B95A7"
text_faint = "#626C7D"
accent = "#82AFFF"
selection = "#26344D"
active_line = "#171C27"
cursor = "#E6EDF7"

syntax_keyword = "#C792EA"
syntax_function = "#82AAFF"
syntax_type = "#FFCB6B"
syntax_string = "#C3E88D"
syntax_number = "#F78C6C"
syntax_comment = "#687487"
syntax_variable = "#D8DEE9"
syntax_constant = "#89DDFF"

git_added = "#9ECE6A"
git_modified = "#E0AF68"
git_deleted = "#F7768E"
git_conflict = "#FF9E64"

diagnostic_error = "#FF6B81"
diagnostic_warning = "#EBCB8B"
diagnostic_info = "#7AA2F7"
diagnostic_hint = "#73DACA"
```

Exact values may be tuned after contrast testing, but the semantic roles must remain stable.

### 7.2 Theme architecture

Never hard-code colors in widgets.

All UI rendering must use semantic theme tokens such as:

```rust
pub struct Theme {
    pub background: Color,
    pub surface: Color,
    pub surface_raised: Color,
    pub border: Color,
    pub text: Color,
    pub text_muted: Color,
    pub accent: Color,
    pub selection: Color,
    pub active_line: Color,
    pub syntax: SyntaxPalette,
    pub git: GitPalette,
    pub diagnostics: DiagnosticPalette,
}
```

Required theme behavior:

- one built-in dark theme
- user-loadable TOML themes
- safe fallback when a theme value is missing
- true-color output when supported
- graceful 256-color approximation
- no essential information conveyed by color alone

### 7.3 Typography assumptions

The application must document recommended terminal fonts but must not bundle font files.

Recommended fonts:

- Berkeley Mono
- JetBrains Mono
- Iosevka
- IBM Plex Mono
- Cascadia Code
- Monaspace Neon or Monaspace Argon
- Geist Mono

Preferred default recommendation for Windows Terminal and WSL:

```text
JetBrains Mono
```

Preferred premium recommendation:

```text
Berkeley Mono
```

The UI must also remain usable with the terminal's default monospace font.

Font-sensitive rules:

- do not rely on ligatures for meaning
- use conservative Unicode symbols with ASCII fallbacks
- test ambiguous glyphs such as `0/O`, `1/l/I`, braces, arrows, and box drawing
- keep line-height assumptions to one terminal cell
- avoid dense icon-only controls
- provide Nerd Font icons only as an optional mode

### 7.4 Visual hierarchy

Use hierarchy through restrained contrast and spacing:

- background: editor canvas
- surface: sidebar and bottom panel
- raised surface: input boxes, command palette, active menus
- border: subtle separators
- accent: active tab, focus, selected command, links
- muted text: inactive labels and secondary metadata

Avoid:

- bright borders around every panel
- excessive gradients or simulated shadows
- saturated colors across large areas
- more than one dominant accent color
- blinking elements except the terminal cursor

### 7.5 Focus and selection

Every focused region must be obvious without being visually loud.

Use at least two signals where practical:

- accent border or underline
- brighter title
- cursor or selection state
- active tab treatment

Selection must remain readable when syntax colors are applied beneath it.

### 7.6 Render performance

Visual refinement must not compromise responsiveness.

Do not implement:

- frame-by-frame animations
- continuously animated cursors beyond terminal support
- background blur
- opacity compositing
- per-cell dynamic gradients

Theme lookup should be allocation-free in the hot render path.

---

## 8. UI Requirements

### 7.1 Main layout

The default layout is:

```text
┌───────────────┬──────────────────────┬──────────────────────────────┐
│ Activity Bar  │ Sidebar              │ Editor Group                 │
│               │ Explorer or Git      │ Tabs + active editor         │
├───────────────┴──────────────────────┴──────────────────────────────┤
│ Bottom Panel: Problems | Output | Terminal                         │
├────────────────────────────────────────────────────────────────────┤
│ Status Bar                                                         │
└────────────────────────────────────────────────────────────────────┘
```

### 7.2 Sidebar behavior

The sidebar uses views rather than separate windows.

Required views:

- Explorer
- Source Control
- Search

Required shortcuts:

- `Ctrl+Shift+E`: Explorer
- `Ctrl+Shift+G`: Source Control
- `Ctrl+Shift+F`: Search
- `Ctrl+B`: toggle sidebar
- `Alt+1`: Explorer
- `Alt+2`: Source Control
- `Alt+3`: Search

### 7.3 Bottom panel behavior

Required tabs:

- Problems
- Output
- Terminal

Required shortcuts:

- `` Ctrl+` ``: toggle terminal
- `Ctrl+Shift+M`: Problems
- `Ctrl+J`: toggle bottom panel

### 7.4 Terminal constraints

Do not fake a terminal with captured stdout.

The terminal must:

- own a PTY
- support resize
- parse ANSI and VT sequences
- support interactive shells
- support full-screen TUI programs such as `lazygit`
- maintain a scrollback buffer
- support multiple named sessions eventually

---

## 9. Editor Requirements

The first implementation must include:

- insert and normal navigation behavior
- cursor movement
- selection
- line insertion and deletion
- undo and redo
- UTF-8 text
- save and save-as
- horizontal and vertical scrolling
- line numbers
- active line indication
- file modified state
- syntax highlighting
- diagnostic underline or colored underline fallback
- Git gutter markers
- editor tabs

Do not attempt advanced Vim emulation unless a dedicated mode is explicitly specified.

---

## 10. Git Requirements

Use Git terminology consistently:

- Working Tree Changes
- Staged Changes
- Untracked
- Conflicted

Required operations:

- repository discovery
- status refresh
- open file diff
- inline diff
- optional side-by-side diff
- stage file
- unstage file
- stage hunk
- unstage hunk
- discard file
- discard hunk
- commit
- branch list
- branch switch
- create branch
- pull
- push
- refresh

Potentially destructive actions require confirmation:

- discard file
- discard hunk
- delete branch
- reset
- force push

Never execute destructive Git commands silently.

---

## 11. Diagnostics Rules

All diagnostic sources normalize into one model:

```rust
pub struct Diagnostic {
    pub file: PathBuf,
    pub range: TextRange,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source: DiagnosticSource,
    pub code: Option<String>,
}
```

Required sources:

- LSP
- compiler
- linter
- task runner

Required severities:

- Error
- Warning
- Information
- Hint

Diagnostics must appear in:

- editor underline
- gutter marker
- file tree count or badge
- Problems panel

Deduplicate equivalent diagnostics using:

- normalized file path
- start position
- end position
- normalized message
- severity

When duplicates exist, prefer:

1. compiler
2. language server
3. linter
4. generic task parser

---

## 12. Minimal LSP Contract

Required client messages:

- `initialize`
- `initialized`
- `shutdown`
- `exit`
- `textDocument/didOpen`
- `textDocument/didChange`
- `textDocument/didSave`
- `textDocument/didClose`
- `textDocument/hover`
- `textDocument/definition`
- `textDocument/completion`

Required server handling:

- `textDocument/publishDiagnostics`
- progress and logging messages may be recorded in Output
- unknown notifications must not crash the client

Initially advertise only capabilities that are implemented.

Use full document synchronization first. Incremental synchronization may be added after correctness is established.

---

## 13. Language Support

Default server definitions:

```toml
[languages.cpp]
extensions = ["c", "cc", "cpp", "cxx", "h", "hh", "hpp", "hxx"]
command = "clangd"
args = []
root_markers = ["compile_commands.json", "CMakeLists.txt", ".git"]

[languages.rust]
extensions = ["rs"]
command = "rust-analyzer"
args = []
root_markers = ["Cargo.toml", ".git"]

[languages.python]
extensions = ["py", "pyi"]
command = "pyright-langserver"
args = ["--stdio"]
root_markers = ["pyproject.toml", "setup.py", "requirements.txt", ".git"]

[languages.json]
extensions = ["json", "jsonc"]
command = "vscode-json-language-server"
args = ["--stdio"]
root_markers = [".git"]

[languages.markdown]
extensions = ["md", "markdown"]
command = "marksman"
args = ["server"]
root_markers = [".git"]
```

The exact executable must be configurable.

Missing language servers must produce a clear non-fatal warning.

---

## 14. Configuration

Use one user-level configuration file and optional workspace overrides.

Suggested locations:

```text
~/.config/<app-name>/config.toml
<workspace>/.<app-name>.toml
```

Configuration should cover:

- theme
- tab width
- line numbers
- word wrap
- keybindings
- shell
- terminal scrollback
- language server commands
- task definitions
- Git refresh interval
- sidebar width
- bottom panel height
- theme name or theme file
- icon mode: ASCII, Unicode, or Nerd Font
- high-contrast mode
- reduced decoration mode

Workspace configuration must not execute arbitrary commands without explicit user action.

---

## 15. Testing Requirements

Every significant change must include appropriate tests.

Minimum test categories:

### Unit tests

- path normalization
- file tree sorting
- Git porcelain parsing
- diff hunk parsing
- diagnostic deduplication
- LSP framing
- root detection
- keymap resolution
- editor buffer operations

### Integration tests

- open and save a file
- stage and unstage a file in a temporary Git repository
- parse diagnostics from a mock language server
- spawn and interact with a PTY shell
- refresh file tree after filesystem changes

### Snapshot or render tests

Use sparingly for stable UI components:

- Explorer view
- Source Control view
- Problems panel
- editor gutter
- diff view
- theme token mapping
- active and inactive focus states
- 256-color fallback rendering

Do not use snapshots as a substitute for behavioral tests.

---

## 16. Performance Targets

Treat these as engineering targets, not strict benchmarks:

- application startup: under 200 ms on a typical development machine
- open project tree for 10,000 files: visible initial result under 500 ms
- keystroke-to-render latency: under 16 ms for ordinary files
- Git status refresh: non-blocking
- diagnostic rendering: proportional to visible lines, not total file size
- large file handling: degrade gracefully rather than crash

Avoid full-project rescans on every input event.

---

## 17. Coding Standards

- Use `Result` for recoverable failures.
- Avoid `unwrap()` and `expect()` outside tests and startup invariants.
- Use structured error types.
- Keep public interfaces documented.
- Prefer small modules with explicit responsibilities.
- Do not introduce global mutable singletons.
- Use tracing for diagnostics, not ad hoc prints.
- Keep asynchronous cancellation explicit.
- Ensure child processes are terminated on shutdown.
- Preserve user files by writing through safe temporary-file replacement where practical.

Run before considering a task complete:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

---

## 18. Agent Workflow

For each task:

1. Read `SPEC.md`.
2. Identify the smallest coherent implementation slice.
3. Inspect existing architecture before adding new modules.
4. State assumptions in the change summary.
5. Implement the behavior.
6. Add or update tests.
7. Run formatting, linting, and tests.
8. Report:
   - files changed
   - behavior added
   - tests run
   - known limitations
   - any specification deviation

Do not claim completion when required tests are failing.

---

## 19. Definition of Done

A feature is complete only when:

- behavior matches `SPEC.md`
- failure states are handled
- UI remains responsive
- keyboard operation exists
- tests cover core behavior
- documentation or configuration examples are updated
- no unrelated scope was added
