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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_push_drops_the_forward_part_and_back_and_forward_only_move() {
        let mut h = History::new(1);
        assert!(!h.can_go_back() && !h.can_go_forward());
        assert!(h.push(2) && h.push(3));
        assert_eq!(
            (h.current(), h.can_go_back(), h.can_go_forward()),
            (3, true, false)
        );
        assert_eq!(h.back(), Some(2));
        assert_eq!(h.back(), Some(1));
        assert_eq!(h.back(), None);
        assert_eq!(
            (h.len(), h.can_go_forward()),
            (3, true),
            "stepping back pushes nothing"
        );
        assert_eq!(h.forward(), Some(2));
        assert!(h.push(4));
        assert_eq!(
            (h.len(), h.current(), h.can_go_forward()),
            (3, 4, false),
            "3 was dropped"
        );
        assert!(!h.push(4), "pushing the current entry is not a push");
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn the_history_has_no_cap() {
        let mut h = History::new(0u32);
        for i in 1..=10_000 {
            assert!(h.push(i));
        }
        assert_eq!(h.len(), 10_001);
        let mut steps = 0;
        while h.back().is_some() {
            steps += 1;
        }
        assert_eq!(
            (steps, h.current()),
            (10_000, 0),
            "every entry is reachable"
        );
    }
}
