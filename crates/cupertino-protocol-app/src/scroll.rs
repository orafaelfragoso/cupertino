//! `cupertino_scroll_v1`: the scroll count of the pointer's scrolls.

use crate::{AppEvent, Quiet};
use cupertino_app_protocols::client::scroll::cupertino_pointer_scroll_v1::{
    self, CupertinoPointerScrollV1,
};
use cupertino_app_protocols::client::scroll::cupertino_scroll_v1::CupertinoScrollV1;
use cupertino_wayland_client::{Connection, Error, State};
use wayland_client::protocol::wl_pointer::{Axis, WlPointer};
use wayland_client::{Connection as WlConnection, Dispatch, QueueHandle, WEnum};

/// The scroll global (`cupertino_scroll_v1`).
pub struct ScrollCounts(CupertinoScrollV1);

impl ScrollCounts {
    /// Bind the global; `None` when the server does not offer it.
    pub fn bind(conn: &mut Connection) -> Option<ScrollCounts> {
        conn.bind_extension(1..=1, Quiet).map(ScrollCounts)
    }

    /// Follow the scroll counts of `pointer`: [`AppEvent::ScrollCount`] arrives before each frame's scroll while the
    /// result lives.
    pub fn watch(&self, conn: &Connection, pointer: &WlPointer) -> PointerScroll {
        PointerScroll(self.0.get_scroll(pointer, &conn.queue_handle(), Quiet))
    }
}

/// The scroll counts of one pointer; dropping it stops them.
pub struct PointerScroll(CupertinoPointerScrollV1);

impl Drop for PointerScroll {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

impl Dispatch<CupertinoPointerScrollV1, Quiet> for State {
    fn event(
        state: &mut State,
        _: &CupertinoPointerScrollV1,
        event: cupertino_pointer_scroll_v1::Event,
        _: &Quiet,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
        match event {
            cupertino_pointer_scroll_v1::Event::ScrollCount {
                axis: WEnum::Value(axis),
                count,
            } => state.emit(AppEvent::ScrollCount {
                vertical: axis == Axis::VerticalScroll,
                count,
            }),
            cupertino_pointer_scroll_v1::Event::ScrollCount { .. } => {
                state.fail(Error::Protocol("scroll count of an unknown axis"))
            }
            _ => {}
        }
    }
}
