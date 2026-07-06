//! Decorative glyph lookup for the render layer (SPEC/01_ui.md §6.5).
//!
//! Unicode symbols used for decoration (tree expand/collapse, file/symlink
//! markers, arrows, bullets, ...) must be conservative and always have an
//! ASCII fallback. This module centralizes that mapping so `rendering.rs`
//! never hard-codes a glyph directly; it looks the glyph up through
//! [`icons`] keyed on `UiSettings::icon_mode`.
//!
//! File-type icons ([`file_icon`], [`folder_icon`]) are looked up
//! separately, keyed on the same [`IconMode`], and resolve to a
//! [`FileIconColor`] role rather than a concrete [`ratatui::style::Color`] —
//! `rendering.rs` maps that role onto the active [`super::Theme`] (mirroring
//! the existing `git_status_color`/`syntax_color` helpers) so this module
//! never hard-codes a color either.

use crate::config::IconMode;

/// One glyph per decorative role used by `rendering.rs`. All fields are
/// single logical glyphs (though the ASCII fallback may be a short multi-
/// character sequence, e.g. `"..."` for an ellipsis).
#[derive(Debug, Clone, Copy)]
pub struct IconSet {
    /// Expanded directory marker in the Explorer tree and collapsible
    /// search-result groups. Doubles as the "folder" glyph: it is the only
    /// icon slot a directory row has, so it carries both the disclosure
    /// state and the file-type meaning (`NerdFont` uses a real open-folder
    /// glyph instead of a chevron).
    pub dir_expanded: &'static str,
    /// Collapsed directory marker. See [`Self::dir_expanded`].
    pub dir_collapsed: &'static str,
    /// Leaf file marker in the Explorer tree (Ascii mode only — Unicode and
    /// NerdFont modes use [`file_icon`] instead).
    pub file: &'static str,
    /// Symlink marker in the Explorer tree.
    pub symlink: &'static str,
    /// Trailing "in progress" indicator (search running, git refreshing).
    pub ellipsis: &'static str,
    /// Filled-circle "has unsaved changes" / "current selection" marker
    /// (dirty tab indicator, current branch in the branch picker).
    pub dirty: &'static str,
    /// Tab close button.
    pub close: &'static str,
    /// Source Control branch-line marker.
    pub branch: &'static str,
    /// "Ahead of upstream" arrow.
    pub arrow_up: &'static str,
    /// "Behind upstream" arrow.
    pub arrow_down: &'static str,
    /// Generic list bullet (crash-recovery buffer list).
    pub bullet: &'static str,
    /// Status-bar changed-file-count prefix.
    pub delta: &'static str,
    /// Left-edge marker on the selected row of a list/tree (Explorer,
    /// palette, pickers). Purely decorative — skipped when
    /// `reduced_decoration` is set.
    pub accent_bar: &'static str,
    /// Per-depth indent guide in the Explorer tree. Purely decorative —
    /// skipped when `reduced_decoration` is set (Ascii mode is always
    /// empty, matching SPEC/01_ui.md §6.6's ascii/none guidance).
    pub indent_guide: &'static str,
    /// Prompt marker in front of a query input row (palette, file picker).
    pub prompt: &'static str,
    /// Horizontal rule fill for the Source Control section-header pattern
    /// (`─ STAGED (3) ─────`).
    pub rule: &'static str,
    /// Vertical divider between editor tabs. Unlike [`Self::indent_guide`]
    /// (purely decorative, empty in Ascii mode) this is structural — tabs
    /// need a separator in every mode — so Ascii gets `"|"` rather than
    /// nothing.
    pub separator: &'static str,
    /// Status-bar diagnostics summary error glyph.
    pub error_icon: &'static str,
    /// Status-bar diagnostics summary warning glyph.
    pub warning_icon: &'static str,
}

