//! Back and forward history of a browser window: going somewhere new drops the forward list, going
//! back or forward moves one location between the lists (browser behaviour of the
//! reference toolbar's back and forward buttons). A caller that may fail to open a location looks
//! at the target first ([`History::back_target`]) and takes the step only once it has opened it.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct History {
    current: PathBuf,
    /// Most recent last.
    back: Vec<PathBuf>,
    /// The location `forward` returns to is the last element.
    forward: Vec<PathBuf>,
}

impl History {
    pub fn new(start: PathBuf) -> Self {
        History {
            current: start,
            back: Vec::new(),
            forward: Vec::new(),
        }
    }

    pub fn current(&self) -> &Path {
        &self.current
    }

    /// Where [`History::back`] goes; `None` at the start of the history.
    pub fn back_target(&self) -> Option<&Path> {
        self.back.last().map(PathBuf::as_path)
    }

    /// Where [`History::forward`] goes; `None` when nothing was stepped back from.
    pub fn forward_target(&self) -> Option<&Path> {
        self.forward.last().map(PathBuf::as_path)
    }

    /// Go to `to`; going to the current location changes nothing.
    pub fn visit(&mut self, to: PathBuf) {
        if to == self.current {
            return;
        }
        self.back.push(std::mem::replace(&mut self.current, to));
        self.forward.clear();
    }

    /// Step back; the new current location, `None` at the start of the history.
    pub fn back(&mut self) -> Option<&Path> {
        let to = self.back.pop()?;
        self.forward.push(std::mem::replace(&mut self.current, to));
        Some(&self.current)
    }

    /// Step forward again; `None` when nothing was stepped back from.
    pub fn forward(&mut self) -> Option<&Path> {
        let to = self.forward.pop()?;
        self.back.push(std::mem::replace(&mut self.current, to));
        Some(&self.current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn back_and_forward_walk_the_visited_locations() {
        let mut h = History::new(p("/home"));
        assert_eq!((h.back_target(), h.forward_target()), (None, None));
        h.visit(p("/home/docs"));
        h.visit(p("/home/docs/work"));
        assert_eq!(h.back(), Some(Path::new("/home/docs")));
        assert_eq!(h.back(), Some(Path::new("/home")));
        assert_eq!(h.back(), None);
        assert_eq!(h.forward(), Some(Path::new("/home/docs")));
        assert_eq!(h.forward(), Some(Path::new("/home/docs/work")));
        assert_eq!(h.forward(), None);
        assert_eq!(h.current(), Path::new("/home/docs/work"));
    }

    #[test]
    fn a_new_visit_drops_the_forward_list_and_the_same_location_is_no_visit() {
        let mut h = History::new(p("/a"));
        h.visit(p("/b"));
        h.back();
        assert_eq!(h.forward_target(), Some(Path::new("/b")));
        h.visit(p("/a"));
        assert_eq!(
            h.forward_target(),
            Some(Path::new("/b")),
            "visiting the current location is not a step"
        );
        h.visit(p("/c"));
        assert_eq!(h.forward_target(), None);
        assert_eq!(h.back(), Some(Path::new("/a")));
        assert_eq!(h.back(), None);
    }

    #[test]
    fn targets_name_the_next_step_without_taking_it() {
        let mut h = History::new(p("/a"));
        h.visit(p("/b"));
        assert_eq!(h.back_target(), Some(Path::new("/a")));
        assert_eq!(h.current(), Path::new("/b"));
        h.back();
        assert_eq!(
            (h.back_target(), h.forward_target()),
            (None, Some(Path::new("/b")))
        );
    }
}
