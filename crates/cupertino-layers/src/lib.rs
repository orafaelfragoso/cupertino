//! Layer rules of the workspace. A crate declares its layer in its manifest
//! (`[package.metadata.cupertino] layer = "<layer>"`). [`RULES`] says which layers each layer may depend on; the
//! further rules are checks of their own: no layer reaches up through a chain of dependencies, `foundation` and
//! `models` sources stay pure, `system` crates start no threads ([`THREADLESS_LAYERS`]), components stay apart, the window-system boundaries ([`WINDOW_SYSTEM`]) hold, `shell`
//! and `apps` crates reach the toolkit through its facades ([`FACADES`]), external crates stay in their layers
//! ([`EXTERNAL_CRATES`]), every crate declares a licence, only dev-dependencies turn on the toolkit's test feature
//! ([`TEST_FEATURE`]) and the alias that builds the programs names every other feature ([`PROGRAMS_ALIAS`]).
//!
//! Problems that today's crates still have are listed in the committed remaining-work file ([`REMAINING`]): the check
//! fails on a problem the file does not list and on a line that is no longer a problem, and the lines of the layers in
//! [`ENFORCED`] must be absent. [`check_workspace`] runs every check on a checkout, as the `cupertino-layers` binary
//! and `cupertino dev layers` do.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

mod clippy;
mod graph;
mod sources;
mod workspace;

pub use clippy::{clippy_toml, DISALLOWED_METHODS};
pub use graph::{crate_paths, dot, reaches_up, without_counts, GRAPH_SNAPSHOT};
pub use workspace::{check_workspace, check_workspace_with, Findings, LayersError, Policy, POLICY};

/// The committed remaining-work file, relative to the workspace root.
pub const REMAINING: &str = "crates/cupertino-layers/remaining.txt";

/// The layers, from the bottom. The order is the height: a crate reaches up when it reaches a later layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    /// Values, maths and data models with no I/O.
    Foundation,
    /// Drawing maths shared by every renderer, and the GPU device.
    Graphics,
    /// Desktop policy as pure logic.
    Models,
    /// User-space operating-system access: settings, application database, icons, trash, diagnostics, bus callers.
    System,
    /// Kernel interfaces that more than one program needs: devices and their input events, the seat.
    Platform,
    /// Wire schemas: generated Wayland types, versioned IPC messages.
    Protocol,
    /// Client connections without widgets.
    Transport,
    /// The compositor's own drawing: materials, decorations, cursors.
    Render,
    /// The application toolkit.
    Ui,
    /// The shell's extension of the toolkit.
    Shellkit,
    /// The compositor.
    Core,
    /// Processes with no interface of their own.
    Services,
    /// The shell host, its components and the separate toolkit programs of the shell.
    Shell,
    /// Applications.
    Apps,
    /// Developer and theming tools.
    Tools,
    /// Test harnesses and tests that start programs from more than one crate.
    Testing,
}

use Layer::*;

impl Layer {
    pub const ALL: [Layer; 16] = [
        Foundation, Graphics, Models, System, Platform, Protocol, Transport, Render, Ui, Shellkit,
        Core, Services, Shell, Apps, Tools, Testing,
    ];

    /// The value of `package.metadata.cupertino.layer`.
    pub fn name(self) -> &'static str {
        match self {
            Foundation => "foundation",
            Graphics => "graphics",
            Models => "models",
            System => "system",
            Platform => "platform",
            Protocol => "protocol",
            Transport => "transport",
            Render => "render",
            Ui => "ui",
            Shellkit => "shellkit",
            Core => "core",
            Services => "services",
            Shell => "shell",
            Apps => "apps",
            Tools => "tools",
            Testing => "testing",
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

/// The layers each layer may depend on, in normal, build and dev dependencies. A crate may only declare a layer that
/// has a row. Besides the rows, every crate may have `testing` crates as dev-dependencies, and only dev-dependencies
/// point at a `testing` crate.
pub type Rules = &'static [(Layer, &'static [Layer])];

