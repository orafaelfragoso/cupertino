//! Layer rules of the workspace. A crate declares its layer in its manifest
//! (`[package.metadata.cupertino] layer = "<layer>"`); [`RULES`] checks those layers. [`TARGET`] classifies every crate
//! into the twelve target layers and [`TARGET_RULES`] checks that classification, excusing the edges in
//! [`TARGET_EXCEPTIONS`] until the migration removes them.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

mod graph;

pub use graph::{denied_paths, dot};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    /// Dependency-free building blocks: geometry, animation, design tokens, settings.
    Foundation,
    /// Pure domain logic without I/O.
    Models,
    /// Kernel interfaces: devices, input events, seat.
    Platform,
    /// Generated Wayland message types and versioned IPC schemas.
    Protocol,
    /// Client connection, seat and surface transport, without widgets.
    Transport,
    /// Rendering fundamentals and desktop GPU effects.
    Render,
    /// The application toolkit.
    Ui,
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

use Layer::*;

impl Layer {
    pub const ALL: [Layer; 12] = [
        Foundation, Models, Platform, Protocol, Transport, Render, Ui, Core, Shell, Services, Apps,
        Tools,
    ];

    /// The value of `package.metadata.cupertino.layer`.
    pub fn name(self) -> &'static str {
        match self {
            Foundation => "foundation",
            Models => "models",
            Platform => "platform",
            Protocol => "protocol",
            Transport => "transport",
            Render => "render",
            Ui => "ui",
            Core => "core",
            Shell => "shell",
            Services => "services",
            Apps => "apps",
            Tools => "tools",
        }
    }

    pub fn from_name(name: &str) -> Option<Layer> {
        Layer::ALL.into_iter().find(|l| l.name() == name)
    }
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The layers each layer may depend on. A crate may only declare a layer that has a row.
pub type Rules = &'static [(Layer, &'static [Layer])];

/// The rules the manifests follow today: dependencies point down the stack, sideways only inside the library layers,
/// and the shippable layers never depend on the core or on each other.
pub const RULES: Rules = &[
    (Foundation, &[Foundation]),
    (Models, &[Foundation, Models]),
    (Platform, &[Foundation, Platform]),
    (Render, &[Foundation, Platform, Render]),
    (Core, &[Foundation, Models, Platform, Render]),
    (Shell, &[Foundation, Models, Platform, Render]),
    (Services, &[Foundation, Models, Platform, Render]),
    (Apps, &[Foundation, Models, Platform, Render]),
    (Tools, &[Foundation, Models, Platform, Render]),
];

/// The target rules: twelve layers; same-layer dependencies only inside the library layers, `services` and `tools`.
pub const TARGET_RULES: Rules = &[
    (Foundation, &[Foundation]),
    (Models, &[Foundation, Models]),
    (Platform, &[Foundation, Platform]),
    (Protocol, &[Foundation, Protocol]),
    (Transport, &[Foundation, Platform, Protocol, Transport]),
    (Render, &[Foundation, Platform, Render]),
    (Ui, &[Foundation, Protocol, Transport, Ui]),
    (Core, &[Foundation, Models, Platform, Protocol, Render]),
    (
        Shell,
        &[
            Foundation, Models, Platform, Protocol, Transport, Render, Ui,
        ],
    ),
    (
        Services,
        &[Foundation, Models, Platform, Protocol, Services],
    ),
    (Apps, &[Foundation, Models, Protocol, Transport, Ui]),
    (
        Tools,
        &[
            Foundation, Models, Platform, Protocol, Transport, Render, Ui, Tools,
        ],
    ),
];

/// Layers that must not reach other layers through any chain of dependencies: the toolkit never reaches the server,
/// a shell process or a daemon.
pub const DENIED_PATHS: &[(Layer, &[Layer])] = &[(Ui, &[Core, Shell, Services])];

/// Whether `rules` let a crate of layer `from` depend on a crate of layer `to`.
pub fn allows(rules: Rules, from: Layer, to: Layer) -> bool {
    rules
        .iter()
        .any(|(l, to_layers)| *l == from && to_layers.contains(&to))
}

/// Known violations of [`RULES`] that a planned phase removes, as `(from, to, reason)`. Each one must still exist: a
/// stale entry is an error too, so the list only shrinks.
pub const PENDING: &[(&str, &str, &str)] = &[];

