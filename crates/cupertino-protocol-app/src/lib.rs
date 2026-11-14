//! The public protocols of the Cupertino server (`cupertino-app-protocols`) as extensions on the transport's queue
//! (`cupertino-wayland-client`), for any application.
//!
//! [`AppEffects`] (`cupertino_app_effects_v1`) observes the session's appearance and display options
//! ([`AppEffects::appearance`]), creates server-rendered [`Backdrop`]s, sets the title bar style of an
//! application window, announces where a window's menus are on the session bus ([`AppEffects::menu`]), for the
//! desktop's menu bar, and disables a window's minimize and zoom controls ([`AppEffects::set_disabled_controls`]).
//! A glass backdrop tells the tone of the face behind it ([`AppEvent::BackdropTone`], version 7), which picks the
//! colour of the text and symbols over a small shape.
//! [`ScrollCounts`] (`cupertino_scroll_v1`) reports how many scroll steps each pointer frame carries. [`Layers`]
//! (`cupertino_layers_v1`) hands the server [`Layer`]s to place and animate on its clock, and from version 2 the
//! presentation of a layer shell surface as a whole ([`Layers::presentation`]), so a panel opens and closes without a
//! frame drawn. What the objects
//! receive arrives as an [`AppEvent`] inside the transport's `Event::Extension`, in order with the transport's own
//! events.

mod backdrop;
mod effects;
mod error;
mod layers;
mod scroll;
mod semantics;
pub use semantics::{SemanticRequest, SemanticSurface, SemanticWake, Semantics};

pub use backdrop::{Backdrop, BackdropId, BackdropTone, GlassStyle};
pub use effects::{
    report_appearance, report_display_options, AppEffects, AppEvent, AppMenu, Appearance,
    MenuAddress, WindowControls, WindowStyle,
};
pub use error::Error;
pub use layers::{Fixed, Layer, LayerId, LayerProperty, Layers};
pub use scroll::{PointerScroll, ScrollCounts};

/// User data of the objects whose events carry nothing for the application.
pub(crate) struct Quiet;
