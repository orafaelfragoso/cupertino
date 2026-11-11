//! Desktop Control and Screen Capture in the Privacy pane: the apps that asked to operate the desktop or to take images
//! of the screen, and the person's answer to each (`desktop-control.conf`, written by the compositor when an app first
//! asks). Each app has a switch (on allows, off denies) and Forget, which removes the answer so the app asks again.
//!
//! The compositor follows the file while it runs: an app turned off or forgotten loses a session it holds at once.
//! Every change is one app's, made under the file's lock ([`DesktopControlConf::update`]), so an answer the compositor
//! records while the pane is open is never lost.

use std::path::Path;

use cupertino_defaults::desktop_control::{App, DesktopControlConf, Service};

use crate::clipboard::Installed;

/// What the file holds, as the pane shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answers {
    pub conf: DesktopControlConf,
    /// Why the file was refused or could not be read: nothing may then be written over it.
    pub unreadable: Option<String>,
}

impl Answers {
    /// The file at `path`. A missing file holds no answer; a refused or unreadable one is [`Answers::unreadable`]. A
    /// line that answers nothing is dropped from the list, and from the file at the next change, which reads the file
    /// again under its lock.
    pub fn load(path: &Path) -> Self {
        match DesktopControlConf::load(path) {
            Ok((conf, _)) => Answers {
                conf,
                unreadable: None,
            },
            Err(e) => Answers {
                unreadable: Some(e.to_string()),
                ..Answers::default()
            },
        }
    }
}

/// The title of the Privacy pane's section, and sub-page, that lists the apps of `service`.
pub fn title(service: Service) -> &'static str {
    match service {
        Service::Control => "Desktop Control",
        Service::Capture => "Screen Capture",
    }
}

/// How an app shows in its row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub icon: Option<String>,
    /// The second line under the name.
    pub detail: Option<String>,
}

/// How `app` shows: an app of a desktop entry by the entry's name and icon among `installed` (its id without one); an
/// executable by its file name, with its whole path as the second line.
pub fn identity(app: &App, installed: &[Installed]) -> Identity {
    match app {
        App::Desktop(id) => {
            let (name, icon) = crate::screen_sharing::identity(id, installed);
            Identity {
                name,
                icon,
                detail: None,
            }
        }
        App::Executable(path) => Identity {
            name: path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            icon: None,
            detail: Some(path.display().to_string()),
        },
    }
}
