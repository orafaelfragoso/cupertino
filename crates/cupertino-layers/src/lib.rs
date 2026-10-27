//! Layer rules of the workspace. A crate declares its layer in its manifest
//! (`[package.metadata.cupertino] layer = "<layer>"`); [`RULES`] checks those layers. [`TARGET`] classifies every crate
//! into the twelve target layers and [`TARGET_RULES`] checks that classification, excusing the edges in
//! [`TARGET_EXCEPTIONS`]. Both views also fail on any chain of dependencies that [`DENIED_PATHS`] forbids.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

mod graph;

pub use graph::{denied_paths, dot};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    /// Dependency-free building blocks: geometry, animation, design tokens, settings, toolkit-facing models.
    Foundation,
    /// Pure desktop policy without I/O.
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

/// The kind of a dependency, as `cargo metadata` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Normal,
    Build,
    Dev,
}

impl Kind {
    /// The `kind` field of a `cargo metadata` dependency (`null` for a normal one).
    pub fn from_metadata(kind: Option<&str>) -> Option<Kind> {
        match kind {
            None => Some(Kind::Normal),
            Some("build") => Some(Kind::Build),
            Some("dev") => Some(Kind::Dev),
            Some(_) => None,
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Kind::Normal => "normal",
            Kind::Build => "build",
            Kind::Dev => "dev",
        })
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

/// Layers that must not reach other layers through any chain of dependencies, exceptions included: the toolkit never
/// reaches the server, a shell process or a daemon.
pub const DENIED_PATHS: &[(Layer, &[Layer])] = &[(Ui, &[Core, Shell, Services])];

/// Whether `rules` let a crate of layer `from` depend on a crate of layer `to`.
pub fn allows(rules: Rules, from: Layer, to: Layer) -> bool {
    rules
        .iter()
        .any(|(l, to_layers)| *l == from && to_layers.contains(&to))
}

/// The part of the project that answers for an exception.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    Compositor,
    Rendering,
    Design,
    Apps,
    Services,
    Packaging,
}

impl fmt::Display for Area {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Area::Compositor => "compositor",
            Area::Rendering => "rendering",
            Area::Design => "design",
            Area::Apps => "apps",
            Area::Services => "services",
            Area::Packaging => "packaging",
        })
    }
}

/// Whether an exception stays or is debt that an issue removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tracking {
    /// A reviewed edge that stays: a dev-only edge for tests, or an approved render primitive.
    Permanent,
    /// Migration debt, removed by the work of this issue number.
    Issue(u32),
}

impl fmt::Display for Tracking {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tracking::Permanent => f.write_str("permanent"),
            Tracking::Issue(n) => write!(f, "migration #{n}"),
        }
    }
}

/// A forbidden edge that the check excuses. It matches only an edge of the same kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exception {
    pub from: &'static str,
    pub to: &'static str,
    pub kind: Kind,
    pub tracking: Tracking,
    pub area: Area,
    pub reason: &'static str,
}

impl Exception {
    pub fn matches(&self, edge: &Edge) -> bool {
        self.from == edge.from && self.to == edge.to && self.kind == edge.kind
    }
}

/// Known violations of [`RULES`]. Each one must still exist: a stale entry is an error too, so the list only shrinks.
pub const PENDING: &[Exception] = &[];

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
    ("cupertino-icons", Render),
    ("cupertino-input", Models),
    ("cupertino-keyboard", Models),
    ("cupertino-launcher", Shell),
    ("cupertino-layers", Tools),
    ("cupertino-login", Shell),
    ("cupertino-login-ipc", Protocol),
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
    ("cupertino-text-field", Foundation),
    ("cupertino-theme", Foundation),
    ("cupertino-thumbnails", Foundation),
    ("cupertino-typeahead", Foundation),
    ("cupertino-windowmanager", Shell),
    ("cupertino-wm", Models),
    ("xkbcommon", Foundation),
];

const APPS_DRAW: &str =
    "apps paint fields, shapes, text and system icons with desktop render crates until they draw \
                         through the toolkit (WP-10)";

