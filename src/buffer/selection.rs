use serde::{Deserialize, Serialize};

/// A Unicode scalar-value offset into a buffer. This is not a byte offset or
/// terminal display column.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CharOffset(pub usize);

/// One selection. `anchor == head` represents a caret.
///
/// Buffers store a list of these even though Mica 1.0 exposes one selection,
/// leaving the model ready for a future multi-selection feature.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub anchor: CharOffset,
    pub head: CharOffset,
}

impl Selection {
    pub const fn caret(offset: CharOffset) -> Self {
        Self {
            anchor: offset,
            head: offset,
        }
    }

    pub fn range(self) -> std::ops::Range<usize> {
        let start = self.anchor.0.min(self.head.0);
        let end = self.anchor.0.max(self.head.0);
        start..end
    }

    pub fn is_caret(self) -> bool {
        self.anchor == self.head
    }
}