/// The target layer of every workspace crate.
pub const TARGET: &[(&str, Layer)] = &[
    ("cupertino-alsa", Platform),
    ("cupertino-animation", Foundation),
    ("cupertino-apps", Foundation),
    ("cupertino-assets", Foundation),
    ("cupertino-bluetooth", Platform),
    ("cupertino-btd", Services),
    ("cupertino-client", Transport),
    ("cupertino-controlcenter", Shell),
    ("cupertino-cursor", Render),
    ("cupertino-decorations", Render),
    ("cupertino-defaults", Foundation),
    ("cupertino-devices", Platform),
    ("cupertino-displays", Models),
    ("cupertino-dock", Shell),
    ("cupertino-effects", Models),
    ("cupertino-evdev", Platform),
    ("cupertino-filepicker", Foundation),
    ("cupertino-files", Apps),
    ("cupertino-geometry", Foundation),
    ("cupertino-glass", Render),
    ("cupertino-gtk-theme", Tools),
    ("cupertino-icons", Render),
    ("cupertino-input", Models),
    ("cupertino-keyboard", Models),
    ("cupertino-launcher", Shell),
    ("cupertino-layers", Tools),
    ("cupertino-login", Shell),
    ("cupertino-login-ipc", Models),
    ("cupertino-menubar", Shell),
    ("cupertino-netd", Services),
    ("cupertino-netlink", Platform),
    ("cupertino-notifications", Shell),
    ("cupertino-osd", Shell),
    ("cupertino-overview", Models),
    ("cupertino-powerd", Services),
    ("cupertino-protocols", Protocol),
    ("cupertino-screenshot", Shell),
    ("cupertino-seat", Platform),
    ("cupertino-server", Core),
    ("cupertino-sessiond", Services),
    ("cupertino-settings", Apps),
    ("cupertino-stage", Models),
    ("cupertino-testclient", Tools),
    ("cupertino-text-field", Models),
    ("cupertino-theme", Foundation),
    ("cupertino-thumbnails", Foundation),
    ("cupertino-typeahead", Foundation),
    ("cupertino-windowmanager", Shell),
    ("cupertino-wm", Models),
    ("xkbcommon", Foundation),
];

/// Edges that exist today and that [`TARGET_RULES`] forbid, as `(from, to, reason)`. Stale entries are errors.
pub const TARGET_EXCEPTIONS: &[(&str, &str, &str)] = &[
    (
        "cupertino-files",
        "cupertino-decorations",
        "apps draw fields, shapes and text with the decoration crate until the toolkit owns them",
    ),
    (
        "cupertino-files",
        "cupertino-icons",
        "apps load system icons through the icon crate until the toolkit has an icon adapter",
    ),
    (
        "cupertino-server",
        "cupertino-client",
        "dev-dependency: server integration tests connect through the client library",
    ),
    (
        "cupertino-settings",
        "cupertino-alsa",
        "the sound pane opens the mixer itself until a daemon serves it",
    ),
    (
        "cupertino-settings",
        "cupertino-decorations",
        "apps draw fields, shapes and text with the decoration crate until the toolkit owns them",
    ),
    (
        "cupertino-settings",
        "cupertino-icons",
        "apps load system icons through the icon crate until the toolkit has an icon adapter",
    ),
];

/// A dependency between two workspace crates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: String,
    pub from_layer: Layer,
    pub to: String,
    pub to_layer: Layer,
}

/// The dependencies in `deps` whose ends both have a layer in `layers`, in `deps` order.
pub fn edges(layers: &BTreeMap<String, Layer>, deps: &BTreeSet<(String, String)>) -> Vec<Edge> {
    deps.iter()
        .filter_map(|(from, to)| {
            Some(Edge {
                from: from.clone(),
                from_layer: *layers.get(from)?,
                to: to.clone(),
                to_layer: *layers.get(to)?,
            })
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Problem {
    Forbidden(Edge),
    StaleException {
        from: &'static str,
        to: &'static str,
    },
    Unclassified(String),
    NotACrate(&'static str),
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Forbidden(e) => write!(
                f,
                "{} ({}) must not depend on {} ({})",
                e.from, e.from_layer, e.to, e.to_layer
            ),
            Problem::StaleException { from, to } => {
                write!(f, "{from} no longer depends on {to}: remove its exception")
            }
            Problem::Unclassified(name) => write!(f, "{name} has no entry in TARGET"),
            Problem::NotACrate(name) => {
                write!(f, "{name} is in TARGET but is not a workspace crate")
            }
        }
    }
}

/// Every edge `rules` forbid that `exceptions` do not list, then every exception without a matching edge.
pub fn check(
    rules: Rules,
    edges: &[Edge],
    exceptions: &[(&'static str, &'static str, &str)],
) -> Vec<Problem> {
    let mut out: Vec<Problem> = edges
        .iter()
        .filter(|e| !allows(rules, e.from_layer, e.to_layer) && exception(exceptions, e).is_none())
        .cloned()
        .map(Problem::Forbidden)
        .collect();
    for &(from, to, _) in exceptions {
        if !edges.iter().any(|e| e.from == from && e.to == to) {
            out.push(Problem::StaleException { from, to });
        }
    }
    out
}

/// The reason `exceptions` give for `edge`.
pub fn exception<'a>(exceptions: &[(&str, &str, &'a str)], edge: &Edge) -> Option<&'a str> {
    exceptions
        .iter()
        .find(|(f, t, _)| *f == edge.from && *t == edge.to)
        .map(|&(_, _, reason)| reason)
}