/// The layer table.
pub const RULES: Rules = &[
    (Foundation, &[Foundation]),
    (Graphics, &[Foundation, Graphics]),
    (Models, &[Foundation, Models]),
    (System, &[Foundation, System]),
    (Platform, &[Foundation, Platform]),
    (Protocol, &[Foundation, Protocol]),
    (
        Transport,
        &[Foundation, System, Platform, Protocol, Transport],
    ),
    (Render, &[Foundation, Graphics, System, Platform, Render]),
    (Ui, &[Foundation, Graphics, System, Protocol, Transport, Ui]),
    (Shellkit, &[Foundation, System, Protocol, Transport, Ui]),
    (
        Core,
        &[
            Foundation, Graphics, Models, System, Platform, Protocol, Render, Core,
        ],
    ),
    (
        Services,
        &[
            Foundation, Models, System, Platform, Protocol, Transport, Services,
        ],
    ),
    (
        Shell,
        &[Foundation, Models, System, Protocol, Ui, Shellkit, Shell],
    ),
    (Apps, &[Foundation, Models, System, Protocol, Ui]),
    (
        Tools,
        &[
            Foundation, Graphics, Models, System, Platform, Protocol, Transport, Render, Ui,
            Shellkit, Tools,
        ],
    ),
    (
        Testing,
        &[
            Foundation, Graphics, Models, System, Platform, Protocol, Transport, Render, Ui,
            Shellkit, Core, Services, Shell, Apps, Tools, Testing,
        ],
    ),
];

/// The closed layers: none of their problems may be listed in the remaining-work file.
pub const ENFORCED: &[Layer] = &[
    Layer::Foundation,
    Layer::Graphics,
    Layer::Models,
    Layer::System,
    Layer::Platform,
    Layer::Protocol,
    Layer::Transport,
    Layer::Render,
    Layer::Ui,
    Layer::Shellkit,
    Layer::Core,
    Layer::Services,
    Layer::Testing,
];

/// Allowed dependencies among the `core` server crates (D25): `cupertino-server-backend` and
/// `cupertino-server-input` depend on no other `core` crate; `cupertino-server-render` depends
/// only on `cupertino-server-backend`; `cupertino-server` depends on all three.
pub const CORE_DIRECTION: &[(&str, &[&str])] = &[
    ("cupertino-server-backend", &[]),
    ("cupertino-server-input", &[]),
    ("cupertino-server-render", &["cupertino-server-backend"]),
    (
        "cupertino-server",
        &[
            "cupertino-server-backend",
            "cupertino-server-render",
            "cupertino-server-input",
        ],
    ),
];

/// The layers whose crates start no threads in their `src/`: they offer blocking functions, which callers run as
/// tasks.
pub const THREADLESS_LAYERS: &[Layer] = &[System];

/// The layers whose sources must stay pure: no I/O, no threads, no `unsafe`, no FFI.
pub const PURE_LAYERS: &[Layer] = &[Foundation, Models];

/// The standard library modules a pure source must not use.
pub const IMPURE: &[&str] = &[
    "std::fs",
    "std::net",
    "std::process",
    "std::env",
    "std::thread",
];

/// The shell host, the one `shell` crate that may depend on other `shell` crates: the components it runs.
pub const SHELL_HOST: &str = "cupertino-shell";

/// The `shell` crates that are programs of their own, never components of the host.
pub const SEPARATE_PROGRAMS: &[&str] = &[
    "cupertino-login",
    "cupertino-prompts",
    "cupertino-screenshot",
];

/// Crates that must not reach these crates through any chain of normal dependencies, external crates included. A name
/// ending in `*` stands for every crate it starts. The public transport speaks standard and the public Cupertino protocols only, the public
/// protocol crate stays off the private protocols, the toolkit stays off the private protocols and the privileged shell
/// client, and the toolkit's renderer stays off the window system.
pub const WINDOW_SYSTEM: &[(&str, &[&str])] = &[
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
        "cupertino-kit",
        &["cupertino-protocols", "cupertino-shell-client"],
    ),
    (
        "cupertino-ui-*",
        &["cupertino-protocols", "cupertino-shell-client"],
    ),
    (
        "cupertino-ui-render",
        &["cupertino-ui-wayland", "cupertino-wayland-client"],
    ),
];

/// A facade: the crates that a crate of `layers` never depends on, in any kind of dependency, and reaches through
/// `facade` instead.
#[derive(Clone, Copy, Debug)]
pub struct Facade {
    pub hidden: &'static [&'static str],
    pub facade: &'static str,
    pub layers: &'static [Layer],
}

