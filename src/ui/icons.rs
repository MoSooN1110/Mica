//! Decorative glyph lookup for the render layer (SPEC/01_ui.md §6.5).
//!
//! Unicode symbols used for decoration (tree expand/collapse, file/symlink
//! markers, arrows, bullets, ...) must be conservative and always have an
//! ASCII fallback. This module centralizes that mapping so `rendering.rs`
//! never hard-codes a glyph directly; it looks the glyph up through
//! [`icons`] keyed on `UiSettings::icon_mode`.
//!
//! `IconMode::NerdFont` currently falls back to the same glyphs as
//! `IconMode::Unicode`. Nerd Font codepoints are not wired up yet; doing so
//! well (choosing legible glyphs per element, verifying width-1 rendering)
//! is more than a trivial lookup swap, so it is left as a follow-up rather
//! than invented here.

use crate::config::IconMode;

/// One glyph per decorative role used by `rendering.rs`. All fields are
/// single logical glyphs (though the ASCII fallback may be a short multi-
/// character sequence, e.g. `"..."` for an ellipsis).
#[derive(Debug, Clone, Copy)]
pub struct IconSet {
    /// Expanded directory marker in the Explorer tree and collapsible
    /// search-result groups.
    pub dir_expanded: &'static str,
    /// Collapsed directory marker in the Explorer tree and collapsible
    /// search-result groups.
    pub dir_collapsed: &'static str,
    /// Leaf file marker in the Explorer tree.
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
    delta: "+-",
};

/// Returns the glyph set for `mode`. `NerdFont` currently reuses the
/// `Unicode` set (see module docs).
#[must_use]
pub fn icons(mode: IconMode) -> &'static IconSet {
    match mode {
        IconMode::Ascii => &ASCII,
        IconMode::Unicode | IconMode::NerdFont => &UNICODE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_mode_has_no_non_ascii_glyphs() {
        let set = icons(IconMode::Ascii);
        for glyph in [
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
        ] {
            assert!(glyph.is_ascii(), "expected ASCII glyph, got {glyph:?}");
        }
    }

    #[test]
    fn nerd_font_mode_falls_back_to_unicode() {
        let nerd = icons(IconMode::NerdFont);
        let unicode = icons(IconMode::Unicode);
        assert_eq!(nerd.dir_expanded, unicode.dir_expanded);
        assert_eq!(nerd.file, unicode.file);
    }
}
