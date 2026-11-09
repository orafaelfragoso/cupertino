//! Setting a display's scale through `zwlr_output_management_unstable_v1`: one short connection reads the heads,
//! applies a configuration that names every head and changes only the scale of the one asked for, and waits for the
//! server's answer.

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry;
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::output_management::v1::client::{
    zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1,
    zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
    zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::ZwlrOutputModeV1,
};

/// A head as the server described it.
struct Head {
    proxy: ZwlrOutputHeadV1,
    name: Option<String>,
    enabled: bool,
}

#[derive(Default)]
struct State {
    heads: Vec<Head>,
    /// The serial of the last complete description.
    serial: Option<u32>,
    /// The manager stopped: the session locked, or the server is going away.
    finished: bool,
    outcome: Option<Result<(), String>>,
}

/// Set the display on `connector` to `scale` panel pixels per point. Fails when the server offers no output
/// management, the display is gone or off, or the server refuses or cancels the configuration.
pub fn set_scale(connector: &str, scale: f64) -> Result<(), String> {
    if !(scale.is_finite() && scale > 0.0) {
        return Err(format!("scale {scale} is not above zero"));
    }
    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let manager: ZwlrOutputManagerV1 = globals
        .bind(&qh, 1..=4, ())
        .map_err(|_| "the server offers no output management".to_string())?;
    let mut state = State::default();
    while state.serial.is_none() && !state.finished {
        queue
            .blocking_dispatch(&mut state)
            .map_err(|e| e.to_string())?;
    }
    let result = apply(&manager, &mut queue, &qh, &mut state, connector, scale);
    for head in &state.heads {
        if head.proxy.version() >= 3 {
            head.proxy.release();
        }
    }
    manager.stop();
    let _ = conn.flush();
    result
}

fn apply(
    manager: &ZwlrOutputManagerV1,
    queue: &mut wayland_client::EventQueue<State>,
    qh: &QueueHandle<State>,
    state: &mut State,
    connector: &str,
    scale: f64,
) -> Result<(), String> {
    let serial = state
        .serial
        .ok_or_else(|| "the server stopped describing the displays".to_string())?;
    if !state
        .heads
        .iter()
        .any(|h| h.enabled && h.name.as_deref() == Some(connector))
    {
        return Err(format!("{connector} is not connected or is off"));
    }
    let config = manager.create_configuration(serial, qh, ());
    let mut named = Vec::new();
    for head in &state.heads {
        if head.enabled {
            let configured = config.enable_head(&head.proxy, qh, ());
            if head.name.as_deref() == Some(connector) {
                configured.set_scale(scale);
            }
            named.push(configured);
        } else {
            config.disable_head(&head.proxy);
        }
    }
    config.apply();
    while state.outcome.is_none() {
        queue.blocking_dispatch(state).map_err(|e| e.to_string())?;
    }
    config.destroy();
    drop(named);
    state.outcome.take().expect("the loop waited for it")
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputManagerV1,
        event: zwlr_output_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_manager_v1::Event::Head { head } => state.heads.push(Head {
                proxy: head,
                name: None,
                enabled: false,
            }),
            zwlr_output_manager_v1::Event::Done { serial } => state.serial = Some(serial),
            zwlr_output_manager_v1::Event::Finished => state.finished = true,
            _ => {}
        }
    }

    event_created_child!(State, ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &ZwlrOutputHeadV1,
        event: zwlr_output_head_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(head) = state.heads.iter_mut().find(|h| &h.proxy == proxy) else {
            return;
        };
        match event {
            zwlr_output_head_v1::Event::Name { name } => head.name = Some(name),
            zwlr_output_head_v1::Event::Enabled { enabled } => head.enabled = enabled != 0,
            zwlr_output_head_v1::Event::Finished => {
                state.heads.retain(|h| &h.proxy != proxy);
            }
            _ => {}
        }
    }

    event_created_child!(State, ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputModeV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrOutputModeV1,
        _: <ZwlrOutputModeV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputConfigurationV1,
        event: zwlr_output_configuration_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.outcome = match event {
            zwlr_output_configuration_v1::Event::Succeeded => Some(Ok(())),
            zwlr_output_configuration_v1::Event::Failed => {
                Some(Err("the server refused the configuration".to_string()))
            }
            zwlr_output_configuration_v1::Event::Cancelled => Some(Err(
                "the displays changed while the configuration was made".to_string(),
            )),
            _ => return,
        };
    }
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrOutputConfigurationHeadV1,
        _: <ZwlrOutputConfigurationHeadV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