/// The toolkit's facades.
pub const FACADES: &[Facade] = &[
    Facade {
        hidden: &[
            "cupertino-ui-core",
            "cupertino-ui-controls",
            "cupertino-ui-files",
            "cupertino-ui-text",
            "cupertino-ui-render",
            "cupertino-ui-wayland",
            "cupertino-ui-test",
            "cupertino-ui-abi",
            "cupertino-theme",
            "cupertino-animation",
            "cupertino-raster-math",
            "cupertino-gpu",
            "cupertino-wayland-client",
            "cupertino-app-protocols",
        ],
        facade: "cupertino-kit",
        layers: &[Shell, Apps],
    },
    Facade {
        hidden: &["cupertino-shell-client", "cupertino-protocols"],
        facade: "cupertino-kit-shell",
        layers: &[Shell, Apps],
    },
];

/// The toolkit's feature for program tests, as package and feature: only a dev-dependency may turn it on, never a
/// normal or build dependency or a feature, so no program is built with it.
pub const TEST_FEATURE: (&str, &str) = ("cupertino-kit", "test");

/// The Cargo alias in the workspace's `.cargo/config.toml` that builds the programs as they are packaged: it names
/// every feature of the workspace's packages with `--features`, `default` aside (on without being named), except
/// [`TEST_FEATURE`].
pub const PROGRAMS_ALIAS: &str = "build-programs";

/// External crates that only some layers may depend on directly. A name ending in `*` stands for every crate it
/// starts.
#[derive(Clone, Copy, Debug)]
pub struct External {
    pub crates: &'static [&'static str],
    /// The layers that may depend on them in any kind of dependency.
    pub layers: &'static [Layer],
    /// The layers that may depend on them as dev-dependencies only.
    pub dev_layers: &'static [Layer],
}

/// Where the external crates that carry a layer's concerns may be used.
pub const EXTERNAL_CRATES: &[External] = &[
    External {
        crates: &["smithay", "smithay-*"],
        layers: &[Core],
        dev_layers: &[],
    },
    External {
        crates: &["tiny-skia", "resvg", "usvg", "image", "png"],
        layers: &[Graphics, System, Render, Ui, Core, Services, Tools, Testing],
        dev_layers: &[Models],
    },
    External {
        crates: &[
            "wayland-client",
            "wayland-backend",
            "wayland-protocols*",
            "wayland-scanner",
        ],
        layers: &[
            Protocol, Transport, Ui, Shellkit, Core, Services, Tools, Testing,
        ],
        dev_layers: &[],
    },
    External {
        crates: &["xkbcommon"],
        layers: &[Transport, Ui, Core, Services, Tools, Testing],
        dev_layers: &[],
    },
    External {
        crates: &["wayland-server"],
        layers: &[Protocol, Core, Testing],
        dev_layers: &Layer::ALL,
    },
    External {
        crates: &["zbus"],
        layers: &[
            System, Platform, Ui, Shellkit, Core, Services, Shell, Apps, Tools, Testing,
        ],
        dev_layers: &[],
    },
    External {
        crates: &["libc", "rustix", "nix", "inotify", "notify"],
        layers: &[
            Graphics, System, Platform, Protocol, Transport, Render, Ui, Shellkit, Core, Services,
            Shell, Apps, Tools, Testing,
        ],
        dev_layers: &[],
    },
];

/// Whether `name` is `pattern`, or starts with it when `pattern` ends in `*`.
pub fn matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pattern == name,
    }
}

/// Whether `rules` let a crate of layer `from` depend on a crate of layer `to`.
pub fn allows(rules: Rules, from: Layer, to: Layer) -> bool {
    rules
        .iter()
        .any(|(l, to_layers)| *l == from && to_layers.contains(&to))
}

