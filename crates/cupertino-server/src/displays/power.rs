//! `zwlr_output_power_management_unstable_v1`: a client (an idle daemon) turns an output's display
//! off and on. A powered-off output is not drawn; the TTY backend also blanks its CRTC.

use cupertino_wm::OutputId;
use smithay::output::Output;
use smithay::reexports::wayland_protocols_wlr::output_power_management::v1::server::{
    zwlr_output_power_manager_v1::{self, ZwlrOutputPowerManagerV1},
    zwlr_output_power_v1::{self, Mode, ZwlrOutputPowerV1},
};
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};

use crate::state::Cupertino;

pub const VERSION: u32 = 1;

/// A power object and the output it controls.
pub(crate) struct PowerObject {
    resource: ZwlrOutputPowerV1,
    output: OutputId,
}

pub fn create_global(dh: &DisplayHandle) -> GlobalId {
    dh.create_global::<Cupertino, ZwlrOutputPowerManagerV1, ()>(VERSION, ())
}

fn mode(on: bool) -> Mode {
    if on {
        Mode::On
    } else {
        Mode::Off
    }
}

/// `output` went away: its power objects fail and are forgotten.
pub(crate) fn output_gone(state: &mut Cupertino, output: OutputId) {
    state.displays.powers.retain(|p| {
        if p.output != output {
            return true;
        }
        p.resource.failed();
        false
    });
}

/// `output` was powered on or off: every power object of it is told.
pub(crate) fn mode_changed(state: &mut Cupertino, output: OutputId, on: bool) {
    for p in state.displays.powers.iter().filter(|p| p.output == output) {
        p.resource.mode(mode(on));
    }
}

impl GlobalDispatch<ZwlrOutputPowerManagerV1, ()> for Cupertino {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwlrOutputPowerManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl Dispatch<ZwlrOutputPowerManagerV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwlrOutputPowerManagerV1,
        request: zwlr_output_power_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_output_power_manager_v1::Request::GetOutputPower { id, output } = request {
            let resource = init.init(id, ());
            let found =
                Output::from_resource(&output).and_then(|o| state.output_by_wl(&o).map(|e| e.id));
            match found {
                Some(output) => {
                    resource.mode(mode(state.displays.powered(output)));
                    state.displays.powers.push(PowerObject { resource, output });
                }
                // the output is already gone
                None => resource.failed(),
            }
        }
    }
}

impl Dispatch<ZwlrOutputPowerV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ZwlrOutputPowerV1,
        request: zwlr_output_power_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let zwlr_output_power_v1::Request::SetMode { mode: wanted } = request else {
            return;
        };
        let Some(output) = state
            .displays
            .powers
            .iter()
            .find(|p| &p.resource == resource)
            .map(|p| p.output)
        else {
            return;
        };
        let on = match wanted {
            WEnum::Value(Mode::On) => true,
            WEnum::Value(Mode::Off) => false,
            WEnum::Value(_) | WEnum::Unknown(_) => {
                resource.post_error(
                    zwlr_output_power_v1::Error::InvalidMode,
                    "unknown power mode",
                );
                return;
            }
        };
        if let Err(e) = state.set_output_power(output, on) {
            tracing::warn!(error = %e, on, "setting the output power failed");
            resource.failed();
            state.displays.powers.retain(|p| &p.resource != resource);
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &ZwlrOutputPowerV1, _: &()) {
        state.displays.powers.retain(|p| &p.resource != resource);
    }
}
