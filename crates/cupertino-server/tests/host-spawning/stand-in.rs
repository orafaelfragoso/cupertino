//! A stand-in for `cupertino-shell` in the server's tests: the supervisor starts the test binary of
//! `tests/host-spawning.rs` as the host program, and its test `stand_in_host` runs [`run`]. As a host it binds `cupertino_host_v1` on its `WAYLAND_SOCKET`, asks for a connection for every component of its list
//! and runs each as a component that says it is ready at once. It answers `unavailable` for the components it was
//! "built without" ([`BUILT_WITHOUT`]), starts a component again when the server asks for a restart, exits when the
//! server sends `restart`, and exits successfully as a separate host whose list became empty.
//!
//! The menu bar ([`PERSISTING`]) maps a persisted bar reserving [`BAR_HEIGHT`] at the top of its output before it says
//! it is ready, so what the server holds for it can be looked at.

use std::collections::HashMap;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;

use cupertino_protocols::client::host::cupertino_component_v1::CupertinoComponentV1;
use cupertino_protocols::client::host::cupertino_host_component_v1::{
    self, CupertinoHostComponentV1, UnavailableReason,
};
use cupertino_protocols::client::host::cupertino_host_v1::{self, CupertinoHostV1, Kind};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

/// The components this stand-in host was built without: it answers `unavailable` with `absent` for each.
const BUILT_WITHOUT: [&str; 1] = ["dialogs"];

/// The components that map a persisted bar before they say they are ready.
const PERSISTING: [&str; 1] = ["menubar"];

/// The key a persisting component persists its bar under.
const BAR_KEY: &str = "bar";

/// The height of a persisting component's bar in logical pixels, and the space it reserves at the top of its output.
pub const BAR_HEIGHT: u32 = 28;

/// Be the host on the connection of `WAYLAND_SOCKET`.
pub fn run() {
    let conn = Connection::connect_to_env().expect("started with a WAYLAND_SOCKET");
    let mut queue = conn.new_event_queue::<Host>();
    let _registry = conn.display().get_registry(&queue.handle(), ());
    let mut host = Host::default();
    while !host.exit && queue.blocking_dispatch(&mut host).is_ok() {}
}

#[derive(Default)]
struct Host {
    host: Option<CupertinoHostV1>,
    kind: Option<Kind>,
    /// The names of the list being sent.
    incoming: Vec<String>,
    /// The component objects of the list in force.
    objects: HashMap<String, CupertinoHostComponentV1>,
    exit: bool,
}

impl Host {
    /// Make the list just sent the one in force: objects for the new names, none for the names that left.
    fn apply_list(&mut self, qh: &QueueHandle<Self>) {
        let list = std::mem::take(&mut self.incoming);
        self.objects.retain(|name, object| {
            let keep = list.contains(name);
            if !keep {
                object.destroy();
            }
            keep
        });
        let Some(host) = self.host.clone() else {
            return;
        };
        for name in &list {
            if self.objects.contains_key(name) {
                continue;
            }
            let object = host.get_component(name.clone(), qh, name.clone());
            if BUILT_WITHOUT.contains(&name.as_str()) {
                object.unavailable(UnavailableReason::Absent);
            } else {
                object.connect();
            }
            self.objects.insert(name.clone(), object);
        }
        if self.kind == Some(Kind::Separate) && list.is_empty() {
            self.exit = true;
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Host {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            if interface == "cupertino_host_v1" {
                state.host = Some(registry.bind::<CupertinoHostV1, _, _>(name, 1, qh, ()));
            }
        }
    }
}

impl Dispatch<CupertinoHostV1, ()> for Host {
    fn event(
        state: &mut Self,
        _: &CupertinoHostV1,
        event: cupertino_host_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_host_v1::Event::Kind {
                kind: WEnum::Value(kind),
            } => state.kind = Some(kind),
            cupertino_host_v1::Event::Component { name } => state.incoming.push(name),
            cupertino_host_v1::Event::Done => state.apply_list(qh),
            cupertino_host_v1::Event::Restart => state.exit = true,
            // the bus bridge's descriptors are closed as they are dropped
            _ => {}
        }
    }
}