/// Whether `rules` allow `edge`: a `testing` crate only as a dev-dependency, which every crate may have, and any other
/// crate as its row says.
pub fn allows_edge(rules: Rules, edge: &Edge) -> bool {
    if edge.to_layer == Testing {
        edge.kind == Kind::Dev
    } else {
        allows(rules, edge.from_layer, edge.to_layer)
    }
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// A dependency the layer table does not allow.
    Forbidden(Edge),
    /// A crate that reaches a crate of a higher layer through a chain of dependencies, as crate names from the crate.
    ReachesUp {
        path: Vec<String>,
        from_layer: Layer,
        to_layer: Layer,
    },
    /// A `shell` crate that depends on another `shell` crate, other than the host on a component.
    ComponentsApart(Edge),
    /// A `shell` or `apps` crate that depends on a crate it reaches through `facade`.
    Facade { edge: Edge, facade: &'static str },
    /// A chain of normal dependencies across a window-system boundary, as crate names from the crate.
    WindowSystem {
        from_layer: Layer,
        path: Vec<String>,
    },
    /// A direct dependency on an external crate that the crate's layer may not use in that kind of dependency.
    ExternalCrate {
        from: String,
        from_layer: Layer,
        to: String,
        kind: Kind,
    },
    /// A source of a pure layer that uses `what`: a standard library module, `unsafe` or `ffi`. `file` is relative to
    /// the crate's directory.
    Impure {
        krate: String,
        layer: Layer,
        file: String,
        what: &'static str,
    },
    /// A source in the `src/` of a crate of a threadless layer that starts a thread with `call`. `file` is relative to
    /// the crate's directory.
    StartsThread {
        krate: String,
        layer: Layer,
        file: String,
        call: &'static str,
    },
    /// A source in the `src/` of a crate other than the shell host (and `cupertino-kit-shell` itself) that references
    /// `cupertino_kit_shell::hosting`. `file` is relative to the crate's directory.
    HostingApi {
        krate: String,
        layer: Layer,
        file: String,
    },
    /// A crate whose `license` is missing or no licence expression.
    UndeclaredLicence {
        krate: String,
        layer: Layer,
        licence: String,
    },
    /// A line of the remaining-work file that is no longer a problem.
    StaleRemaining(String),
    /// A line of the remaining-work file for a layer in [`ENFORCED`].
    EnforcedRemaining(String),
    /// The committed DOT snapshot of the layers, at this path relative to the workspace root, no longer matches the
    /// workspace.
    StaleGraph(String),
    /// A `shell` or `apps` crate whose `clippy.toml` is missing or differs from the list of disallowed methods.
    DisallowedMethods { krate: String, layer: Layer },
    /// A normal or build dependency, or a feature, of `krate` that turns on the test feature (`feature`, as
    /// `package/feature`); `how` names it.
    TestFeature {
        krate: String,
        feature: String,
        how: String,
    },
    /// The alias that builds the programs (`alias`) in the workspace's `.cargo/config.toml` is missing, or does not
    /// name exactly the workspace's features but the test feature: those it leaves out, and those it names that are
    /// not among them (`--all-features` included).
    ProgramFeatures {
        alias: String,
        missing: Vec<String>,
        extra: Vec<String>,
    },
}

impl Problem {
    /// A stable snake_case name of the kind of problem.
    pub fn kind(&self) -> &'static str {
        match self {
            Problem::Forbidden(_) => "forbidden_edge",
            Problem::ReachesUp { .. } => "reaches_up",
            Problem::ComponentsApart(_) => "components_apart",
            Problem::Facade { .. } => "facade",
            Problem::WindowSystem { .. } => "window_system",
            Problem::ExternalCrate { .. } => "external_crate",
            Problem::Impure { .. } => "impure_source",
            Problem::StartsThread { .. } => "starts_thread",
            Problem::HostingApi { .. } => "hosting_api",
            Problem::UndeclaredLicence { .. } => "undeclared_licence",
            Problem::StaleRemaining(_) => "stale_remaining",
            Problem::EnforcedRemaining(_) => "enforced_remaining",
            Problem::StaleGraph(_) => "stale_graph",
            Problem::DisallowedMethods { .. } => "disallowed_methods",
            Problem::TestFeature { .. } => "test_feature",
            Problem::ProgramFeatures { .. } => "program_features",
        }
    }

    /// The crate the problem starts at.
    pub fn from(&self) -> &str {
        match self {
            Problem::Forbidden(e)
            | Problem::ComponentsApart(e)
            | Problem::Facade { edge: e, .. } => &e.from,
            Problem::ReachesUp { path, .. } | Problem::WindowSystem { path, .. } => {
                path.first().map_or("", String::as_str)
            }
            Problem::ExternalCrate { from, .. } => from,
            Problem::Impure { krate, .. }
            | Problem::StartsThread { krate, .. }
            | Problem::HostingApi { krate, .. }
            | Problem::UndeclaredLicence { krate, .. }
            | Problem::DisallowedMethods { krate, .. }
            | Problem::TestFeature { krate, .. } => krate,
            Problem::StaleRemaining(_)
            | Problem::EnforcedRemaining(_)
            | Problem::StaleGraph(_)
            | Problem::ProgramFeatures { .. } => "cupertino-layers",
        }
    }

    /// The crate it leads to, when it is about a dependency.
    pub fn to(&self) -> Option<&str> {
        match self {
            Problem::Forbidden(e)
            | Problem::ComponentsApart(e)
            | Problem::Facade { edge: e, .. } => Some(&e.to),
            Problem::ReachesUp { path, .. } | Problem::WindowSystem { path, .. } => {
                path.last().map(String::as_str)
            }
            Problem::ExternalCrate { to, .. } => Some(to),
            _ => None,
        }
    }

    /// The layer of the crate the problem starts at, for a problem of the rules.
    pub fn layer(&self) -> Option<Layer> {
        match self {
            Problem::Forbidden(e)
            | Problem::ComponentsApart(e)
            | Problem::Facade { edge: e, .. } => Some(e.from_layer),
            Problem::ReachesUp { from_layer, .. }
            | Problem::WindowSystem { from_layer, .. }
            | Problem::ExternalCrate { from_layer, .. } => Some(*from_layer),
            Problem::Impure { layer, .. }
            | Problem::StartsThread { layer, .. }
            | Problem::HostingApi { layer, .. }
            | Problem::UndeclaredLicence { layer, .. }
            | Problem::DisallowedMethods { layer, .. } => Some(*layer),
            _ => None,
        }
    }

    /// The problem's line in the remaining-work file, `<layer> <kind> <crate> …`, for a problem of the rules. It
    /// names only what the problem is about, so it changes only when the problem does.
    pub fn line(&self) -> Option<String> {
        let layer = self.layer()?;
        let kind = self.kind();
        let rest = match self {
            Problem::Forbidden(e)
            | Problem::ComponentsApart(e)
            | Problem::Facade { edge: e, .. } => {
                format!("{} -> {} {}", e.from, e.to, e.kind)
            }
            Problem::ReachesUp { .. } | Problem::WindowSystem { .. } => {
                format!("{} -> {}", self.from(), self.to().unwrap_or_default())
            }
            Problem::ExternalCrate { from, to, kind, .. } => format!("{from} -> {to} {kind}"),
            Problem::Impure {
                krate, file, what, ..
            } => format!("{krate} {file} {what}"),
            Problem::StartsThread {
                krate, file, call, ..
            } => format!("{krate} {file} {call}"),
            Problem::HostingApi { krate, file, .. } => format!("{krate} {file}"),
            Problem::UndeclaredLicence { krate, .. } => krate.clone(),
            Problem::DisallowedMethods { krate, .. } => format!("{krate} clippy.toml"),
            _ => return None,
        };
        Some(format!("{layer} {kind} {rest}"))
    }
}

