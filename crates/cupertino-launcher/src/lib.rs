//! Launcher (search panel) for a Wayland compositor.
//!
//! * [`desktop`], [`exec`]: desktop entry indexing and `Exec` handling (the shared `cupertino-apps`).
//! * [`matcher`]: tiered fuzzy matching; [`frecency`]: persisted usage ranking.
//! * [`calc`]: the calculator; [`files`]: bounded file search.
//! * [`engine`]: providers, sections and activation (returns a [`CommandSpec`] for the host to run).
//! * [`panel`]: panel state with keyboard navigation.
//! * [`view`]: the panel's windows on CupertinoKit; [`kit`]: the launcher as a Kit application model, with what
//!   needs the session behind [`kit::Host`].
//! * feature `app`, the parts of the `cupertino-launcher` process (the panel on a layer surface as a
//!   client of the server): [`control`] (the `org.cupertino.Launcher1` bus interface).
//!
//! No Smithay dependency. Panel metrics, keyboard semantics, the top-hit rule and the section limits are taken from
//! the analysis; ranking scores and the remaining row metrics are approximate (authored) and configurable; colours,
//! type, material and controls come from CupertinoKit's theme.

pub mod browse;
pub mod calc;
#[cfg(feature = "app")]
pub mod control;
pub mod convert;
pub use cupertino_apps::entry as desktop;
pub mod engine;
pub use cupertino_apps::exec;
pub mod files;
pub mod frecency;
pub mod kit;
pub mod matcher;
pub mod modes;
pub mod motion;
pub mod nav;
pub mod panel;
pub mod preview;
pub mod providers;
pub mod refine;
pub mod scope;
pub mod view;

pub use desktop::{DesktopAction, DesktopEntry};
pub use engine::{
    Action, Activation, ItemKind, LaunchRequest, Launcher, LauncherConfig, ResultItem, Results,
    Section, SectionKind,
};
pub use exec::CommandSpec;
pub use frecency::Frecency;
