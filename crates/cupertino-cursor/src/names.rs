//! Names: which freedesktop cursor names each cursor of the procedural set provides.
//!
//! Every cursor has a primary freedesktop (CSS cursor) name, which its XCursor file carries, and aliases: the legacy X11
//! and toolkit names that applications still request, installed as symlinks to the primary file. The mappings are
//! authored (approximate).

/// A cursor and the names it is installed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mapping {
    /// Primary freedesktop (CSS cursor) name; the XCursor file carries this name.
    pub primary: &'static str,
    /// Further names installed as symlinks to the primary file.
    pub aliases: &'static [&'static str],
}

const fn m(primary: &'static str, aliases: &'static [&'static str]) -> Mapping {
    Mapping { primary, aliases }
}

/// The names of every cursor of the procedural set.
pub const NAMES: &[Mapping] = &[
    m("default", &["left_ptr", "arrow", "top_left_arrow"]),
    m("text", &["xterm", "ibeam"]),
    m("pointer", &["hand1", "hand2", "pointing_hand"]),
    m("grab", &["openhand"]),
    m("grabbing", &["closedhand"]),
    m("vertical-text", &[]),
    m(
        "crosshair",
        &["cross", "tcross", "diamond_cross", "cross_reverse"],
    ),
    m(
        "not-allowed",
        &["crossed_circle", "forbidden", "no-drop", "dnd-no-drop"],
    ),
    m("copy", &["dnd-copy"]),
    m("alias", &["link", "dnd-link"]),
    m("context-menu", &[]),
    m("help", &["question_arrow", "whats_this", "left_ptr_help"]),
    m(
        "progress",
        &["left_ptr_watch", "half-busy", "wait", "watch"],
    ),
    m("cell", &["plus"]),
    m("move", &["all-scroll", "fleur", "size_all"]),
    m("zoom-in", &[]),
    m("zoom-out", &[]),
    m("e-resize", &["right_side"]),
    m("w-resize", &["left_side"]),
    m("n-resize", &["top_side"]),
    m("s-resize", &["bottom_side"]),
    m("ne-resize", &["top_right_corner"]),
    m("nw-resize", &["top_left_corner"]),
    m("se-resize", &["bottom_right_corner"]),
    m("sw-resize", &["bottom_left_corner"]),
    m(
        "ew-resize",
        &["sb_h_double_arrow", "size_hor", "h_double_arrow"],
    ),
    m(
        "ns-resize",
        &["sb_v_double_arrow", "size_ver", "v_double_arrow"],
    ),
    m("nesw-resize", &["size_bdiag"]),
    m("nwse-resize", &["size_fdiag"]),
    m("col-resize", &["split_h"]),
    m("row-resize", &["split_v"]),
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
        for m in NAMES {
            for n in all_names(m) {
                assert!(seen.insert(n), "{n} is claimed by more than one cursor");
            }
        }
    }

    #[test]
    fn required_names_are_covered() {
        let covered: HashSet<&str> = NAMES.iter().flat_map(all_names).collect();
        for n in REQUIRED_NAMES {
            assert!(covered.contains(n), "{n} has no mapping");
        }
    }

    #[test]
    fn every_procedural_cursor_has_a_mapping() {
        for kind in crate::procedural::Kind::ALL {
            assert!(
                NAMES.iter().any(|m| m.primary == kind.name()),
                "{} has no mapping",
                kind.name()
            );
        }
    }
}