const WRITE_REMAINING: &str = "cargo run -q -p cupertino-layers -- --write-remaining";

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Forbidden(e) => write!(
                f,
                "{} ({}) must not depend on {} ({}), {} dependency",
                e.from, e.from_layer, e.to, e.to_layer, e.kind
            ),
            Problem::ReachesUp {
                path,
                from_layer,
                to_layer,
            } => write!(
                f,
                "{} ({from_layer}) reaches up to {} ({to_layer}): {}",
                self.from(),
                self.to().unwrap_or_default(),
                path.join(" -> ")
            ),
            Problem::ComponentsApart(e) => write!(
                f,
                "{} must not depend on {}: shell crates stay apart, {} dependency",
                e.from, e.to, e.kind
            ),
            Problem::Facade { edge, facade } => write!(
                f,
                "{} ({}) must reach {} through {facade}, {} dependency",
                edge.from, edge.from_layer, edge.to, edge.kind
            ),
            Problem::WindowSystem { path, .. } => {
                write!(f, "window-system boundary crossed: {}", path.join(" -> "))
            }
            Problem::ExternalCrate {
                from,
                from_layer,
                to,
                kind,
            } => write!(
                f,
                "{from} ({from_layer}) must not use the external crate {to}, {kind} dependency"
            ),
            Problem::Impure {
                krate,
                layer,
                file,
                what,
            } => write!(f, "{krate} ({layer}) is pure and its {file} uses {what}"),
            Problem::StartsThread {
                krate,
                layer,
                file,
                call,
            } => write!(
                f,
                "{krate} ({layer}) starts a thread with {call} in {file}: offer the blocking function and let the \
                 caller run it as a task"
            ),
            Problem::HostingApi { krate, layer, file } => write!(
                f,
                "{krate} ({layer}) references cupertino_kit_shell::hosting in {file}: only {SHELL_HOST} may use \
                 the hosting API"
            ),
            Problem::UndeclaredLicence { krate, licence, .. } => {
                write!(f, "{krate} declares no licence expression (`{licence}`)")
            }
            Problem::StaleRemaining(line) => write!(
                f,
                "{REMAINING} lists `{line}`, which is no longer a problem: {WRITE_REMAINING}"
            ),
            Problem::EnforcedRemaining(line) => write!(
                f,
                "{REMAINING} lists `{line}` of an enforced layer: fix it instead"
            ),
            Problem::StaleGraph(path) => write!(
                f,
                "{path} is out of date: cargo run -q -p cupertino-layers -- --graph > {path}"
            ),
            Problem::DisallowedMethods { krate, layer } => write!(
                f,
                "{krate} ({layer}) has no clippy.toml with the disallowed methods: remove its banned calls, then \
                 cargo run -q -p cupertino-layers -- --write-clippy {krate}"
            ),
            Problem::TestFeature {
                krate,
                feature,
                how,
            } => write!(
                f,
                "{krate} turns on {feature} with {how}: only a dev-dependency may, so that no program is built with it"
            ),
            Problem::ProgramFeatures {
                alias,
                missing,
                extra,
            } => write!(
                f,
                ".cargo/config.toml: the alias {alias} must name every feature of the workspace's packages but the \
                 test feature, as --features=PACKAGE/FEATURE; it leaves out [{}] and names [{}]",
                missing.join(", "),
                extra.join(", ")
            ),
        }
    }
}