const UNICODE: IconSet = IconSet {
    dir_expanded: "▾",
    dir_collapsed: "▸",
    file: "·",
    symlink: "↗",
    ellipsis: "…",
    dirty: "●",
    close: "×",
    branch: "◉",
    arrow_up: "↑",
    arrow_down: "↓",
    bullet: "•",
    delta: "±",
    accent_bar: "▎",
    indent_guide: "│",
    prompt: "❯",
    rule: "─",
    separator: "│",
    error_icon: "✗",
    warning_icon: "▲",
};

const ASCII: IconSet = IconSet {
    dir_expanded: "v",
    dir_collapsed: ">",
    file: "-",
    symlink: "~",
    ellipsis: "...",
    dirty: "*",
    close: "x",
    branch: "*",
    arrow_up: "^",
    arrow_down: "v",
    bullet: "-",
    rule: "-",
    delta: "+-",
    accent_bar: ">",
    indent_guide: "",
    prompt: ">",
    separator: "|",
    error_icon: "x",
    warning_icon: "!",
};

// Nerd Font codepoints below are the Devicons (`nf-dev-*`) glyphs, which are
// the best-attested single-width PUA codepoints available without a locally
// installed Nerd Font to render-test against; see `file_icon` doc comment
// for the rest of the file-type table and its confidence notes. Everything
// that isn't a file-type glyph reuses the `Unicode` set: arrows, bullets,
// and disclosure chevrons aren't file-type icons and a wholesale nerd-font
// reskin of them is out of scope for this change.
const NERD_FONT: IconSet = IconSet {
    // nf-fa-folder_open / nf-fa-folder — stable, widely-documented Font
    // Awesome codepoints.
    dir_expanded: "\u{F07C}",
    dir_collapsed: "\u{F07B}",
    ..UNICODE
};

/// Returns the glyph set for `mode`.
#[must_use]
pub fn icons(mode: IconMode) -> &'static IconSet {
    match mode {
        IconMode::Ascii => &ASCII,
        IconMode::Unicode => &UNICODE,
        IconMode::NerdFont => &NERD_FONT,
    }
}

/// Semantic color role for a file-type icon. `rendering.rs` maps each
/// variant onto an existing [`super::Theme`] token — this module never
/// hard-codes a [`ratatui::style::Color`] (SPEC/01_ui.md §6.3).
///
/// Mapping (documented here so the choice lives next to the lookup table):
/// - `Number` (`theme.syntax_number`): Rust — matches the editor's number
///   highlight, giving Rust files a warm accent distinct from the rest.
/// - `Function` (`theme.syntax_function`): JavaScript/TypeScript/Python —
///   "scripting language" bucket.
/// - `Type` (`theme.syntax_type`): TOML/YAML/JSON/INI — structured data and
///   config, matching the editor's type-highlight color.
/// - `Keyword` (`theme.syntax_keyword`): C/C++/headers/shell scripts,
///   Makefile/Dockerfile — "compiled/systems" bucket.
/// - `Str` (`theme.syntax_string`): HTML/CSS — markup/style files.
/// - `Muted` (`theme.text_muted`): Markdown/plain text/LICENSE — prose.
/// - `Constant` (`theme.syntax_constant`): images — inert binary assets.
/// - `GitModified` (`theme.git_modified`): lock files and `.git*` dotfiles —
///   "generated/VCS-adjacent" bucket, reusing the existing modified-file
///   color rather than adding a new token.
/// - `Accent` (`theme.accent`): folders.
/// - `Faint` (`theme.text_faint`): unrecognized/binary files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileIconColor {
    Number,
    Function,
    Type,
    Keyword,
    Str,
    Muted,
    Constant,
    GitModified,
    Accent,
    Faint,
}

/// A resolved file-type icon: a glyph plus the semantic color role to
/// render it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIcon {
    pub glyph: &'static str,
    pub color: FileIconColor,
}

/// Shape bucket for `Unicode` mode (SPEC/01_ui.md §6.5: conservative,
/// width-1, differentiated mainly by color). `Ascii` mode ignores shape
/// entirely and always renders the existing dash marker, differentiating
/// purely by color, per the design brief.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileCategory {
    Rust,
    Markdown,
    Config,
    Python,
    C,
    Cpp,
    Header,
    JavaScript,
    TypeScript,
    Markup,
    Shell,
    Lock,
    GitDotfile,
    License,
    Image,
    ScriptFile,
    Text,
    Binary,
}

