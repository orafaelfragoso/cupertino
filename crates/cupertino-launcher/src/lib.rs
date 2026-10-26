//! Launcher (search panel) for a Wayland compositor.
//!
//! * [`desktop`], [`exec`]: desktop entry indexing and `Exec` handling (the shared `cupertino-apps`).
//! * [`matcher`]: tiered fuzzy matching; [`frecency`]: persisted usage ranking.
//! * [`calc`]: the calculator; [`files`]: bounded file search.
//! * [`engine`]: providers, sections and activation (returns a [`CommandSpec`] for the host to run).
//! * [`panel`]: panel state with keyboard navigation; [`render`]: panel layout and CPU rendering.
//! * feature `app`, the parts of the `cupertino-launcher` process (the panel on a layer surface as a
//!   client of the server): [`control`] (the `org.cupertino.Launcher1` bus interface) and [`surface`]
//!   (client key events to panel keys).
//!
//! No Smithay dependency. Panel metrics, material choice, keyboard semantics, the top-hit rule and the
//! section limits are taken from the analysis;
//! ranking scores and the remaining row metrics are approximate (authored) and configurable;
//! colours, typography, corner and shadow values come from `cupertino-theme`.

#![allow(clippy::chunks_exact_to_as_chunks)]

pub mod calc;
#[cfg(feature = "app")]
pub mod control;
pub use cupertino_apps::entry as desktop;
pub mod engine;
pub use cupertino_apps::exec;
pub mod files;
pub mod frecency;
pub mod matcher;
pub mod panel;
pub mod render;
#[cfg(feature = "app")]
pub mod surface;

pub use desktop::{DesktopAction, DesktopEntry};
pub use engine::{
    Action, Activation, ItemKind, LaunchRequest, Launcher, LauncherConfig, ResultItem, Results,
    Section, SectionKind,
};
pub use exec::CommandSpec;
pub use frecency::Frecency;
