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