/// Edges that exist today and that [`TARGET_RULES`] forbid. Stale entries are errors.
pub const TARGET_EXCEPTIONS: &[Exception] = &[
    Exception {
        from: "cupertino-files",
        to: "cupertino-decorations",
        kind: Kind::Normal,
        tracking: Tracking::Issue(46),
        area: Area::Apps,
        reason: APPS_DRAW,
    },
    Exception {
        from: "cupertino-files",
        to: "cupertino-icons",
        kind: Kind::Normal,
        tracking: Tracking::Issue(46),
        area: Area::Apps,
        reason: APPS_DRAW,
    },
    Exception {
        from: "cupertino-server",
        to: "cupertino-client",
        kind: Kind::Dev,
        tracking: Tracking::Permanent,
        area: Area::Compositor,
        reason: "server integration tests connect real shell clients",
    },
    Exception {
        from: "cupertino-settings",
        to: "cupertino-alsa",
        kind: Kind::Normal,
        tracking: Tracking::Issue(21),
        area: Area::Services,
        reason: "the sound pane drives the mixer itself until audio goes through a service",
    },
    Exception {
        from: "cupertino-settings",
        to: "cupertino-decorations",
        kind: Kind::Normal,
        tracking: Tracking::Issue(46),
        area: Area::Apps,
        reason: APPS_DRAW,
    },
    Exception {
        from: "cupertino-settings",
        to: "cupertino-icons",
        kind: Kind::Normal,
        tracking: Tracking::Issue(46),
        area: Area::Apps,
        reason: APPS_DRAW,
    },
    Exception {
        from: "cupertino-text-field",
        to: "cupertino-keyboard",
        kind: Kind::Normal,
        tracking: Tracking::Issue(47),
        area: Area::Design,
        reason: "the text field takes its editing bindings from the compositor keyboard model until the toolkit text \
                 crate owns them (Phase 3, WP-07)",
    },
];

/// A dependency between two workspace crates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: String,
    pub from_layer: Layer,
    pub to: String,
    pub to_layer: Layer,
    pub kind: Kind,
}

/// A dependency of a workspace crate: `(from, to, kind)`.
pub type Dep = (String, String, Kind);