/// The problems of the dependency rules: every edge the table forbids, every chain that reaches up, every dependency
/// between `shell` crates other than the host's on a component, and every dependency on a crate behind a facade.
pub fn check(rules: Rules, edges: &[Edge], facades: &[Facade]) -> Vec<Problem> {
    let mut out: Vec<Problem> = edges
        .iter()
        .filter(|e| {
            !allows_edge(rules, e)
                || (e.from_layer == Core
                    && e.to_layer == Core
                    && CORE_DIRECTION
                        .iter()
                        .find(|(from, _)| *from == e.from)
                        .is_some_and(|(_, allowed)| !allowed.contains(&e.to.as_str())))
        })
        .cloned()
        .map(Problem::Forbidden)
        .collect();
    out.extend(reaches_up(edges));
    out.extend(
        edges
            .iter()
            .filter(|e| e.from_layer == Shell && e.to_layer == Shell)
            .filter(|e| e.from != SHELL_HOST || SEPARATE_PROGRAMS.contains(&e.to.as_str()))
            .cloned()
            .map(Problem::ComponentsApart),
    );
    for facade in facades {
        out.extend(
            edges
                .iter()
                .filter(|e| {
                    facade.layers.contains(&e.from_layer) && facade.hidden.contains(&e.to.as_str())
                })
                .map(|e| Problem::Facade {
                    edge: e.clone(),
                    facade: facade.facade,
                }),
        );
    }
    out
}

/// A problem for each direct dependency of a workspace crate on an external crate (one without a layer in `layers`)
/// that `externals` keep out of the crate's layer for that kind of dependency.
pub fn external_problems(
    layers: &BTreeMap<String, Layer>,
    deps: &BTreeSet<Dep>,
    externals: &[External],
) -> Vec<Problem> {
    deps.iter()
        .filter(|(_, to, _)| !layers.contains_key(to))
        .filter_map(|(from, to, kind)| {
            let from_layer = *layers.get(from)?;
            let rule = externals
                .iter()
                .find(|x| x.crates.iter().any(|p| matches(p, to)))?;
            let allowed = rule.layers.contains(&from_layer)
                || *kind == Kind::Dev && rule.dev_layers.contains(&from_layer);
            (!allowed).then(|| Problem::ExternalCrate {
                from: from.clone(),
                from_layer,
                to: to.clone(),
                kind: *kind,
            })
        })
        .collect()
}

/// Whether `licence` is a licence expression.
pub fn is_licence(licence: &str) -> bool {
    !licence.trim().is_empty()
        && spdx::Expression::parse_mode(licence, spdx::ParseMode::LAX).is_ok()
}
