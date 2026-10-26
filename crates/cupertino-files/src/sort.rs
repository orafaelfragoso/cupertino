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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn names(v: &[Entry]) -> Vec<&str> {
        v.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn name_ascending_and_descending() {
        let mut v: Vec<Entry> = ["b2", "b10", "a", "B1"]
            .iter()
            .map(|n| Entry::synthetic(n, false, 0))
            .collect();
        sort_entries(&mut v, &SortSpec::default());
        assert_eq!(names(&v), ["a", "B1", "b2", "b10"]);
        sort_entries(
            &mut v,
            &SortSpec {
                ascending: false,
                ..SortSpec::default()
            },
        );
        assert_eq!(names(&v), ["b10", "b2", "B1", "a"]);
    }

    #[test]
    fn folders_first_is_direction_independent() {
        let mut v = vec![
            Entry::synthetic("z", true, 0),
            Entry::synthetic("a", false, 0),
            Entry::synthetic("m", true, 0),
        ];
        let spec = SortSpec {
            folders_first: true,
            ..SortSpec::default()
        };
        sort_entries(&mut v, &spec);
        assert_eq!(names(&v), ["m", "z", "a"]);
        sort_entries(
            &mut v,
            &SortSpec {
                ascending: false,
                ..spec
            },
        );
        assert_eq!(names(&v), ["z", "m", "a"]);
    }

    #[test]
    fn size_and_date_use_name_as_tiebreak() {
        let mut v = vec![
            Entry::synthetic("b", false, 5),
            Entry::synthetic("a", false, 5),
            Entry::synthetic("c", false, 1),
        ];
        v[0].modified = Some(UNIX_EPOCH + Duration::from_secs(10));
        v[1].modified = Some(UNIX_EPOCH + Duration::from_secs(20));
        sort_entries(
            &mut v,
            &SortSpec {
                key: SortKey::Size,
                ascending: true,
                folders_first: false,
            },
        );
        assert_eq!(names(&v), ["c", "a", "b"]);
        sort_entries(
            &mut v,
            &SortSpec {
                key: SortKey::DateModified,
                ascending: false,
                folders_first: false,
            },
        );
        assert_eq!(names(&v), ["a", "b", "c"]);
    }

    #[test]
    fn insertion_index_keeps_order() {
        let spec = SortSpec::default();
        let mut v: Vec<Entry> = ["a", "c", "e"]
            .iter()
            .map(|n| Entry::synthetic(n, false, 0))
            .collect();
        let e = Entry::synthetic("d", false, 0);
        let i = insertion_index(&v, &e, &spec);
        v.insert(i, e);
        assert_eq!(names(&v), ["a", "c", "d", "e"]);
    }
}
