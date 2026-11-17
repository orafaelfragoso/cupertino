//! Recents: the files of the session's recently-used list, `$XDG_DATA_HOME/recently-used.xbel` (the desktop
//! bookmark file GTK's recent manager writes), shown as the virtual folder `recent:/// (the GIO
//! scheme for the same list). The design's Recents is a saved search over last-opened dates; its
//! layout and order is not known, so the folder is listed in the window's sort order (approximate).

use crate::entry::Entry;
use cupertino_apps::recent::read_recent_files;
pub use cupertino_apps::recent::{parse_xbel, XBEL_NAME};
use std::io;
use std::path::Path;

/// Where the Recents place points: not a directory, the browser opens it from the bookmark file.
pub const RECENTS: &str = "recent:///";

/// [`RECENTS`] as a path.
pub fn location() -> &'static Path {
    Path::new(RECENTS)
}

/// The listed files that still exist, as entries, in file order. No bookmark file yet means nothing was used.
pub fn recent_entries(xbel: &Path) -> io::Result<Vec<Entry>> {
    // a file removed since it was used drops out of the list
    Ok(read_recent_files(xbel)?
        .into_iter()
        .filter_map(|file| Entry::from_path(file.path).ok())
        .collect())
}