impl FileCategory {
    fn glyph(self, mode: IconMode) -> &'static str {
        if mode == IconMode::Ascii {
            return "-";
        }
        if mode == IconMode::NerdFont {
            return self.nerd_glyph();
        }
        match self {
            Self::Rust
            | Self::Python
            | Self::C
            | Self::Cpp
            | Self::Shell
            | Self::ScriptFile
            | Self::JavaScript
            | Self::TypeScript => "◆",
            Self::Markdown | Self::Text | Self::License => "≡",
            Self::Config | Self::Lock | Self::GitDotfile | Self::Binary => "●",
            Self::Header | Self::Markup | Self::Image => "◇",
        }
    }

    /// Devicons/FontAwesome codepoints, best-effort (see module docs).
    /// Where no well-attested codepoint was available, this falls back to
    /// the `Unicode` shape glyph rather than guessing one, to avoid
    /// shipping a fabricated codepoint.
    fn nerd_glyph(self) -> &'static str {
        match self {
            Self::Rust => "\u{E7A8}",                     // nf-dev-rust
            Self::Markdown => "\u{EB1D}",                 // nf-dev-markdown
            Self::Python => "\u{E73C}",                   // nf-dev-python
            Self::C => "\u{E71E}",                        // nf-dev-c
            Self::Cpp | Self::Header => "\u{E7A3}",       // nf-dev-cplusplus
            Self::JavaScript => "\u{E781}",               // nf-dev-javascript
            Self::TypeScript => "\u{E628}",               // nf-seti-typescript
            Self::Markup => "\u{F13B}",                   // nf-dev-html5
            Self::Shell | Self::ScriptFile => "\u{E795}", // nf-dev-terminal
            Self::Lock => "\u{F023}",                     // nf-fa-lock
            Self::GitDotfile => "\u{F1D3}",               // nf-dev-git
            Self::License => "\u{F24E}",                  // nf-fa-balance-scale
            Self::Image => "\u{F1C5}",                    // nf-fa-file-image-o
            Self::Config => "\u{F013}",                   // nf-fa-cog
            Self::Text => "\u{F15C}",                     // nf-fa-file-text
            Self::Binary => "\u{F15B}",                   // nf-fa-file-o
        }
    }

    fn color(self) -> FileIconColor {
        match self {
            Self::Rust => FileIconColor::Number,
            Self::JavaScript | Self::TypeScript | Self::Python => FileIconColor::Function,
            Self::Config => FileIconColor::Type,
            Self::C | Self::Cpp | Self::Header | Self::Shell | Self::ScriptFile => {
                FileIconColor::Keyword
            }
            Self::Markup => FileIconColor::Str,
            Self::Markdown | Self::Text | Self::License => FileIconColor::Muted,
            Self::Image => FileIconColor::Constant,
            Self::Lock | Self::GitDotfile => FileIconColor::GitModified,
            Self::Binary => FileIconColor::Faint,
        }
    }
}

const LOCK_FILE_NAMES: &[&str] = &[
    "cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "composer.lock",
    "gemfile.lock",
    "poetry.lock",
];

const GIT_DOTFILES: &[&str] = &[".gitignore", ".gitattributes", ".gitmodules", ".gitkeep"];

const SCRIPT_FILE_NAMES: &[&str] = &["makefile", "dockerfile", "justfile"];

