//! Screen capture for the screenshot process (process model rule 2): the standard
//! `ext-image-capture-source-v1` (output and toplevel sources) and `ext-image-copy-capture-v1` (shm frames with
//! damage; the cursor is painted only when the session asks for it), plus `cupertino_screenshot_v1`, where the windows
//! are drawn ([`windows`]). smithay 0.7 implements neither standard protocol, so this module serves them on the
//! generated bindings of `wayland-protocols`.
//!
//! Every global is offered only to privileged clients: the shell processes the server started on a connection of its
//! own ([`launch`]). A client of the public socket does not see them, so an application cannot read the screen.
//!
//! The screen is not captured while the session is locked: an output is copied as black and a window frame waits until
//! the session is unlocked. The surfaces of the authentication prompt are never captured.
//!
//! Frames are copied after an output's frame was rendered ([`Cupertino::process_captures`]): the source is rendered once
//! more offscreen (an output through the normal element tree, a toplevel at its natural size like the window
//! snapshots), compared with the last frame its session delivered and, when something changed or the session has had no
//! frame yet, copied into the client's buffer with the changed rectangle as damage. A frame of an unchanged source
//! waits for the next change, as the protocol allows.

mod copy;
pub mod launch;
mod windows;

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::output::{Output, WeakOutput};
use smithay::reexports::wayland_protocols::ext::image_capture_source::v1::server::ext_foreign_toplevel_image_capture_source_manager_v1::{
    self as toplevel_manager, ExtForeignToplevelImageCaptureSourceManagerV1,
};
use smithay::reexports::wayland_protocols::ext::image_capture_source::v1::server::ext_image_capture_source_v1::ExtImageCaptureSourceV1;
use smithay::reexports::wayland_protocols::ext::image_capture_source::v1::server::ext_output_image_capture_source_manager_v1::{
    self as output_manager, ExtOutputImageCaptureSourceManagerV1,
};
use smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New};
use smithay::wayland::foreign_toplevel_list::{ForeignToplevelHandle, ForeignToplevelWeakHandle};

use cupertino_protocols::server::screenshot::cupertino_screenshot_v1::CupertinoScreenshotV1;
use cupertino_wm::WindowId;

use crate::authorization::{authorized, ShellRole};
use crate::state::Cupertino;

/// Version of every global of this module.
const VERSION: u32 = 1;

/// Whether `client` may see and use the capture globals: the screenshot process the server started itself ([`launch`]).
fn privileged(client: &Client) -> bool {
    authorized(client, ShellRole::Capture)
}
fn capture_client(client: &Client) -> bool {
    privileged(client) || authorized(client, ShellRole::DesktopControl)
}

/// What an `ext_image_capture_source_v1` captures; `None` when the output or window was already gone when the source
/// was created, so its sessions stop at once.
#[derive(Clone)]
pub enum Source {
    Output(Option<WeakOutput>),
    Toplevel(Option<ForeignToplevelWeakHandle>),
    /// A controller's output source is bound to its granted run epoch, never to a later grant.
    ControlledOutput(Option<WeakOutput>, u64),
}

/// Where a source is drawn right now: the output it is rendered with and its size in buffer pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Placement {
    output: usize,
    size: (i32, i32),
    window: Option<WindowId>,
}

/// The capture globals, the copy sessions and their frames, and the window feeds.
pub struct Capture {
    _globals: [GlobalId; 4],
    copy: copy::Sessions,
    feeds: windows::Feeds,
}

impl Capture {
    pub fn new(dh: &DisplayHandle) -> Self {
        Self {
            _globals: [
                dh.create_global::<Cupertino, ExtOutputImageCaptureSourceManagerV1, ()>(
                    VERSION,
                    (),
                ),
                dh.create_global::<Cupertino, ExtForeignToplevelImageCaptureSourceManagerV1, ()>(
                    VERSION,
                    (),
                ),
                dh.create_global::<Cupertino, ExtImageCopyCaptureManagerV1, ()>(VERSION, ()),
                dh.create_global::<Cupertino, CupertinoScreenshotV1, ()>(VERSION, ()),
            ],
            copy: copy::Sessions::default(),
            feeds: windows::Feeds::default(),
        }
    }
}

