//! Notification daemon model for a Wayland compositor.
//!
//! * [`center`]: the pure model (banner slot and waiting slot, notification centre, grouping,
//!   do-not-disturb, hover hold, swipe-to-dismiss). No threads, no clocks.
//! * [`markup`]: the body markup subset of the notification specification.
//! * [`notification`]: data types of a notification and of the `Notify` arguments.
//! * [`platter`]: material and shadow parameters of the banner platter.
//! * [`render`]: banner layout and CPU rendering with `tiny-skia` and `cosmic-text`.
//! * [`stack`]: geometry of a collapsed group in the notification centre.
//! * [`surface`]: layout, input region and press routing of the layer surface of the `cupertino-notifications` binary.
//! * `dbus` (feature `dbus`): the `org.freedesktop.Notifications` server on `zbus`.
//!
//! Colours and typography come from `cupertino-theme`.

#![allow(clippy::chunks_exact_to_as_chunks)]

pub mod center;
pub mod focus;
#[cfg(feature = "dbus")]
pub mod dbus;
pub mod markup;
pub mod notification;
pub mod platter;
pub mod render;
pub mod stack;
pub mod surface;

pub use center::{
    ActivateOutcome, BannerMeasure, CenterConfig, ClearScope, DndMode, Event, FixedHeight, Group, Motion, NotificationCenter, Placement, StackLayout, SwipeConfig, SwipeOutcome, TimeoutPolicy, Wakeup,
};
pub use stack::{GroupStack, StackCard};
pub use notification::{Action, CloseReason, Expire, Hints, ImageData, Notification, NotifyInput, Urgency, DEFAULT_ACTION_KEY};
