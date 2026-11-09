//! Layer rules of the workspace. A crate declares its layer in its manifest
//! (`[package.metadata.cupertino] layer = "<layer>"`); [`RULES`] checks those layers, excusing the edges in
//! [`EXCEPTIONS`]. The check also fails on any chain of dependencies that [`DENIED_PATHS`] forbids, and on any chain of
//! normal dependencies, external crates included, that [`DENIED_CRATES`] forbids.
//!
//! [`check_workspace`] runs every check on a checkout, as the `cupertino-layers` binary and `cupertino dev layers` do.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

mod closure;
mod graph;
mod workspace;

pub use closure::REQUIRED_MPL_ROOTS;
pub use graph::{denied_crate_paths, denied_paths, dot};
pub use workspace::{check_workspace, check_workspace_with, Findings, LayersError, Policy, POLICY};

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

/// Twelve layers: dependencies point down the stack, same-layer dependencies only inside the library layers,
/// `services` and `tools`.
pub const RULES: Rules = &[
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

/// Crates that must not reach these crates through any chain of normal dependencies: the public transport speaks
/// standard Wayland protocols only, never the desktop's private ones or another compositor's extensions, and the
/// public crates that applications link stay off the private protocols and the privileged shell client.
pub const DENIED_CRATES: &[(&str, &[&str])] = &[
    (
        "cupertino-wayland-client",
        &[
            "cupertino-protocols",
            "wayland-protocols-wlr",
            "wayland-protocols-misc",
            "wayland-protocols-plasma",
        ],
    ),
    ("cupertino-app-protocols", &["cupertino-protocols"]),
    (
        "cupertino-protocol-app",
        &["cupertino-protocols", "cupertino-shell-client"],
    ),
    (
        "cupertino-ui-menus",
        &["cupertino-protocols", "cupertino-shell-client"],
    ),
    (
        "cupertino-ui-wayland",
        &["cupertino-protocols", "cupertino-shell-client"],
    ),
    (
        "cupertino-ui-abi",
        &["cupertino-protocols", "cupertino-shell-client"],
    ),
    (
        "cupertino-kit",
        &["cupertino-protocols", "cupertino-shell-client"],
    ),
    (
        "cupertino-ui-controls",
        &["cupertino-protocols", "cupertino-shell-client"],
    ),
    (
        "cupertino-ui-core",
        &["cupertino-protocols", "cupertino-shell-client"],
    ),
    (
        "cupertino-ui-layout",
        &["cupertino-protocols", "cupertino-shell-client"],
    ),
    (
        "cupertino-ui-platform",
        &["cupertino-protocols", "cupertino-shell-client"],
    ),
    (
        "cupertino-ui-render",
        &[
            "cupertino-protocols",
            "cupertino-shell-client",
            "cupertino-ui-wayland",
            "cupertino-wayland-client",
            "cupertino-server",
            "smithay",
            "wayland-client",
            "wayland-backend",
            "wayland-protocols",
            "wayland-server",
            "pipewire",
        ],
    ),
    (
        "cupertino-ui-test",
        &["cupertino-protocols", "cupertino-shell-client"],
    ),
    (
        "cupertino-ui-text",
        &["cupertino-protocols", "cupertino-shell-client"],
    ),
    (
        "cupertino-ui-theme",
        &["cupertino-protocols", "cupertino-shell-client"],
    ),
    ("cupertino-files", &["cupertino-shell-client"]),
    ("cupertino-settings", &["cupertino-shell-client"]),
    ("cupertino-scene", RENDERER_AND_PROTOCOL),
    ("cupertino-render-api", RENDERER_AND_PROTOCOL),
    ("cupertino-effects", RENDERER_AND_PROTOCOL),
    ("cupertino-wm", RENDERER_AND_PROTOCOL),
    ("cupertino-stage", RENDERER_AND_PROTOCOL),
];

/// What the scene, the renderer seam and the policy models that feed them must never reach: a graphics API, the
/// compositor library, a window-system protocol, the desktop's private protocols, or the server that implements the
/// renderer. Keeping them out of the dependency closure is what lets another GPU backend replace the GLES one.
const RENDERER_AND_PROTOCOL: &[&str] = &[
    "smithay",
    "wgpu",
    "wgpu-core",
    "wgpu-hal",
    "wayland-server",
    "wayland-client",
    "wayland-backend",
    "wayland-protocols",
    "cupertino-glass",
    "cupertino-protocols",
    "cupertino-server",
];

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

const SOUND_PANE: &str =
    "the sound pane drives the default sink and the mixer itself until audio goes through a service";

const APP_MENUS: &str =
    "the menu bar's integration test opens and activates the menus the apps publish";

/// Edges that exist today and that [`RULES`] forbid. Stale entries are errors, so the list only shrinks.
pub const EXCEPTIONS: &[Exception] = &[
    Exception {
        from: "cupertino-settings",
        to: "cupertino-alsa",
        kind: Kind::Normal,
        tracking: Tracking::Issue(21),
        area: Area::Services,
        reason: SOUND_PANE,
    },
    Exception {
        from: "cupertino-settings",
        to: "cupertino-pipewire",
        kind: Kind::Normal,
        tracking: Tracking::Issue(21),
        area: Area::Services,
        reason: SOUND_PANE,
    },
    Exception {
        from: "cupertino-settings",
        to: "cupertino-pipewire",
        kind: Kind::Dev,
        tracking: Tracking::Issue(21),
        area: Area::Services,
        reason: SOUND_PANE,
    },
    Exception {
        from: "cupertino-menubar",
        to: "cupertino-files",
        kind: Kind::Dev,
        tracking: Tracking::Permanent,
        area: Area::Apps,
        reason: APP_MENUS,
    },
    Exception {
        from: "cupertino-menubar",
        to: "cupertino-settings",
        kind: Kind::Dev,
        tracking: Tracking::Permanent,
        area: Area::Apps,
        reason: APP_MENUS,
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
    /// An MPL-2.0 crate with a normal dependency on a crate under a GPL licence: `(from, to, licence of to)`.
    GplDependency(String, String, String),
    /// An MPL-2.0 crate without the licence text in a `LICENSE` file next to its manifest.
    MissingLicenceText(String),
    /// A crate that must be in the workspace under MPL-2.0 is not there.
    MissingMplRoot(String),
    /// A crate that must be under MPL-2.0 is not: `(crate, its licence)`.
    NonMplRoot(String, String),
    /// A crate in the normal or build closure of an MPL-2.0 workspace crate, external ones included, whose licence is
    /// not accepted for it.
    UnapprovedLicence {
        root: String,
        package: String,
        version: String,
        licence: String,
    },
}

impl Problem {
    /// A stable snake_case name of the kind of problem.
    pub fn kind(&self) -> &'static str {
        match self {
            Problem::Forbidden(_) => "forbidden_edge",
            Problem::DeniedPath(_) => "denied_path",
            Problem::StaleException(_) => "stale_exception",
            Problem::GplDependency(..) => "gpl_dependency",
            Problem::MissingLicenceText(_) => "missing_licence_text",
            Problem::MissingMplRoot(_) => "missing_mpl_root",
            Problem::NonMplRoot(..) => "non_mpl_root",
            Problem::UnapprovedLicence { .. } => "unapproved_licence",
        }
    }

    /// The crate the problem starts at.
    pub fn from(&self) -> &str {
        match self {
            Problem::Forbidden(e) => &e.from,
            Problem::DeniedPath(path) => path.first().map_or("", String::as_str),
            Problem::StaleException(x) => x.from,
            Problem::GplDependency(from, ..) => from,
            Problem::MissingLicenceText(name)
            | Problem::MissingMplRoot(name)
            | Problem::NonMplRoot(name, _) => name,
            Problem::UnapprovedLicence { root, .. } => root,
        }
    }

    /// The crate it leads to, when it is about a dependency.
    pub fn to(&self) -> Option<&str> {
        match self {
            Problem::Forbidden(e) => Some(&e.to),
            Problem::DeniedPath(path) => path.last().map(String::as_str),
            Problem::StaleException(x) => Some(x.to),
            Problem::GplDependency(_, to, _) => Some(to),
            Problem::UnapprovedLicence { package, .. } => Some(package),
            Problem::MissingLicenceText(_)
            | Problem::MissingMplRoot(_)
            | Problem::NonMplRoot(..) => None,
        }
    }
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
            Problem::GplDependency(from, to, licence) => write!(
                f,
                "{from} is MPL-2.0 and must not depend on {to} ({licence}), normal dependency"
            ),
            Problem::MissingLicenceText(name) => {
                write!(
                    f,
                    "{name} is MPL-2.0 and has no LICENSE with the MPL-2.0 text"
                )
            }
            Problem::MissingMplRoot(name) => {
                write!(f, "required MPL root {name} is missing from workspace")
            }
            Problem::NonMplRoot(name, licence) => {
                write!(f, "required MPL root {name} has non-MPL licence {licence}")
            }
            Problem::UnapprovedLicence {
                root,
                package,
                version,
                licence,
            } => write!(
                f,
                "{root} MPL closure: {package} {version} has unapproved licence {licence}"
            ),
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

/// Whether the SPDX expression `licence` names `MPL-2.0` first: the crate's own code is under the MPL (a bundled font
/// or similar may add terms with `AND`).
pub fn is_mpl(licence: &str) -> bool {
    licence.split_whitespace().next() == Some("MPL-2.0")
}

/// Whether the SPDX expression `licence` names a GPL or AGPL licence (LGPL is not one).
pub fn is_gpl(licence: &str) -> bool {
    licence
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '+')))
        .any(|id| id.starts_with("GPL-") || id.starts_with("AGPL-"))
}