impl Cupertino {
    /// Copy the frames waiting on the sources drawn on output `idx` and refresh the window feeds. Called after the
    /// output's frame was rendered, by every backend (through [`Cupertino::process_screenshots`]). A session whose
    /// connection lost the Capture role is stopped first. While the session is locked (from the lock request on) an
    /// output is copied as black, a window frame waits for the unlock and the window feeds are not refreshed, so no
    /// buffer shows the lock screen or what it covers.
    pub fn process_captures(&mut self, renderer: &mut GlesRenderer, idx: usize) {
        self.refresh_control();
        self.stop_captures();
        if !self.session_lock.is_active() {
            self.refresh_window_feeds();
        }
        self.copy_frames(renderer, idx);
    }

    pub(super) fn capture_source_allowed(&self, client: &Client, source: &Source) -> bool {
        if privileged(client) {
            return true;
        }
        match source {
            Source::ControlledOutput(Some(output), epoch) => output
                .upgrade()
                .is_some_and(|o| self.control_capture_allowed(client, &o, *epoch)),
            _ => false,
        }
    }

    /// Where `source` is drawn, `None` once its output or window is gone.
    fn placement(&self, source: &Source) -> Option<Placement> {
        match source {
            Source::Output(weak) | Source::ControlledOutput(weak, _) => {
                let output = weak.as_ref()?.upgrade()?;
                let idx = self.outputs.iter().position(|o| o.output == output)?;
                // the output as it is shown: a quarter-turned output has its mode on its side
                let size = output
                    .current_transform()
                    .transform_size(output.current_mode()?.size);
                Some(Placement {
                    output: idx,
                    size: (size.w, size.h),
                    window: None,
                })
            }
            Source::Toplevel(weak) => {
                let handle = weak.as_ref()?.upgrade().filter(|h| !h.is_closed())?;
                let identifier = handle.identifier();
                let entry = self.windows.iter().find(|e| {
                    e.foreign
                        .as_ref()
                        .is_some_and(|f| f.identifier() == identifier)
                })?;
                let window = self.wm.window(entry.id)?;
                let idx = self.outputs.iter().position(|o| o.id == window.output)?;
                let scale = self.outputs[idx].scale;
                let geo = entry.window.geometry();
                let ins = entry.insets();
                // the size `capture_window` renders the window at
                let px = |logical: f64| ((logical * scale).round() as i32).max(1);
                let size = (
                    px(f64::from(geo.size.w) + ins.horizontal()),
                    px(f64::from(geo.size.h) + ins.vertical()),
                );
                Some(Placement {
                    output: idx,
                    size,
                    window: Some(entry.id),
                })
            }
        }
    }
}

impl GlobalDispatch<ExtOutputImageCaptureSourceManagerV1, ()> for Cupertino {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtOutputImageCaptureSourceManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }

    fn can_view(client: Client, _: &()) -> bool {
        capture_client(&client)
    }
}

impl Dispatch<ExtOutputImageCaptureSourceManagerV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        _: &ExtOutputImageCaptureSourceManagerV1,
        request: output_manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            output_manager::Request::CreateSource { source, output } => {
                state.refresh_control();
                let output = Output::from_resource(&output);
                let value = if privileged(client) {
                    Source::Output(output.map(|o| o.downgrade()))
                } else if let Some((output, epoch)) =
                    output.and_then(|o| state.control_capture_epoch(client, &o).map(|e| (o, e)))
                {
                    Source::ControlledOutput(Some(output.downgrade()), epoch)
                } else {
                    Source::ControlledOutput(None, 0)
                };
                init.init(source, value);
            }
            output_manager::Request::Destroy => {}
            _ => unreachable!(
                "ext_output_image_capture_source_manager_v1 version 1 has no other request"
            ),
        }
    }
}

impl GlobalDispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> for Cupertino {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtForeignToplevelImageCaptureSourceManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }

    fn can_view(client: Client, _: &()) -> bool {
        privileged(&client)
    }
}

impl Dispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> for Cupertino {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ExtForeignToplevelImageCaptureSourceManagerV1,
        request: toplevel_manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            toplevel_manager::Request::CreateSource { source, toplevel_handle } => {
                init.init(source, Source::Toplevel(ForeignToplevelHandle::from_resource(&toplevel_handle).map(|h| h.downgrade())));
            }
            toplevel_manager::Request::Destroy => {}
            _ => unreachable!("ext_foreign_toplevel_image_capture_source_manager_v1 version 1 has no other request"),
        }
    }
}

/// A source has only `destroy`; sessions keep their own copy of what it names.
impl Dispatch<ExtImageCaptureSourceV1, Source> for Cupertino {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ExtImageCaptureSourceV1,
        _: smithay::reexports::wayland_protocols::ext::image_capture_source::v1::server::ext_image_capture_source_v1::Request,
        _: &Source,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}
