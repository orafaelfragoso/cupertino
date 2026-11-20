//! Keeping the list in its file (D21, [`crate::store`]): read back as the component starts, written whole on the I/O
//! pool each time it changed, one write at a time.

use std::path::PathBuf;
use std::time::Duration;

use cupertino_kit::core::Command as Do;
use cupertino_kit::task;

use super::Message;
use crate::center::NotificationCenter;
use crate::store;

/// How long reading or writing the file should take: tens of kilobytes on the runtime directory's memory file system.
const FILE_DEADLINE: Duration = Duration::from_secs(2);

/// The file and what was last written to it.
pub(super) struct Store {
    /// `None`: there is no runtime directory, and the list is not kept.
    path: Option<PathBuf>,
    /// The model's revision when the list was last encoded.
    revision: Option<u64>,
    /// What the file holds, or will once the write under way ends.
    written: Vec<u8>,
    /// A write is under way.
    writing: bool,
    /// The last write failed, and said so.
    failed: bool,
}

impl Store {
    pub(super) fn new() -> Self {
        Store {
            path: store::path(),
            revision: None,
            written: Vec::new(),
            writing: false,
            failed: false,
        }
    }

    /// Read what an earlier run kept, and the menu bar's height from the theme, on the I/O pool; the list waits for
    /// both. `offset` is the model's clock on the wall clock.
    pub(super) fn restore(&self, offset: f64) -> Do<Message> {
        let path = self.path.clone();
        task::io(
            FILE_DEADLINE,
            move || {
                let height = cupertino_kit_shell::design::menu_bar().height;
                let Some(path) = path else {
                    return (None, height);
                };
                let restored = match store::read(&path) {
                    Ok(Some(bytes)) => store::decode(&bytes, offset)
                        .map_err(|e| {
                            eprintln!(
                                "cupertino-notifications: {} is not a kept list: {e}",
                                path.display()
                            );
                        })
                        .ok(),
                    Ok(None) => None,
                    Err(e) => {
                        eprintln!(
                            "cupertino-notifications: cannot read {}: {e}",
                            path.display()
                        );
                        None
                    }
                };
                (restored, height)
            },
            |(restored, height)| Message::Restored(restored, height),
        )
    }

    /// The write of the list, when it changed since it was last written and no write is under way. Nothing is written
    /// before the earlier run's list is back (`restored`), so it is not lost; `events`: the model announced
    /// something, which may have changed what is kept of a notification without a new revision.
    pub(super) fn keep(
        &mut self,
        center: &NotificationCenter,
        offset: f64,
        restored: bool,
        events: bool,
    ) -> Option<Do<Message>> {
        let path = self.path.clone()?;
        if !restored || self.writing {
            return None;
        }
        if self.revision == Some(center.revision()) && !events {
            return None;
        }
        self.revision = Some(center.revision());
        let bytes = store::encode(center, offset);
        if bytes == self.written {
            return None;
        }
        self.written = bytes.clone();
        self.writing = true;
        Some(task::io(
            FILE_DEADLINE,
            move || store::write(&path, &bytes).map_err(|e| e.to_string()),
            Message::Saved,
        ))
    }

    /// The write ended; a change since is written by the next `keep`.
    pub(super) fn saved(&mut self, result: Result<(), String>) {
        self.writing = false;
        match result {
            Ok(()) => self.failed = false,
            Err(e) => {
                // written again with the next change; a failure is told once until a write succeeds
                if !self.failed {
                    eprintln!("cupertino-notifications: the list is not kept: {e}");
                }
                self.failed = true;
                self.written.clear();
                self.revision = None;
            }
        }
    }
}