/// The target layer of each of `crates`, with a problem for each crate `target` misses and each entry that names no
/// crate.
pub fn classify<'a>(
    crates: impl IntoIterator<Item = &'a String>,
    target: &[(&'static str, Layer)],
) -> (BTreeMap<String, Layer>, Vec<Problem>) {
    let crates: BTreeSet<&str> = crates.into_iter().map(String::as_str).collect();
    let mut layers = BTreeMap::new();
    let mut problems = Vec::new();
    for &name in &crates {
        match target.iter().find(|(n, _)| *n == name) {
            Some(&(_, layer)) => {
                layers.insert(name.to_owned(), layer);
            }
            None => problems.push(Problem::Unclassified(name.to_owned())),
        }
    }
    for &(name, _) in target {
        if !crates.contains(name) {
            problems.push(Problem::NotACrate(name));
        }
    }
    (layers, problems)
}

#[cfg(test)]
pub(crate) fn edge(from: &str, from_layer: Layer, to: &str, to_layer: Layer) -> Edge {
    Edge {
        from: from.into(),
        from_layer,
        to: to.into(),
        to_layer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downward_and_library_sideways_edges_pass() {
        let edges = [
            edge("wm", Models, "geometry", Foundation),
            edge("dock", Shell, "wm", Models),
            edge("glass", Render, "theme", Foundation),
        ];
        assert_eq!(check(RULES, &edges, &[]), vec![]);
    }

    #[test]
    fn upward_and_shippable_sideways_edges_fail() {
        let up = edge("wm", Models, "glass", Render);
        let shell = edge("dock", Shell, "menubar", Shell);
        let core = edge("dock", Shell, "server", Core);
        let problems = check(RULES, &[up.clone(), shell.clone(), core.clone()], &[]);
        assert_eq!(
            problems,
            vec![
                Problem::Forbidden(up),
                Problem::Forbidden(shell),
                Problem::Forbidden(core)
            ]
        );
    }

    #[test]
    fn target_rules_let_a_shell_use_the_toolkit_and_keep_apps_off_the_renderer() {
        let toolkit = edge("dock", Shell, "kit", Ui);
        let renderer = edge("files", Apps, "glass", Render);
        assert_eq!(
            check(TARGET_RULES, &[toolkit, renderer.clone()], &[]),
            vec![Problem::Forbidden(renderer.clone())]
        );
        assert_eq!(
            Problem::Forbidden(renderer).to_string(),
            "files (apps) must not depend on glass (render)"
        );
    }

    #[test]
    fn exceptions_excuse_an_edge_and_go_stale_once_it_is_gone() {
        let e = edge("server", Core, "dock", Shell);
        let exceptions = [("server", "dock", "phase 1")];
        assert_eq!(check(RULES, std::slice::from_ref(&e), &exceptions), vec![]);
        assert_eq!(
            check(RULES, &[], &exceptions),
            vec![Problem::StaleException {
                from: "server",
                to: "dock"
            }]
        );
    }

    #[test]
    fn edges_keep_only_dependencies_between_crates_with_a_layer() {
        let layers = BTreeMap::from([("dock".to_owned(), Shell), ("wm".to_owned(), Models)]);
        let deps = BTreeSet::from([
            ("dock".to_owned(), "serde".to_owned()),
            ("dock".to_owned(), "wm".to_owned()),
        ]);
        assert_eq!(
            edges(&layers, &deps),
            vec![edge("dock", Shell, "wm", Models)]
        );
    }

    #[test]
    fn classify_reports_missing_and_unknown_crates() {
        let crates = ["dock".to_owned(), "wm".to_owned()];
        let (layers, problems) = classify(&crates, &[("wm", Models), ("gone", Apps)]);
        assert_eq!(layers, BTreeMap::from([("wm".to_owned(), Models)]));
        assert_eq!(
            problems,
            vec![
                Problem::Unclassified("dock".into()),
                Problem::NotACrate("gone")
            ]
        );
    }

    #[test]
    fn layer_names_parse() {
        assert_eq!(Layer::from_name("transport"), Some(Transport));
        assert_eq!(Layer::from_name("widgets"), None);
    }
}