/// A problem for each normal dependency of an MPL-2.0 workspace crate on a workspace crate under a GPL licence, and for
/// each MPL-2.0 crate for which `has_text` finds no licence text. `licences` is the SPDX expression of each workspace
/// crate; build and dev dependencies are not linked into the crate and do not count.
pub fn licence_problems(
    licences: &BTreeMap<String, String>,
    deps: &BTreeSet<Dep>,
    has_text: impl Fn(&str) -> bool,
) -> Vec<Problem> {
    let mut out: Vec<Problem> = deps
        .iter()
        .filter(|(_, _, kind)| *kind == Kind::Normal)
        .filter_map(|(from, to, _)| {
            let (from_licence, to_licence) = (licences.get(from)?, licences.get(to)?);
            (is_mpl(from_licence) && is_gpl(to_licence))
                .then(|| Problem::GplDependency(from.clone(), to.clone(), to_licence.clone()))
        })
        .collect();
    out.extend(
        licences
            .iter()
            .filter(|(name, licence)| is_mpl(licence) && !has_text(name))
            .map(|(name, _)| Problem::MissingLicenceText(name.clone())),
    );
    out
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
    fn a_shell_may_use_the_toolkit_and_apps_stay_off_the_renderer() {
        let toolkit = edge("dock", Shell, "kit", Ui);
        let renderer = edge("files", Apps, "glass", Render);
        assert_eq!(
            check(RULES, &[toolkit, renderer.clone()], &[], DENIED_PATHS),
            vec![Problem::Forbidden(renderer.clone())]
        );
        assert_eq!(
            Problem::Forbidden(renderer).to_string(),
            "files (apps) must not depend on glass (render), normal dependency"
        );
    }

    #[test]
    fn a_toolkit_crate_may_not_depend_on_the_server() {
        let up = edge("kit", Ui, "server", Core);
        assert_eq!(
            check(RULES, std::slice::from_ref(&up), &[], DENIED_PATHS),
            vec![
                Problem::Forbidden(up),
                Problem::DeniedPath(vec!["kit".into(), "server".into()])
            ]
        );
    }

    #[test]
    fn a_toolkit_chain_to_the_server_fails_even_when_its_edges_are_excused() {
        let up = edge("net", Transport, "server", Core);
        let edges = [edge("kit", Ui, "net", Transport), up.clone()];
        let problems = check(
            RULES,
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
        assert_eq!(check(RULES, std::slice::from_ref(&dev), &[x], &[]), vec![]);
        assert_eq!(
            check(RULES, std::slice::from_ref(&normal), &[x], &[]),
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
    fn layer_and_kind_names_parse() {
        assert_eq!(Layer::from_name("transport"), Some(Transport));
        assert_eq!(Layer::from_name("widgets"), None);
        assert_eq!(Kind::from_metadata(None), Some(Kind::Normal));
        assert_eq!(Kind::from_metadata(Some("dev")), Some(Kind::Dev));
        assert_eq!(Kind::from_metadata(Some("proc")), None);
    }

    fn licences(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(n, l)| (n.to_string(), l.to_string()))
            .collect()
    }

    fn dep(from: &str, to: &str, kind: Kind) -> Dep {
        (from.into(), to.into(), kind)
    }

    #[test]
    fn an_mpl_crate_may_not_link_a_gpl_crate() {
        let licences = licences(&[
            ("kit", "MPL-2.0"),
            ("font", "MPL-2.0 AND OFL-1.1"),
            ("shell", "GPL-3.0-or-later"),
            ("data", "GPL-3.0-or-later AND OFL-1.1"),
            ("lib", "MPL-2.0"),
        ]);
        let deps = BTreeSet::from([
            dep("kit", "lib", Kind::Normal),
            dep("kit", "shell", Kind::Normal),
            dep("font", "data", Kind::Normal),
            dep("kit", "serde", Kind::Normal),
            dep("shell", "kit", Kind::Normal),
        ]);
        assert_eq!(
            licence_problems(&licences, &deps, |_| true),
            vec![
                Problem::GplDependency(
                    "font".into(),
                    "data".into(),
                    "GPL-3.0-or-later AND OFL-1.1".into()
                ),
                Problem::GplDependency("kit".into(), "shell".into(), "GPL-3.0-or-later".into()),
            ]
        );
    }

    #[test]
    fn build_and_dev_dependencies_and_lgpl_do_not_count() {
        let licences = licences(&[
            ("kit", "MPL-2.0"),
            ("shell", "GPL-3.0-or-later"),
            ("lgpl", "LGPL-2.1-or-later"),
        ]);
        let deps = BTreeSet::from([
            dep("kit", "shell", Kind::Dev),
            dep("kit", "shell", Kind::Build),
            dep("kit", "lgpl", Kind::Normal),
        ]);
        assert_eq!(licence_problems(&licences, &deps, |_| true), vec![]);
    }

    #[test]
    fn an_mpl_crate_needs_its_licence_text() {
        let licences = licences(&[("kit", "MPL-2.0"), ("shell", "GPL-3.0-or-later")]);
        assert_eq!(
            licence_problems(&licences, &BTreeSet::new(), |name| name != "kit"),
            vec![Problem::MissingLicenceText("kit".into())]
        );
    }

    #[test]
    fn mpl_means_the_first_term_of_the_expression() {
        assert!(is_mpl("MPL-2.0 AND OFL-1.1"));
        assert!(!is_mpl("GPL-3.0-or-later AND OFL-1.1"));
        assert!(!is_mpl("MIT OR MPL-2.0"));
        assert!(is_gpl("(GPL-3.0-or-later)"));
        assert!(is_gpl("AGPL-3.0-only"));
        assert!(!is_gpl("LGPL-2.1-or-later"));
    }

    #[test]
    fn the_scene_the_seam_and_their_policy_models_stay_off_the_renderer_and_the_protocols() {
        for krate in [
            "cupertino-scene",
            "cupertino-render-api",
            "cupertino-effects",
            "cupertino-wm",
            "cupertino-stage",
        ] {
            let denied = DENIED_CRATES
                .iter()
                .find(|(c, _)| *c == krate)
                .unwrap_or_else(|| panic!("{krate} has no denied crates"))
                .1;
            for forbidden in [
                "smithay",
                "wgpu",
                "wayland-server",
                "cupertino-glass",
                "cupertino-server",
            ] {
                assert!(denied.contains(&forbidden), "{krate} may reach {forbidden}");
            }
        }
    }

    #[test]
    fn the_toolkit_renderer_cannot_reach_a_window_system_to_read_the_screen_back() {
        let denied = DENIED_CRATES
            .iter()
            .find(|(c, _)| *c == "cupertino-ui-render")
            .expect("cupertino-ui-render has denied crates")
            .1;
        for forbidden in [
            "cupertino-ui-wayland",
            "cupertino-wayland-client",
            "cupertino-server",
            "smithay",
            "wayland-client",
            "wayland-server",
            "pipewire",
        ] {
            assert!(
                denied.contains(&forbidden),
                "cupertino-ui-render may reach {forbidden}"
            );
        }
    }
}