impl Dispatch<CupertinoHostComponentV1, String> for Host {
    fn event(
        _: &mut Self,
        object: &CupertinoHostComponentV1,
        event: cupertino_host_component_v1::Event,
        name: &String,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_host_component_v1::Event::Connection { fd } => {
                let name = name.clone();
                std::thread::spawn(move || run_component(&name, UnixStream::from(fd)));
            }
            cupertino_host_component_v1::Event::Restart => object.connect(),
            _ => {}
        }
    }
}

/// A component on its own connection: it binds `cupertino_component_v1`, maps its bar if it is one of
/// [`PERSISTING`], says it is ready, and runs until the server closes the connection.
fn run_component(name: &str, socket: UnixStream) {
    let Ok(conn) = Connection::from_socket(socket) else {
        return;
    };
    let mut queue = conn.new_event_queue::<Component>();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());
    let mut component = Component::default();
    if queue.roundtrip(&mut component).is_err() {
        return;
    }
    let Some(object) = component.object.clone() else {
        eprintln!("stand-in host: {name} sees no cupertino_component_v1");
        return;
    };
    let _bar = if PERSISTING.contains(&name) {
        match map_bar(name, &object, &mut queue, &mut component) {
            Some(bar) => Some(bar),
            None => return,
        }
    } else {
        None
    };
    object.ready();
    while queue.blocking_dispatch(&mut component).is_ok() {}
}

/// What a mapped bar needs kept alive.
struct Bar {
    _surface: wl_surface::WlSurface,
    _layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    _buffer: (std::fs::File, wl_shm_pool::WlShmPool, wl_buffer::WlBuffer),
}

/// Map a bar across the top of the output, persisted under [`BAR_KEY`] and reserving [`BAR_HEIGHT`].
fn map_bar(
    name: &str,
    object: &CupertinoComponentV1,
    queue: &mut wayland_client::EventQueue<Component>,
    component: &mut Component,
) -> Option<Bar> {
    let qh = queue.handle();
    let (Some(compositor), Some(shm), Some(layer_shell)) = (
        component.compositor.clone(),
        component.shm.clone(),
        component.layer_shell.clone(),
    ) else {
        eprintln!("stand-in host: {name} cannot map its bar");
        return None;
    };
    let surface = compositor.create_surface(&qh, ());
    let layer = layer_shell.get_layer_surface(
        &surface,
        None,
        zwlr_layer_shell_v1::Layer::Top,
        name.into(),
        &qh,
        (),
    );
    layer.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top
            | zwlr_layer_surface_v1::Anchor::Left
            | zwlr_layer_surface_v1::Anchor::Right,
    );
    layer.set_size(0, BAR_HEIGHT);
    layer.set_exclusive_zone(BAR_HEIGHT as i32);
    object.persist(&surface, BAR_KEY.into());
    surface.commit();
    queue.roundtrip(component).ok()?;
    let width = component.width.filter(|w| *w > 0)? as i32;
    let height = BAR_HEIGHT as i32;
    let stride = width * 4;
    let size = (stride * height) as usize;
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("stand-in-bar-{}-{name}.bin", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .ok()?;
    let _ = std::fs::remove_file(&path);
    file.write_all(&vec![0x7f; size]).ok()?;
    let pool = shm.create_pool(file.as_fd(), size as i32, &qh, ());
    let buffer = pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, &qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.commit();
    queue.roundtrip(component).ok()?;
    Some(Bar {
        _surface: surface,
        _layer: layer,
        _buffer: (file, pool, buffer),
    })
}

#[derive(Default)]
struct Component {
    object: Option<CupertinoComponentV1>,
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    /// The width the bar was configured to.
    width: Option<u32>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Component {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "cupertino_component_v1" => {
                    state.object = Some(registry.bind(name, 1, qh, ()));
                }
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(6), qh, ()))
                }
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for Component {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, width, .. } = event {
            layer.ack_configure(serial);
            state.width = Some(width);
        }
    }
}

wayland_client::delegate_noop!(Component: ignore CupertinoComponentV1);
wayland_client::delegate_noop!(Component: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(Component: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(Component: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(Component: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(Component: ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(Component: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
