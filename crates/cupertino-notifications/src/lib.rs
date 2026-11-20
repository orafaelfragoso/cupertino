//! The banners and the notification list of the desktop, a component of the shell host ([`Notifications`]).
//!
//! * [`center`]: the pure model (banner slot and waiting slot, notification centre, grouping,
//!   do-not-disturb, hover hold, swipe-to-dismiss). No threads, no clocks.
//! * [`component`]: the component: its windows' layer surfaces, the bus, the host bus and the loop's timers.
//! * [`control`]: `org.freedesktop.Notifications` and `org.cupertino.NotificationCenter1` on the component's own
//!   session bus connection.
//! * [`store`]: the file the list is kept in across a restart.
//! * [`markup`]: the body markup subset of the notification specification.
//! * [`notification`]: data types of a notification and of the `Notify` arguments.
//! * [`stack`]: geometry of a collapsed group in the notification centre.
//! * [`view`]: the banners and the notification centre as CupertinoKit windows, and the user's input on them.
//!
//! Colours, typography and motion come from the theme through CupertinoKit.

#![allow(clippy::chunks_exact_to_as_chunks)]
#![forbid(unsafe_code)]

pub mod center;
pub mod component;
pub mod control;
pub mod focus;
pub mod markup;
pub mod notification;
pub mod stack;
pub mod store;
pub mod view;

pub use center::{
    ActivateOutcome, BannerMeasure, CenterConfig, ClearScope, DndMode, Event, FixedHeight, Group,
    Motion, NotificationCenter, Placement, StackLayout, SwipeConfig, SwipeOutcome, TimeoutPolicy,
    Wakeup,
};
pub use component::Notifications;
pub use notification::{
    Action, CloseReason, Expire, Hints, ImageData, Notification, NotifyInput, Urgency,
    DEFAULT_ACTION_KEY,
};
pub use stack::{GroupStack, StackCard};