fn classify(file_name: &str) -> FileCategory {
    let lower = file_name.to_ascii_lowercase();
    if lower == "cargo.toml" {
        return FileCategory::Rust;
    }
    if LOCK_FILE_NAMES.contains(&lower.as_str()) || lower.ends_with(".lock") {
        return FileCategory::Lock;
    }
    if GIT_DOTFILES.contains(&lower.as_str()) {
        return FileCategory::GitDotfile;
    }
    if lower == "license" || lower.starts_with("license.") {
        return FileCategory::License;
    }
    if SCRIPT_FILE_NAMES.contains(&lower.as_str()) {
        return FileCategory::ScriptFile;
    }
    let Some(extension) = lower.rsplit_once('.').map(|(_, ext)| ext) else {
        return FileCategory::Text;
    };
    match extension {
        "rs" => FileCategory::Rust,
        "md" | "markdown" => FileCategory::Markdown,
        "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "json" | "jsonc" => FileCategory::Config,
        "py" | "pyi" => FileCategory::Python,
        "c" => FileCategory::C,
        "cc" | "cpp" | "cxx" => FileCategory::Cpp,
        "h" | "hh" | "hpp" | "hxx" => FileCategory::Header,
        "js" | "mjs" | "cjs" | "jsx" => FileCategory::JavaScript,
        "ts" | "tsx" => FileCategory::TypeScript,
        "html" | "htm" | "css" | "scss" | "sass" => FileCategory::Markup,
        "sh" | "bash" | "zsh" => FileCategory::Shell,
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "bmp" | "ico" | "webp" => FileCategory::Image,
        "txt" => FileCategory::Text,
        _ => FileCategory::Binary,
    }
}

/// Looks up the file-type icon for `file_name` (a base name, not a full
/// path) in `mode`. Falls back to [`FileCategory::Binary`] (Faint, "●"/"-")
/// for unrecognized extensions and to [`FileCategory::Text`] (Muted,
/// "≡"/"-") for extensionless names, so every input resolves to *something*
/// rather than panicking or returning an `Option`.
#[must_use]
pub fn file_icon(file_name: &str, mode: IconMode) -> FileIcon {
    let category = classify(file_name);
    FileIcon {
        glyph: category.glyph(mode),
        color: category.color(),
    }
}

