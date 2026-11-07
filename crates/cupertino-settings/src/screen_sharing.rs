//! Screen Sharing in the Privacy pane: the displays apps may share again without asking (`screen-sharing.conf`,
//! written by the portal when the person ticks to remember a share), each with Forget.

use std::io;
use std::path::Path;

use cupertino_defaults::screen_sharing::ScreenSharingConf;

use crate::clipboard::Installed;

/// What the file holds, as the pane shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grants {
    pub conf: ScreenSharingConf,
    /// Why the file was refused or could not be read: nothing may then be written over it.
    pub unreadable: Option<String>,
}

impl Grants {
    /// The file at `path`. A missing file holds no grant; a refused or unreadable one is [`Grants::unreadable`]. A
    /// line that grants nothing is dropped from the list, and from the file at the next write.
    pub fn load(path: &Path) -> Self {
        match ScreenSharingConf::load(path) {
            Ok((conf, _)) => Grants {
                conf,
                unreadable: None,
            },
            Err(e) => Grants {
                unreadable: Some(e.to_string()),
                ..Grants::default()
            },
        }
    }

    /// Write the grants into `path`.
    pub fn store(&self, path: &Path) -> io::Result<()> {
        if self.unreadable.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the list can't be read",
            ));
        }
        self.conf.store(path)
    }
}

/// The name and icon of the app `app_id` names, from its desktop entry among `installed`; the id itself without one.
pub fn identity(app_id: &str, installed: &[Installed]) -> (String, Option<String>) {
    let file = format!("{app_id}.desktop");
    installed
        .iter()
        .find(|i| i.entry.id == file)
        .map(|i| (i.entry.name.clone(), i.entry.icon.clone()))
        .unwrap_or_else(|| (app_id.to_owned(), None))
}
