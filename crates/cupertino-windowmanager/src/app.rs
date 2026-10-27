//! The process: one `cupertino_window_manager_v1` per output, each driven by a [`Controller`].
//! It has no surface, so it speaks the protocol directly instead of through the shell toolkit,
//! which requires the globals of surface clients.

use std::convert::Infallible;
use std::time::Instant;

use cupertino_protocols::client::cupertino_shell_v1::CupertinoShellV1;
use cupertino_protocols::client::cupertino_window_manager_v1::{
    self, CupertinoWindowManagerV1, Placement as WirePlacement, WindowState,
};
use cupertino_stage::{Placement, StageConfig};
use cupertino_windowmanager::{parse_conf, Change, Controller, Request, Window};
use cupertino_wm::geometry::Rect;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};

/// The window manager of one output.
struct Manager {
    /// Registry name of the output's global.
    name: u32,
    output: wl_output::WlOutput,
    object: CupertinoWindowManagerV1,
    controller: Controller,
    /// Events of the group under way.
    pending: Vec<Change>,
}

struct App {
    shell: CupertinoShellV1,
    managers: Vec<Manager>,
    /// The stage setting at start, for the first output.
    stage_setting: bool,
    start: Instant,
}

impl App {
    fn add_output(
        &mut self,
        registry: &wl_registry::WlRegistry,
        name: u32,
        version: u32,
        qh: &QueueHandle<Self>,
    ) {
        let output: wl_output::WlOutput = registry.bind(name, version.min(4), qh, ());
        let object = self.shell.get_window_manager(&output, qh, name);
        // an output that appears later follows the stage as it is now on the others
        let enabled = self
            .managers
            .first()
            .map_or(self.stage_setting, |m| m.controller.is_enabled());
        self.managers.push(Manager {
            name,
            output,
            object,
            controller: Controller::new(StageConfig::default(), enabled),
            pending: Vec::new(),
        });
    }

    fn remove_output(&mut self, name: u32) {
        if let Some(i) = self.managers.iter().position(|m| m.name == name) {
            let m = self.managers.remove(i);
            m.object.destroy();
            if m.output.version() >= 3 {
                m.output.release();
            }
        }
    }
}

fn whole(r: Rect) -> (i32, i32, i32, i32) {
    let (x, y) = (r.x.round(), r.y.round());
    (
        x as i32,
        y as i32,
        ((r.right().round() - x) as i32).max(1),
        ((r.bottom().round() - y) as i32).max(1),
    )
}

fn send(object: &CupertinoWindowManagerV1, requests: Vec<Request>) {
    for request in requests {
        match request {
            Request::Place {
                id,
                placement: Placement::Stage,
            } => object.set_placement(id, WirePlacement::Stage, 0, 0, 0, 0),
            Request::Place {
                id,
                placement: Placement::Hidden,
            } => object.set_placement(id, WirePlacement::Hidden, 0, 0, 0, 0),
            Request::Place {
                id,
                placement: Placement::Strip(rect),
            } => {
                let (x, y, w, h) = whole(rect);
                object.set_placement(id, WirePlacement::Strip, x, y, w, h);
            }
            Request::Commit => object.commit(),
            Request::Activate(id) => object.activate_window(id),
        }
    }
}

fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
    Rect::new(
        f64::from(x),
        f64::from(y),
        f64::from(width),
        f64::from(height),
    )
}

impl Dispatch<CupertinoWindowManagerV1, u32> for App {
    fn event(
        app: &mut Self,
        _: &CupertinoWindowManagerV1,
        event: cupertino_window_manager_v1::Event,
        name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use cupertino_window_manager_v1::Event as E;
        let now = app.start.elapsed().as_secs_f64();
        let Some(m) = app.managers.iter_mut().find(|m| m.name == *name) else {
            return;
        };
        let change = match event {
            E::UsableArea {
                x,
                y,
                width,
                height,
            } => Change::UsableArea(rect(x, y, width, height)),
            E::Window {
                id,
                app_id,
                x,
                y,
                width,
                height,
                state,
            } => {
                let active = matches!(state, WEnum::Value(s) if s.contains(WindowState::Active));
                Change::Window(Window {
                    id,
                    app_id,
                    frame: rect(x, y, width, height),
                    active,
                })
            }
            E::WindowClosed { id } => Change::WindowClosed(id),
            E::ToggleStage => Change::ToggleStage,
            E::StripPressed { id } => Change::StripPressed(id),
            E::Done => {
                let group = std::mem::take(&mut m.pending);
                let requests = m.controller.apply(&group, now);
                send(&m.object, requests);
                return;
            }
            other => {
                eprintln!("cupertino-windowmanager: ignoring an event this version does not know: {other:?}");
                return;
            }
        };
        m.pending.push(change);
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
    fn event(
        app: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_output" => app.add_output(registry, name, version, qh),
            wl_registry::Event::GlobalRemove { name } => app.remove_output(name),
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for App {
    fn event(
        _: &mut Self,
        _: &wl_output::WlOutput,
        _: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<CupertinoShellV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &CupertinoShellV1,
        _: <CupertinoShellV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// The stage setting of `windowmanager.conf` (`$CUPERTINO_WINDOWMANAGER_CONFIG` overrides the
/// path): off without a file. Lines it cannot use are reported and skipped.
fn stage_setting() -> Result<bool, String> {
    let Some(path) =
        cupertino_defaults::config_file("CUPERTINO_WINDOWMANAGER_CONFIG", "windowmanager.conf")
    else {
        return Ok(false);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let (file, problems) = parse_conf(&text);
    for p in problems {
        eprintln!(
            "cupertino-windowmanager: {}:{}: {}, line skipped",
            path.display(),
            p.line,
            p.problem
        );
    }
    Ok(file.stage.unwrap_or(false))
}

/// Connect, become the window manager of every output and serve until the connection fails.
pub fn run() -> Result<Infallible, String> {
    let stage = stage_setting()?;
    let conn =
        Connection::connect_to_env().map_err(|e| format!("cannot connect to the server: {e}"))?;
    let (globals, mut queue) = registry_queue_init::<App>(&conn)
        .map_err(|e| format!("cannot read the server's globals: {e}"))?;
    let qh = queue.handle();
    let shell: CupertinoShellV1 = globals
        .bind(&qh, 8..=8, ())
        .map_err(|e| format!("the server has no cupertino_shell_v1: {e}"))?;
    let mut app = App {
        shell,
        managers: Vec::new(),
        stage_setting: stage,
        start: Instant::now(),
    };
    let outputs: Vec<(u32, u32)> = globals.contents().with_list(|list| {
        list.iter()
            .filter(|g| g.interface == "wl_output")
            .map(|g| (g.name, g.version))
            .collect()
    });
    for (name, version) in outputs {
        app.add_output(globals.registry(), name, version, &qh);
    }
    eprintln!(
        "cupertino-windowmanager: arranging {} output(s), stage {}",
        app.managers.len(),
        if stage { "on" } else { "off" }
    );
    loop {
        if let Err(e) = queue.blocking_dispatch(&mut app) {
            return Err(match conn.protocol_error() {
                Some(p) => format!("protocol error on {}: {}", p.object_interface, p.message),
                None => format!("the connection failed: {e}"),
            });
        }
    }
}
