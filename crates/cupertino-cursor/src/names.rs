//! Names: which freedesktop cursor names each cursor of the design set provides, and which
//! names the procedural set covers.
//!
//! The design set has no plain arrow and no horizontal text cursor (those are drawn by the
//! system elsewhere), so `default` and `text` always come from the procedural set; `wait` and
//! `progress` fall back to the animated "busy but clickable" cursor. Mappings are authored
//! (approximate); aliases are the legacy X11 and toolkit names that applications still request.

/// A cursor and the names it is installed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mapping {
    /// Reference-set directory name (the key in the metadata).
    pub reference: &'static str,
    /// Primary freedesktop (CSS cursor) name; the XCursor file carries this name.
    pub primary: &'static str,
    /// Further names installed as symlinks to the primary file.
    pub aliases: &'static [&'static str],
}

const fn m(
    reference: &'static str,
    primary: &'static str,
    aliases: &'static [&'static str],
) -> Mapping {
    Mapping {
        reference,
        primary,
        aliases,
    }
}

/// Reference cursors that have a freedesktop meaning.
pub const REFERENCE_MAP: &[Mapping] = &[
    m(
        "pointinghand",
        "pointer",
        &["hand1", "hand2", "pointing_hand"],
    ),
    m("openhand", "grab", &["openhand"]),
    m("closedhand", "grabbing", &["closedhand"]),
    m("ibeamvertical", "vertical-text", &[]),
    m(
        "cross",
        "crosshair",
        &["cross", "tcross", "diamond_cross", "cross_reverse"],
    ),
    m(
        "notallowed",
        "not-allowed",
        &["crossed_circle", "forbidden", "no-drop", "dnd-no-drop"],
    ),
    m("copy", "copy", &["dnd-copy"]),
    m("makealias", "alias", &["link", "dnd-link"]),
    m("contextualmenu", "context-menu", &[]),
    m(
        "help",
        "help",
        &["question_arrow", "whats_this", "left_ptr_help"],
    ),
    m(
        "busybutclickable",
        "progress",
        &["left_ptr_watch", "half-busy", "wait", "watch"],
    ),
    m("cell", "cell", &["plus"]),
    m("move", "move", &["all-scroll", "fleur", "size_all"]),
    m("zoomin", "zoom-in", &[]),
    m("zoomout", "zoom-out", &[]),
    m("resizeeast", "e-resize", &["right_side"]),
    m("resizewest", "w-resize", &["left_side"]),
    m("resizenorth", "n-resize", &["top_side"]),
    m("resizesouth", "s-resize", &["bottom_side"]),
    m("resizenortheast", "ne-resize", &["top_right_corner"]),
    m("resizenorthwest", "nw-resize", &["top_left_corner"]),
    m("resizesoutheast", "se-resize", &["bottom_right_corner"]),
    m("resizesouthwest", "sw-resize", &["bottom_left_corner"]),
    m(
        "resizeeastwest",
        "ew-resize",
        &["sb_h_double_arrow", "size_hor", "h_double_arrow"],
    ),
    m(
        "resizenorthsouth",
        "ns-resize",
        &["sb_v_double_arrow", "size_ver", "v_double_arrow"],
    ),
    m("resizenortheastsouthwest", "nesw-resize", &["size_bdiag"]),
    m("resizenorthwestsoutheast", "nwse-resize", &["size_fdiag"]),
    m("resizeleftright", "col-resize", &["split_h"]),
    m("resizeupdown", "row-resize", &["split_v"]),
];

/// Freedesktop names provided by the procedural set only (no reference cursor).
pub const PROCEDURAL_ONLY: &[Mapping] = &[
    m(
        "default",
        "default",
        &["left_ptr", "arrow", "top_left_arrow"],
    ),
    m("text", "text", &["xterm", "ibeam"]),
];

/// Names every complete theme must contain (the CSS cursor set plus the common legacy ones).
pub const REQUIRED_NAMES: &[&str] = &[
    "default",
    "pointer",
    "text",
    "vertical-text",
    "grab",
    "grabbing",
    "not-allowed",
    "no-drop",
    "crosshair",
    "copy",
    "alias",
    "context-menu",
    "help",
    "wait",
    "progress",
    "cell",
    "move",
    "all-scroll",
    "zoom-in",
    "zoom-out",
    "n-resize",
    "s-resize",
    "e-resize",
    "w-resize",
    "ne-resize",
    "nw-resize",
    "se-resize",
    "sw-resize",
    "ew-resize",
    "ns-resize",
    "nesw-resize",
    "nwse-resize",
    "col-resize",
    "row-resize",
    "left_ptr",
    "xterm",
    "watch",
    "fleur",
    "hand2",
];

/// Cursors of the design set that are installed under their own name only.
pub const UNMAPPED_REFERENCE: &[&str] = &[
    "countingdownhand",
    "countingupandownhand",
    "countinguphand",
    "poof",
    "resizedown",
    "resizeleft",
    "resizeright",
    "resizeup",
    "screenshotselection",
    "screenshotwindow",
];

pub fn mapping_for_reference(name: &str) -> Option<&'static Mapping> {
    REFERENCE_MAP.iter().find(|m| m.reference == name)
}

/// Every name (primary and aliases) a mapping installs.
pub fn all_names(m: &Mapping) -> impl Iterator<Item = &'static str> {
    std::iter::once(m.primary).chain(m.aliases.iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn no_name_is_installed_twice() {
        let mut seen = HashSet::new();
        for m in REFERENCE_MAP.iter().chain(PROCEDURAL_ONLY) {
            for n in all_names(m) {
                assert!(seen.insert(n), "{n} is claimed by more than one cursor");
            }
        }
    }

    #[test]
    fn required_names_are_covered() {
        let covered: HashSet<&str> = REFERENCE_MAP
            .iter()
            .chain(PROCEDURAL_ONLY)
            .flat_map(all_names)
            .collect();
        for n in REQUIRED_NAMES {
            assert!(covered.contains(n), "{n} has no mapping");
        }
    }

    #[test]
    fn primary_names_are_unique_and_lookup_works() {
        assert_eq!(
            mapping_for_reference("pointinghand").unwrap().primary,
            "pointer"
        );
        assert_eq!(
            mapping_for_reference("busybutclickable").unwrap().primary,
            "progress"
        );
        assert!(mapping_for_reference("nonsense").is_none());
        assert!(mapping_for_reference("busybutclickable")
            .unwrap()
            .aliases
            .contains(&"wait"));
    }
}
