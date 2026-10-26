//! Layer rules of the prototype workspace. A crate declares its layer in its manifest
//! (`[package.metadata.cupertino] layer = "<layer>"`). Dependencies may point down the stack, and sideways only inside the
//! library layers; the shippable layers never depend on the core or on each other.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    /// Dependency-free building blocks: geometry, animation, design tokens, settings.
    Foundation,
    /// Pure domain logic without I/O.
    Models,
    /// Kernel interfaces: devices, input events, seat, fonts.
    Platform,
    /// Rendering and the shell client toolkit.
    Render,
    /// The display server.
    Core,
    /// Shell processes, each shipped on its own.
    Shell,
    /// System daemons, each shipped on its own.
    Services,
    /// Applications.
    Apps,
    /// Developer and theming tools.
    Tools,
}

impl Layer {
    /// The layer named `name` (the value of `package.metadata.cupertino.layer`).
    pub fn from_name(name: &str) -> Option<Layer> {
        Some(match name {
            "foundation" => Layer::Foundation,
            "models" => Layer::Models,
            "platform" => Layer::Platform,
            "render" => Layer::Render,
            "core" => Layer::Core,
            "shell" => Layer::Shell,
            "services" => Layer::Services,
            "apps" => Layer::Apps,
            "tools" => Layer::Tools,
            _ => return None,
        })
    }

    /// Whether a crate of this layer may depend on a crate of `to`.
    pub fn may_depend_on(self, to: Layer) -> bool {
        use Layer::*;
        match self {
            Foundation => to == Foundation,
            Models => matches!(to, Foundation | Models),
            Platform => matches!(to, Foundation | Platform),
            Render => matches!(to, Foundation | Platform | Render),
            Core | Shell | Services | Apps | Tools => matches!(to, Foundation | Models | Platform | Render),
        }
    }
}

/// A dependency between two workspace crates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: String,
    pub from_layer: Layer,
    pub to: String,
    pub to_layer: Layer,
}

/// Known violations that a planned phase removes. Each one must still exist: a stale entry is an error
/// too, so the list shrinks as the work lands.
pub const PENDING: &[(&str, &str, &str)] = &[];

#[derive(Debug, PartialEq, Eq)]
pub enum Problem {
    Forbidden(Edge),
    StalePending { from: &'static str, to: &'static str },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Forbidden(e) => write!(f, "{} ({:?}) must not depend on {} ({:?})", e.from, e.from_layer, e.to, e.to_layer),
            Problem::StalePending { from, to } => write!(f, "{from} no longer depends on {to}: remove the entry from PENDING"),
        }
    }
}

/// Every forbidden edge not listed in `pending`, then every `pending` entry without a matching edge.
pub fn check(edges: &[Edge], pending: &[(&'static str, &'static str, &str)]) -> Vec<Problem> {
    let listed = |e: &Edge| pending.iter().any(|(f, t, _)| *f == e.from && *t == e.to);
    let mut out: Vec<Problem> =
        edges.iter().filter(|e| !e.from_layer.may_depend_on(e.to_layer) && !listed(e)).cloned().map(Problem::Forbidden).collect();
    for &(from, to, _) in pending {
        if !edges.iter().any(|e| e.from == from && e.to == to) {
            out.push(Problem::StalePending { from, to });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(from: &str, fl: Layer, to: &str, tl: Layer) -> Edge {
        Edge { from: from.into(), from_layer: fl, to: to.into(), to_layer: tl }
    }

    #[test]
    fn downward_and_library_sideways_edges_pass() {
        let edges = [
            edge("wm", Layer::Models, "geometry", Layer::Foundation),
            edge("dock", Layer::Shell, "wm", Layer::Models),
            edge("glass", Layer::Render, "theme", Layer::Foundation),
        ];
        assert_eq!(check(&edges, &[]), vec![]);
    }

    #[test]
    fn upward_and_shippable_sideways_edges_fail() {
        let up = edge("wm", Layer::Models, "glass", Layer::Render);
        let shell = edge("dock", Layer::Shell, "menubar", Layer::Shell);
        let core = edge("dock", Layer::Shell, "server", Layer::Core);
        let problems = check(&[up.clone(), shell.clone(), core.clone()], &[]);
        assert_eq!(problems, vec![Problem::Forbidden(up), Problem::Forbidden(shell), Problem::Forbidden(core)]);
    }

    #[test]
    fn pending_entries_excuse_an_edge_and_go_stale_once_it_is_gone() {
        let e = edge("server", Layer::Core, "dock", Layer::Shell);
        let pending = [("server", "dock", "phase 1")];
        assert_eq!(check(std::slice::from_ref(&e), &pending), vec![]);
        assert_eq!(check(&[], &pending), vec![Problem::StalePending { from: "server", to: "dock" }]);
    }
}
