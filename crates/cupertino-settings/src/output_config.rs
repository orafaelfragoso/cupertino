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

/// A mode of a head: its size in device pixels and its refresh rate in millihertz.
struct Mode {
    proxy: ZwlrOutputModeV1,
    width: i32,
    height: i32,
    refresh: i32,
}

/// A head as the server described it.
struct Head {
    proxy: ZwlrOutputHeadV1,
    name: Option<String>,
    enabled: bool,
    modes: Vec<Mode>,
    current: Option<ZwlrOutputModeV1>,
    position: (i32, i32),
}

/// What a display's configuration changes: the fields that are set.
#[derive(Clone, Copy, Debug, Default)]
pub struct Change {
    pub scale: Option<f64>,
    /// The refresh rate in millihertz, at the size the display has now.
    pub refresh_mhz: Option<u32>,
    /// Where the display's top-left corner lies in the desktop, in points.
    pub position: Option<(i32, i32)>,
}

/// What the server says about a display's modes and place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub connector: String,
    pub position: (i32, i32),
    /// The refresh rates, in millihertz, the display has at its current size, ascending.
    pub refresh_rates: Vec<u32>,
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
    configure(
        connector,
        Change {
            scale: Some(scale),
            ..Change::default()
        },
    )
}

/// The place and the refresh rates of each enabled display, from one short connection.
pub fn layouts() -> Result<Vec<Layout>, String> {
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
    let layouts = state
        .heads
        .iter()
        .filter(|h| h.enabled)
        .filter_map(|h| {
            let connector = h.name.clone()?;
            let current = h
                .current
                .as_ref()
                .and_then(|c| h.modes.iter().find(|m| &m.proxy == c));
            let mut refresh_rates: Vec<u32> = current
                .map(|c| {
                    h.modes
                        .iter()
                        .filter(|m| (m.width, m.height) == (c.width, c.height) && m.refresh > 0)
                        .map(|m| m.refresh as u32)
                        .collect()
                })
                .unwrap_or_default();
            refresh_rates.sort_unstable();
            refresh_rates.dedup();
            Some(Layout {
                connector,
                position: h.position,
                refresh_rates,
            })
        })
        .collect();
    for head in &state.heads {
        if head.proxy.version() >= 3 {
            head.proxy.release();
        }
    }
    manager.stop();
    let _ = conn.flush();
    Ok(layouts)
}

/// Change the display on `connector` as `change` says. Fails when the server offers no output management, the display
/// is gone or off, the mode asked for does not exist, or the server refuses or cancels the configuration.
pub fn configure(connector: &str, change: Change) -> Result<(), String> {
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
    let result = apply(&manager, &mut queue, &qh, &mut state, connector, change);
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
    change: Change,
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
                if let Some(scale) = change.scale {
                    configured.set_scale(scale);
                }
                if let Some(mhz) = change.refresh_mhz {
                    let size = head
                        .current
                        .as_ref()
                        .and_then(|c| head.modes.iter().find(|m| &m.proxy == c))
                        .map(|m| (m.width, m.height));
                    let mode = head
                        .modes
                        .iter()
                        .find(|m| Some((m.width, m.height)) == size && m.refresh == mhz as i32)
                        .ok_or_else(|| "this display has no such refresh rate".to_string())?;
                    configured.set_mode(&mode.proxy);
                }
                if let Some((x, y)) = change.position {
                    configured.set_position(x, y);
                }
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
                modes: Vec::new(),
                current: None,
                position: (0, 0),
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
            zwlr_output_head_v1::Event::Mode { mode } => head.modes.push(Mode {
                proxy: mode,
                width: 0,
                height: 0,
                refresh: 0,
            }),
            zwlr_output_head_v1::Event::CurrentMode { mode } => head.current = Some(mode),
            zwlr_output_head_v1::Event::Position { x, y } => head.position = (x, y),
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
        state: &mut Self,
        proxy: &ZwlrOutputModeV1,
        event: <ZwlrOutputModeV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_protocols_wlr::output_management::v1::client::zwlr_output_mode_v1::Event;
        let Some(mode) = state
            .heads
            .iter_mut()
            .flat_map(|h| h.modes.iter_mut())
            .find(|m| &m.proxy == proxy)
        else {
            return;
        };
        match event {
            Event::Size { width, height } => (mode.width, mode.height) = (width, height),
            Event::Refresh { refresh } => mode.refresh = refresh,
            _ => {}
        }
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