/// The dependencies in `deps` whose ends both have a layer in `layers`, in `deps` order.
pub fn edges(layers: &BTreeMap<String, Layer>, deps: &BTreeSet<Dep>) -> Vec<Edge> {
    deps.iter()
        .filter_map(|(from, to, kind)| {
            Some(Edge {
                from: from.clone(),
                from_layer: *layers.get(from)?,
                to: to.clone(),
                to_layer: *layers.get(to)?,
                kind: *kind,
            })
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Problem {
    Forbidden(Edge),
    DeniedPath(Vec<String>),
    StaleException(Exception),
    Unclassified(String),
    NotACrate(&'static str),
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Forbidden(e) => write!(
                f,
                "{} ({}) must not depend on {} ({}), {} dependency",
                e.from, e.from_layer, e.to, e.to_layer, e.kind
            ),
            Problem::DeniedPath(path) => {
                write!(f, "denied dependency path {}", path.join(" -> "))
            }
            Problem::StaleException(x) => write!(
                f,
                "{} no longer has a {} dependency on {}: remove its exception",
                x.from, x.kind, x.to
            ),
            Problem::Unclassified(name) => write!(f, "{name} has no entry in TARGET"),
            Problem::NotACrate(name) => {
                write!(f, "{name} is in TARGET but is not a workspace crate")
            }
        }
    }
}

/// Every edge `rules` forbid that `exceptions` do not excuse, every path `denied` forbids, then every exception
/// without a matching edge.
pub fn check(
    rules: Rules,
    edges: &[Edge],
    exceptions: &[Exception],
    denied: &[(Layer, &[Layer])],
) -> Vec<Problem> {
    let mut out: Vec<Problem> = edges
        .iter()
        .filter(|e| !allows(rules, e.from_layer, e.to_layer) && exception(exceptions, e).is_none())
        .cloned()
        .map(Problem::Forbidden)
        .collect();
    out.extend(
        denied_paths(edges, denied)
            .into_iter()
            .map(Problem::DeniedPath),
    );
    for x in exceptions {
        if !edges.iter().any(|e| x.matches(e)) {
            out.push(Problem::StaleException(*x));
        }
    }
    out
}

/// The exception that excuses `edge`.
pub fn exception<'a>(exceptions: &'a [Exception], edge: &Edge) -> Option<&'a Exception> {
    exceptions.iter().find(|x| x.matches(edge))
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
        kind: Kind::Normal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exc(from: &'static str, to: &'static str, kind: Kind, tracking: Tracking) -> Exception {
        Exception {
            from,
            to,
            kind,
            tracking,
            area: Area::Compositor,
            reason: "test",
        }
    }

    #[test]
    fn downward_and_library_sideways_edges_pass() {
        let edges = [
            edge("wm", Models, "geometry", Foundation),
            edge("dock", Shell, "wm", Models),
            edge("glass", Render, "theme", Foundation),
        ];
        assert_eq!(check(RULES, &edges, &[], DENIED_PATHS), vec![]);
    }

    #[test]
    fn upward_and_shippable_sideways_edges_fail() {
        let up = edge("wm", Models, "glass", Render);
        let shell = edge("dock", Shell, "menubar", Shell);
        let core = edge("dock", Shell, "server", Core);
        let problems = check(
            RULES,
            &[up.clone(), shell.clone(), core.clone()],
            &[],
            DENIED_PATHS,
        );
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
            check(
                TARGET_RULES,
                &[toolkit, renderer.clone()],
                &[],
                DENIED_PATHS
            ),
            vec![Problem::Forbidden(renderer.clone())]
        );
        assert_eq!(
            Problem::Forbidden(renderer).to_string(),
            "files (apps) must not depend on glass (render), normal dependency"
        );
    }

    #[test]
    fn a_toolkit_chain_to_the_server_fails_even_when_its_edges_are_excused() {
        let up = edge("net", Transport, "server", Core);
        let edges = [edge("kit", Ui, "net", Transport), up.clone()];
        let problems = check(
            TARGET_RULES,
            &edges,
            &[exc("net", "server", Kind::Normal, Tracking::Issue(1))],
            DENIED_PATHS,
        );
        assert_eq!(
            problems,
            vec![Problem::DeniedPath(vec![
                "kit".into(),
                "net".into(),
                "server".into()
            ])]
        );
        assert_eq!(
            problems[0].to_string(),
            "denied dependency path kit -> net -> server"
        );
    }

    #[test]
    fn an_exception_excuses_only_its_kind_and_goes_stale_once_the_edge_is_gone() {
        let dev = Edge {
            kind: Kind::Dev,
            ..edge("server", Core, "client", Transport)
        };
        let normal = edge("server", Core, "client", Transport);
        let x = exc("server", "client", Kind::Dev, Tracking::Permanent);
        assert_eq!(
            check(TARGET_RULES, std::slice::from_ref(&dev), &[x], &[]),
            vec![]
        );
        assert_eq!(
            check(TARGET_RULES, std::slice::from_ref(&normal), &[x], &[]),
            vec![
                Problem::Forbidden(normal.clone()),
                Problem::StaleException(x)
            ]
        );
        assert_eq!(
            Problem::StaleException(x).to_string(),
            "server no longer has a dev dependency on client: remove its exception"
        );
    }

    #[test]
    fn edges_keep_only_dependencies_between_crates_with_a_layer() {
        let layers = BTreeMap::from([("dock".to_owned(), Shell), ("wm".to_owned(), Models)]);
        let deps = BTreeSet::from([
            ("dock".to_owned(), "serde".to_owned(), Kind::Normal),
            ("dock".to_owned(), "wm".to_owned(), Kind::Dev),
        ]);
        assert_eq!(
            edges(&layers, &deps),
            vec![Edge {
                kind: Kind::Dev,
                ..edge("dock", Shell, "wm", Models)
            }]
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
    fn layer_and_kind_names_parse() {
        assert_eq!(Layer::from_name("transport"), Some(Transport));
        assert_eq!(Layer::from_name("widgets"), None);
        assert_eq!(Kind::from_metadata(None), Some(Kind::Normal));
        assert_eq!(Kind::from_metadata(Some("dev")), Some(Kind::Dev));
        assert_eq!(Kind::from_metadata(Some("proc")), None);
    }
}
