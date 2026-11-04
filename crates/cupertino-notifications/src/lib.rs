//! Notification daemon model for a Wayland compositor.
//!
//! * [`center`]: the pure model (banner slot and waiting slot, notification centre, grouping,
//!   do-not-disturb, hover hold, swipe-to-dismiss). No threads, no clocks.
//! * [`markup`]: the body markup subset of the notification specification.
//! * [`notification`]: data types of a notification and of the `Notify` arguments.
//! * [`stack`]: geometry of a collapsed group in the notification centre.
//! * [`view`]: the banners and the notification centre as CupertinoKit windows, and the user's input on them.
//! * `dbus` (feature `dbus`): the `org.freedesktop.Notifications` server on `zbus`.
//!
//! Colours and typography come from `cupertino-theme`.

#![allow(clippy::chunks_exact_to_as_chunks)]

pub mod center;
#[cfg(feature = "dbus")]
pub mod dbus;
pub mod focus;
pub mod markup;
pub mod notification;
pub mod stack;
pub mod view;

pub use center::{
    ActivateOutcome, BannerMeasure, CenterConfig, ClearScope, DndMode, Event, FixedHeight, Group,
    Motion, NotificationCenter, Placement, StackLayout, SwipeConfig, SwipeOutcome, TimeoutPolicy,
    Wakeup,
};
pub use notification::{
    Action, CloseReason, Expire, Hints, ImageData, Notification, NotifyInput, Urgency,
    DEFAULT_ACTION_KEY,
};
pub use stack::{GroupStack, StackCard};
