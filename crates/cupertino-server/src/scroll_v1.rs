//! Server side of `cupertino_scroll_v1`: the scroll count of touchpad scrolls, which `wl_pointer`
//! cannot carry. `events.rs` computes the count per device and calls [`ScrollObservers::send`] between the
//! `wl_pointer.axis` events of a frame and its `wl_pointer.frame`.

use cupertino_protocols::server::scroll::cupertino_pointer_scroll_v1::{self, CupertinoPointerScrollV1};
use cupertino_protocols::server::scroll::cupertino_scroll_v1::{self, CupertinoScrollV1};
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::protocol::wl_pointer::{Axis, WlPointer};
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use crate::state::Cupertino;

/// The global and the scroll objects of every client, in creation order. Each is tied to the
/// `wl_pointer` it was created for.
pub struct ScrollObservers {
    _global: GlobalId,
    objects: Vec<CupertinoPointerScrollV1>,
}

impl ScrollObservers {
    pub fn new(dh: &DisplayHandle) -> Self {
        Self { _global: dh.create_global::<Cupertino, CupertinoScrollV1, ()>(1, ()), objects: Vec::new() }
    }

    /// Tell the scroll objects of `client` that the scroll of `axis` in the frame being built has `count`.
    pub fn send(&self, client: &ClientId, axis: Axis, count: u32) {
        for object in &self.objects {
            let live = object.data::<WlPointer>().is_some_and(Resource::is_alive);
            if live && object.client().is_some_and(|c| c.id() == *client) {
                object.scroll_count(axis, count);
            }
        }
    }
}

impl GlobalDispatch<CupertinoScrollV1, ()> for Cupertino {
    fn bind(_: &mut Self, _: &DisplayHandle, _: &Client, resource: New<CupertinoScrollV1>, _: &(), init: &mut DataInit<'_, Self>) {
        init.init(resource, ());
    }
}

impl Dispatch<CupertinoScrollV1, ()> for Cupertino {
    fn request(state: &mut Self, _: &Client, _: &CupertinoScrollV1, request: cupertino_scroll_v1::Request, _: &(), _: &DisplayHandle, init: &mut DataInit<'_, Self>) {
        match request {
            cupertino_scroll_v1::Request::GetScroll { id, pointer } => state.scroll_counts.objects.push(init.init(id, pointer)),
            cupertino_scroll_v1::Request::Destroy => {}
            _ => unreachable!("cupertino_scroll_v1 version 1 has no other request"),
        }
    }
}

impl Dispatch<CupertinoPointerScrollV1, WlPointer> for Cupertino {
    fn request(_: &mut Self, _: &Client, _: &CupertinoPointerScrollV1, request: cupertino_pointer_scroll_v1::Request, _: &WlPointer, _: &DisplayHandle, _: &mut DataInit<'_, Self>) {
        match request {
            cupertino_pointer_scroll_v1::Request::Destroy => {}
            _ => unreachable!("cupertino_pointer_scroll_v1 version 1 has no other request"),
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &CupertinoPointerScrollV1, _: &WlPointer) {
        state.scroll_counts.objects.retain(|o| o != resource);
    }
}
