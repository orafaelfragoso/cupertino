//! Sorting of directory entries.
//!
//! Ties on any key fall back to the name collation of [`crate::natural`], so the order is total.
//! "Folders first" is off by default (preference default).

use crate::entry::Entry;
use crate::mime;
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SortKey {
    None,
    Name,
    Kind,
    DateModified,
    DateCreated,
    DateAccessed,
    Size,
    Tags,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SortSpec {
    pub key: SortKey,
    pub ascending: bool,
    pub folders_first: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        SortSpec {
            key: SortKey::Name,
            ascending: true,
            folders_first: false,
        }
    }
}

fn by_name(a: &Entry, b: &Entry) -> Ordering {
    a.name_key.cmp(&b.name_key)
}

/// Total order of two entries under `spec`.
pub fn compare(a: &Entry, b: &Entry, spec: &SortSpec) -> Ordering {
    if spec.folders_first && a.is_dir != b.is_dir {
        // folders stay on top whatever the direction
        return if a.is_dir {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let primary = match spec.key {
        SortKey::None | SortKey::Name => Ordering::Equal,
        SortKey::Kind => mime::kind_description(&mime::entry_mime(a))
            .cmp(&mime::kind_description(&mime::entry_mime(b))),
        SortKey::DateModified => a.modified.cmp(&b.modified),
        SortKey::DateCreated => a.created.cmp(&b.created),
        SortKey::DateAccessed => a.accessed.cmp(&b.accessed),
        SortKey::Size => a.size.cmp(&b.size),
        SortKey::Tags => a.tags.first().cmp(&b.tags.first()),
    };
    let primary = if spec.ascending {
        primary
    } else {
        primary.reverse()
    };
    primary.then_with(|| {
        let n = by_name(a, b);
        if spec.key == SortKey::Name && !spec.ascending {
            n.reverse()
        } else {
            n
        }
    })
}

pub fn sort_entries(entries: &mut [Entry], spec: &SortSpec) {
    if spec.key == SortKey::None && !spec.folders_first {
        return;
    }
    entries.sort_by(|a, b| compare(a, b, spec));
}

/// Index at which `e` must be inserted into `sorted` to keep it ordered.
pub fn insertion_index(sorted: &[Entry], e: &Entry, spec: &SortSpec) -> usize {
    sorted.partition_point(|x| compare(x, e, spec) == Ordering::Less)
}
