//! Applications: the applications installed for the session, shown as the virtual folder `applications:///`. Each is
//! listed once under its name, with its own icon, from the desktop entries of `applications/` below every data folder
//! (`$XDG_DATA_HOME` first, then `$XDG_DATA_DIRS`): the first folder that has an entry's id wins, entries that hide
//! themselves (`NoDisplay`, `Hidden`, `OnlyShowIn`, `NotShowIn`, a failing `TryExec`) and command-line programs are
//! left out, as the launcher leaves them out, and so is every file that is not an application's entry (association
//! lists, caches). An item is the entry's file: opening it starts the application ([`crate::open::application_at`]).

use crate::entry::Entry;
use crate::natural::NameKey;
use cupertino_apps::entry::{index_applications, xdg_application_dirs, Locale, Visibility};
use std::path::{Path, PathBuf};

/// Where the Applications place points: not a directory, the browser lists the installed applications there.
pub const APPLICATIONS: &str = "applications:///";

/// The type of an application's item.
pub const APPLICATION_TYPE: &str = "application/x-desktop";

/// [`APPLICATIONS`] as a path.
pub fn location() -> &'static Path {
    Path::new(APPLICATIONS)
}

/// The folders the session's applications are installed in, most important first.
pub fn application_dirs() -> Vec<PathBuf> {
    xdg_application_dirs()
}

/// Whether the session has any applications folder: the place is shown only then.
pub fn any_installed(dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|d| d.is_dir())
}

/// The applications installed in `dirs` (most important first), as items named after their entries, for a person in
/// `locale` on a desktop with `visibility`.
pub fn application_entries(
    dirs: &[PathBuf],
    locale: &Locale,
    visibility: &Visibility,
) -> Vec<Entry> {
    index_applications(dirs, locale, visibility)
        .into_iter()
        .filter_map(|app| {
            let mut entry = Entry::from_path(app.source?).ok()?;
            entry.name_key = NameKey::new(&app.name);
            entry.name = app.name;
            entry.hidden = false;
            entry.mime = Some(APPLICATION_TYPE.into());
            // an application that names no icon draws the generic one
            entry.icon = Some(app.icon.unwrap_or_default());
            Some(entry)
        })
        .collect()
}

/// [`application_entries`] of the session's folders, in its locale and on its desktop.
pub fn session_entries(dirs: &[PathBuf]) -> Vec<Entry> {
    application_entries(dirs, &Locale::from_env(), &Visibility::from_env())
}