/// Looks up the folder icon for a directory row. `expanded` selects the
/// open/closed variant (SPEC brief §2: "Directory rows: folder icon
/// (open/closed variants)"). Always [`FileIconColor::Accent`].
#[must_use]
pub fn folder_icon(expanded: bool, mode: IconMode) -> FileIcon {
    let set = icons(mode);
    FileIcon {
        glyph: if expanded {
            set.dir_expanded
        } else {
            set.dir_collapsed
        },
        color: FileIconColor::Accent,
    }
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use super::*;

    fn all_decorative_glyphs(set: &IconSet) -> Vec<&'static str> {
        vec![
            set.dir_expanded,
            set.dir_collapsed,
            set.file,
            set.symlink,
            set.ellipsis,
            set.dirty,
            set.close,
            set.branch,
            set.arrow_up,
            set.arrow_down,
            set.bullet,
            set.delta,
            set.accent_bar,
            set.indent_guide,
            set.prompt,
            set.rule,
            set.separator,
            set.error_icon,
            set.warning_icon,
        ]
    }

    #[test]
    fn ascii_mode_has_no_non_ascii_glyphs() {
        let set = icons(IconMode::Ascii);
        for glyph in all_decorative_glyphs(set) {
            assert!(glyph.is_ascii(), "expected ASCII glyph, got {glyph:?}");
        }
    }

    #[test]
    fn ascii_file_icons_are_pure_ascii() {
        for name in [
            "main.rs",
            "readme.md",
            "Cargo.toml",
            "config.yaml",
            "data.json",
            "script.py",
            "lib.c",
            "lib.cpp",
            "lib.h",
            "app.js",
            "app.ts",
            "index.html",
            "style.css",
            "build.sh",
            "Cargo.lock",
            ".gitignore",
            "LICENSE",
            "logo.png",
            "notes.txt",
            "weird.xyz",
            "Makefile",
        ] {
            let icon = file_icon(name, IconMode::Ascii);
            assert!(icon.glyph.is_ascii(), "{name} produced {:?}", icon.glyph);
        }
    }

    #[test]
    fn unicode_decorative_glyphs_are_single_width() {
        let set = icons(IconMode::Unicode);
        for glyph in all_decorative_glyphs(set) {
            if glyph.is_empty() {
                continue;
            }
            assert_eq!(
                UnicodeWidthStr::width(glyph),
                1,
                "expected width-1 glyph, got {glyph:?}"
            );
        }
    }

    #[test]
    fn nerd_font_glyphs_are_single_width() {
        let set = icons(IconMode::NerdFont);
        for glyph in all_decorative_glyphs(set) {
            if glyph.is_empty() {
                continue;
            }
            assert_eq!(
                UnicodeWidthStr::width(glyph),
                1,
                "expected width-1 glyph, got {glyph:?}"
            );
        }
        for category in [
            FileCategory::Rust,
            FileCategory::Markdown,
            FileCategory::Config,
            FileCategory::Python,
            FileCategory::C,
            FileCategory::Cpp,
            FileCategory::Header,
            FileCategory::JavaScript,
            FileCategory::TypeScript,
            FileCategory::Markup,
            FileCategory::Shell,
            FileCategory::Lock,
            FileCategory::GitDotfile,
            FileCategory::License,
            FileCategory::Image,
            FileCategory::ScriptFile,
            FileCategory::Text,
            FileCategory::Binary,
        ] {
            let glyph = category.glyph(IconMode::NerdFont);
            assert_eq!(
                UnicodeWidthStr::width(glyph),
                1,
                "{category:?} nerd glyph {glyph:?} is not width 1"
            );
        }
    }

    #[test]
    fn nerd_font_mode_uses_distinct_folder_glyphs() {
        let nerd = icons(IconMode::NerdFont);
        let unicode = icons(IconMode::Unicode);
        assert_ne!(nerd.dir_expanded, unicode.dir_expanded);
        assert_ne!(nerd.dir_collapsed, unicode.dir_collapsed);
        // Non-file-type decoration is intentionally shared with Unicode.
        assert_eq!(nerd.arrow_up, unicode.arrow_up);
        assert_eq!(nerd.branch, unicode.branch);
    }

    #[test]
    fn file_icon_maps_known_extensions() {
        assert_eq!(
            file_icon("main.rs", IconMode::Unicode).color,
            FileIconColor::Number
        );
        assert_eq!(
            file_icon("Cargo.toml", IconMode::Unicode).color,
            FileIconColor::Number
        );
        assert_eq!(
            file_icon("readme.md", IconMode::Unicode).color,
            FileIconColor::Muted
        );
        assert_eq!(
            file_icon("config.toml", IconMode::Unicode).color,
            FileIconColor::Type
        );
        assert_eq!(
            file_icon("data.json", IconMode::Unicode).color,
            FileIconColor::Type
        );
        assert_eq!(
            file_icon("script.py", IconMode::Unicode).color,
            FileIconColor::Function
        );
        assert_eq!(
            file_icon("app.ts", IconMode::Unicode).color,
            FileIconColor::Function
        );
        assert_eq!(
            file_icon("index.html", IconMode::Unicode).color,
            FileIconColor::Str
        );
        assert_eq!(
            file_icon("Cargo.lock", IconMode::Unicode).color,
            FileIconColor::GitModified
        );
        assert_eq!(
            file_icon(".gitignore", IconMode::Unicode).color,
            FileIconColor::GitModified
        );
        assert_eq!(
            file_icon("logo.png", IconMode::Unicode).color,
            FileIconColor::Constant
        );
        assert_eq!(
            file_icon("LICENSE", IconMode::Unicode).color,
            FileIconColor::Muted
        );
    }

    #[test]
    fn file_icon_falls_back_for_unknown_extensions() {
        assert_eq!(
            file_icon("binary.xyz", IconMode::Unicode).color,
            FileIconColor::Faint
        );
        assert_eq!(
            file_icon("no_extension_here", IconMode::Unicode).color,
            FileIconColor::Muted
        );
    }

    #[test]
    fn folder_icon_is_always_accent() {
        assert_eq!(
            folder_icon(true, IconMode::Unicode).color,
            FileIconColor::Accent
        );
        assert_eq!(
            folder_icon(false, IconMode::NerdFont).color,
            FileIconColor::Accent
        );
    }
}
