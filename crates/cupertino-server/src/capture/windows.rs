//! `cupertino_screenshot_v1`: the windows on screen, front to back, for the screenshot process's window pick.

use std::collections::HashMap;

use cupertino_protocols::server::screenshot::cupertino_screenshot_v1::{
    self, CupertinoScreenshotV1,
};
use cupertino_wm::{OutputId, WindowId};
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
};

use super::privileged;
use crate::render::motion::WorkspaceTransition;
use crate::render::scene::{plan_for, windows_on_output};
use crate::state::Cupertino;

/// One window as the feed describes it: foreign-toplevel identifier and frame in global logical points.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FeedWindow {
    identifier: String,
    frame: (i32, i32, i32, i32),
}

/// The bound feeds and the group they were last sent.
#[derive(Default)]
pub(super) struct Feeds {
    resources: Vec<CupertinoScreenshotV1>,
    sent: Vec<FeedWindow>,
}

fn send(resource: &CupertinoScreenshotV1, windows: &[FeedWindow]) {
    for w in windows {
        let (x, y, width, height) = w.frame;
        resource.window(w.identifier.clone(), x, y, width, height);
    }
    resource.done();
}

impl Cupertino {
    /// The windows drawn on an output now, front to back: frames with decorations, without shadows, where the
    /// workspace slide puts them (the rectangle a toplevel capture covers).
    fn screen_windows(&self) -> Vec<FeedWindow> {
        let now = self.now();
        let options = self.display_options();
        let transition = WorkspaceTransition::for_options(options);
        let shown: HashMap<OutputId, Vec<(WindowId, f64, f64)>> = self
            .outputs
            .iter()
            .map(|o| {
                (
                    o.id,
                    windows_on_output(&self.wm, o.id, o.rect.w, transition, now),
                )
            })
            .collect();
        let mut out = Vec::new();
        for id in self.wm.stacking_order().iter().rev() {
            let Some(output) = self.wm.window(*id).map(|w| w.output) else {
                continue;
            };
            let Some(slide) = shown
                .get(&output)
                .and_then(|list| list.iter().find(|(w, _, _)| w == id))
                .map(|(_, s, _)| *s)
            else {
                continue;
            };
            let Some(entry) = self.entry(*id) else {
                continue;
            };
            let Some(handle) = &entry.foreign else {
                continue;
            };
            let Some((plan, _)) = plan_for(&self.wm, entry, slide, now, &self.fx, options) else {
                continue;
            };
            let geo = entry.window.geometry();
            let (w, h) = (
                f64::from(geo.size.w) + entry.insets().horizontal(),
                f64::from(geo.size.h) + entry.insets().vertical(),
            );
            out.push(FeedWindow {
                identifier: handle.identifier(),
                frame: (
                    plan.visual.x.round() as i32,
                    plan.visual.y.round() as i32,
                    w.round() as i32,
                    h.round() as i32,
                ),
            });
        }
        out
    }

    /// Send the window list to every feed when it changed since the last group.
    pub(super) fn refresh_window_feeds(&mut self) {
        if self.capture.feeds.resources.is_empty() {
            return;
        }
        let windows = self.screen_windows();
        if windows == self.capture.feeds.sent {
            return;
        }
        for r in &self.capture.feeds.resources {
            send(r, &windows);
        }
        self.capture.feeds.sent = windows;
    }
}

impl GlobalDispatch<CupertinoScreenshotV1, ()> for Cupertino {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<CupertinoScreenshotV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let resource = init.init(resource, ());
        let windows = state.screen_windows();
        let feeds = &mut state.capture.feeds;
        // the feeds already bound get the group too when it changed since they had the last one
        if windows != feeds.sent {
            for r in &feeds.resources {
                send(r, &windows);
            }
        }
        send(&resource, &windows);
        feeds.sent = windows;
        feeds.resources.push(resource);
    }

    fn can_view(client: Client, _: &()) -> bool {
        privileged(&client)
    }
}

impl Dispatch<CupertinoScreenshotV1, ()> for Cupertino {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &CupertinoScreenshotV1,
        request: cupertino_screenshot_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            cupertino_screenshot_v1::Request::Destroy => {}
            _ => unreachable!("cupertino_screenshot_v1 version 1 has no other request"),
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &CupertinoScreenshotV1, _: &()) {
        state.capture.feeds.resources.retain(|r| r != resource);
    }
}
