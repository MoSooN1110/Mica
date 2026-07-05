/// A reversible text edit, expressed in Unicode scalar-value offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub start_char: usize,
    pub deleted: String,
    pub inserted: String,
}

#[derive(Debug, Default)]
pub(crate) struct History {
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

impl History {
    pub fn record(&mut self, edit: Edit) {
        self.undo.push(edit);
        self.redo.clear();
    }

    pub fn pop_undo(&mut self) -> Option<Edit> {
        self.undo.pop()
    }

    pub fn push_redo(&mut self, edit: Edit) {
        self.redo.push(edit);
    }

    pub fn pop_redo(&mut self) -> Option<Edit> {
        self.redo.pop()
    }

    pub fn push_undo(&mut self, edit: Edit) {
        self.undo.push(edit);
    }
}
