//! Navigation history of the window: the design's `HistoryManager`, an array and a current index with
//! /. [`History::push`] drops the entries after the current one
//! and appends; back and forward only move the index; there is no cap. Pushing the entry that is already current is not
//! a push (approximate: the design's handling is not known). The file manager's history has the same semantics.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct History<T> {
    entries: Vec<T>,
    index: usize,
}

impl<T: PartialEq + Copy> History<T> {
    /// A history holding only `start`.
    pub fn new(start: T) -> Self {
        History {
            entries: vec![start],
            index: 0,
        }
    }

    pub fn current(&self) -> T {
        self.entries[self.index]
    }

    pub fn can_go_back(&self) -> bool {
        self.index > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.index + 1 < self.entries.len()
    }

    /// Go to `to`: the entries after the current one are dropped. False when `to` is already current.
    pub fn push(&mut self, to: T) -> bool {
        if to == self.current() {
            return false;
        }
        self.entries.truncate(self.index + 1);
        self.entries.push(to);
        self.index += 1;
        true
    }

    /// Step back; the new current entry, `None` at the start.
    pub fn back(&mut self) -> Option<T> {
        self.can_go_back().then(|| {
            self.index -= 1;
            self.current()
        })
    }

    /// Step forward; the new current entry, `None` at the end.
    pub fn forward(&mut self) -> Option<T> {
        self.can_go_forward().then(|| {
            self.index += 1;
            self.current()
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
