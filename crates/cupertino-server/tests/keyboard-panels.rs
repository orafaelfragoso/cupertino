//! The keyboard's panels on the server's side (W6.12): only the connection that holds the \`keyboard_panels\` role
//! gets \`cupertino_keyboard_panels_v1\`, once; a pick inserts only a candidate of the open picker; no key of a switch
//! reaches a client; the input method's global is offered to no component; a layer surface with the picker's namespace
//! on a connection without the role is no panel; and the server draws neither panel.

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_keyboard::switcher::Step;
use cupertino_protocols::client::cupertino_keyboard_panels_v1::{self, CupertinoKeyboardPanelsV1};
use cupertino_protocols::client::cupertino_shell_v1::{self, CupertinoShellV1};
use cupertino_protocols::client::host::cupertino_host_component_v1::{
    self, CupertinoHostComponentV1,
};
use cupertino_protocols::client::host::cupertino_host_v1::{self, CupertinoHostV1};
use cupertino_server::authorization::{Holder, DETACHED_LIFETIME};
use cupertino_server::backend::headless;
use cupertino_server::host::HostKind;
use cupertino_server::kbd::SourceSpec;
use cupertino_server::state::{insert_client, ClientState};
use cupertino_server::{Config, Cupertino};
use smithay::backend::input::KeyState;
use smithay::input::keyboard::Keycode;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_protocols::xdg::shell::client::{
    xdg_surface, xdg_toplevel, xdg_wm_base,
};
use smithay::reexports::wayland_server::Display;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_output, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3::ZwpTextInputManagerV3,
    zwp_text_input_v3::{self, ZwpTextInputV3},
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

/// The shell version that has the keyboard's panels.
const KEYBOARD_PANELS: u32 = 30;
/// The offscreen output's size in pixels, at scale 1.
const OUTPUT_SIZE: (i32, i32) = (800, 600);
/// The picker's namespace (\`cupertino_kit_shell::keyboard_panels::ACCENTS_NAMESPACE\`).
const ACCENTS_NAMESPACE: &str = "cupertino-osd-accents";
/// The candidates of \`e\` in the test's accent table.
const ACCENT_TABLE: &str = r#"{"default":{"e":{"direction":"right","strings":["è","é","ê"]}}}"#;
/// The evdev codes of the left Control key and of the space bar.
const KEY_CTRL: u32 = 29;
const KEY_SPACE: u32 = 57;
/// How long a test waits for the server to do what it expects.
const WAIT: Duration = Duration::from_secs(30);

/// The shell host's side: its global and the component connections it was handed.
#[derive(Default)]
struct HostClient {
    host: Option<CupertinoHostV1>,
    connections: Vec<(String, std::os::fd::OwnedFd)>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for HostClient {
    fn event(
        state: &mut Self,
        reg: &wl_registry::WlRegistry,
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
                state.host = Some(reg.bind::<CupertinoHostV1, _, _>(name, 1, qh, ()));
            }
        }
    }
}

impl Dispatch<CupertinoHostV1, ()> for HostClient {
    fn event(
        _: &mut Self,
        _: &CupertinoHostV1,
        _: cupertino_host_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<CupertinoHostComponentV1, String> for HostClient {
    fn event(
        state: &mut Self,
        _: &CupertinoHostComponentV1,
        event: cupertino_host_component_v1::Event,
        name: &String,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let cupertino_host_component_v1::Event::Connection { fd } = event {
            state.connections.push((name.clone(), fd));
        }
    }
}

/// A connection of the test: a component's, or an ordinary client's with one window, and what it was told.
#[derive(Default)]
struct Client {
    /// The interfaces of the globals the server offered.
    globals: Vec<String>,
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    output: Option<wl_output::WlOutput>,
    seat: Option<wl_seat::WlSeat>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    shell: Option<CupertinoShellV1>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    text_inputs: Option<ZwpTextInputManagerV3>,
    configured: bool,
    /// The keys its keyboard was sent, by evdev code.
    keys: Vec<u32>,
    /// What its keyboard panels were told, in order.
    panels: Vec<String>,
    /// Its text input has the keyboard focus.
    entered: bool,
    /// The text its text input was given to commit.
    committed: Vec<String>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Client {
    fn event(
        state: &mut Self,
        reg: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            "wl_compositor" => state.compositor = Some(reg.bind(name, version.min(6), qh, ())),
            "wl_shm" => state.shm = Some(reg.bind(name, 1, qh, ())),
            "wl_output" => state.output = Some(reg.bind(name, version.min(4), qh, ())),
            "wl_seat" => {
                let seat: wl_seat::WlSeat = reg.bind(name, version.min(7), qh, ());
                seat.get_keyboard(qh, ());
                state.seat = Some(seat);
            }
            "zwlr_layer_shell_v1" => {
                state.layer_shell = Some(reg.bind(name, version.min(4), qh, ()))
            }
            "cupertino_shell_v1" => {
                state.shell = Some(reg.bind(name, version.min(KEYBOARD_PANELS), qh, ()))
            }
            "xdg_wm_base" => state.wm_base = Some(reg.bind(name, 1, qh, ())),
            "zwp_text_input_manager_v3" => state.text_inputs = Some(reg.bind(name, 1, qh, ())),
            _ => {}
        }
        state.globals.push(interface);
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key { key, .. } = event {
            state.keys.push(key);
        }
    }
}

impl Dispatch<CupertinoKeyboardPanelsV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &CupertinoKeyboardPanelsV1,
        event: cupertino_keyboard_panels_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use cupertino_keyboard_panels_v1::Event as E;
        let told = match event {
            E::SourceEntry { id, name } => format!("source {id} {name}"),
            E::SourcesChanged { selected, .. } => format!("sources_changed {selected}"),
            E::SourcesClosed => "sources_closed".to_owned(),
            E::AccentEntry { text } => format!("accent {text}"),
            E::AccentsChanged {
                serial, focused, ..
            } => format!("accents_changed {serial} {focused}"),
            E::AccentsClosed { serial } => format!("accents_closed {serial}"),
            _ => "another".to_owned(),
        };
        state.panels.push(told);
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for Client {
    fn event(
        _: &mut Self,
        surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, .. } = event {
            surface.ack_configure(serial);
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Client {
    fn event(
        _: &mut Self,
        base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
    fn event(
        state: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            state.configured = true;
        }
    }
}

impl Dispatch<ZwpTextInputV3, ()> for Client {
    fn event(
        state: &mut Self,
        _: &ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_text_input_v3::Event::Enter { .. } => state.entered = true,
            zwp_text_input_v3::Event::Leave { .. } => state.entered = false,
            zwp_text_input_v3::Event::CommitString { text } => {
                state.committed.push(text.unwrap_or_default());
            }
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(Client: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(Client: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(Client: ignore wl_output::WlOutput);
wayland_client::delegate_noop!(Client: ignore wl_seat::WlSeat);
wayland_client::delegate_noop!(Client: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(Client: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(Client: ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(Client: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
wayland_client::delegate_noop!(Client: ignore CupertinoShellV1);
wayland_client::delegate_noop!(Client: ignore xdg_toplevel::XdgToplevel);
wayland_client::delegate_noop!(Client: ignore ZwpTextInputManagerV3);

/// One client connection of a test: its queue and its state.
struct Side<T> {
    conn: Connection,
    queue: EventQueue<T>,
    client: T,
}

impl<T: 'static> Side<T> {
    fn new(conn: Connection, client: T) -> Self
    where
        T: Dispatch<wl_registry::WlRegistry, ()>,
    {
        let queue = conn.new_event_queue();
        let _registry = conn.display().get_registry(&queue.handle(), ());
        Self {
            conn,
            queue,
            client,
        }
    }

    /// Send what the client queued and read what the server answered.
    fn exchange(&mut self) {
        let _ = self.conn.flush();
        if let Some(guard) = self.conn.prepare_read() {
            let _ = guard.read();
        }
        let _ = self.queue.dispatch_pending(&mut self.client);
    }
}

/// A side the rig pumps along with the compositor.
trait Exchange {
    fn exchange(&mut self);
}

impl<T: 'static> Exchange for Side<T> {
    fn exchange(&mut self) {
        Side::exchange(self);
    }
}

/// The compositor drawing one offscreen output, with two input sources, \`us\` and \`de\`, and an accent table that
/// offers three candidates for \`e\`.
struct Rig {
    event_loop: EventLoop<'static, Cupertino>,
    state: Cupertino,
}

impl Rig {
    fn new(name: &str) -> Self {
        // the software rasteriser, as the program tests run the headless server
        std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
        let dir =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("keyboard-panels-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        let table = dir.join("accents.json");
        std::fs::write(&table, ACCENT_TABLE).unwrap();
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let mut config = Config::default();
        config.shell_processes.clear();
        config.keyboard.sources = ["us", "de"]
            .map(|s| SourceSpec::parse(s).expect("an xkb layout"))
            .to_vec();
        config.keyboard.accent.table = Some(table);
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            false,
            &Assets::builtin(),
        )
        .unwrap();
        headless::init(&mut state, OUTPUT_SIZE, 1.0).expect("an offscreen renderer");
        assert!(
            state.connect_input_method(),
            "the server's input method connects"
        );
        Self { event_loop, state }
    }

    /// One turn of the compositor's loop, a frame drawn when one is due.
    fn turn(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(2),
        )
        .unwrap();
    }

    /// A few turns, each with the exchange of every side in \`sides\`.
    fn settle(&mut self, sides: &mut [&mut dyn Exchange]) {
        for _ in 0..8 {
            for side in sides.iter_mut() {
                side.exchange();
            }
            self.turn();
            for side in sides.iter_mut() {
                side.exchange();
            }
        }
    }

    /// The output as it shows now, without the pointer.
    fn image(&mut self) -> image::RgbaImage {
        let mut backend = self.state.backend.take().unwrap();
        let image = self
            .state
            .draw_output_image(backend.renderer(), 0, false, false)
            .unwrap();
        self.state.backend = Some(backend);
        image
    }

    /// A shared host running \`components\`, and the connection of each.
    fn components(&mut self, components: &[&'static str]) -> (Side<HostClient>, Vec<Side<Client>>) {
        let fd = self
            .state
            .attach_host_connection(
                HostKind::Shared,
                components.to_vec(),
                Holder::Detached(Instant::now() + DETACHED_LIFETIME),
            )
            .unwrap();
        let mut host = Side::new(
            Connection::from_socket(UnixStream::from(fd)).unwrap(),
            HostClient::default(),
        );
        self.settle(&mut [&mut host]);
        let qh = host.queue.handle();
        let global = host.client.host.clone().expect("the host global");
        for name in components {
            global
                .get_component((*name).into(), &qh, (*name).to_string())
                .connect();
        }
        self.settle(&mut [&mut host]);
        let mut sides = Vec::new();
        for name in components {
            let at = host
                .client
                .connections
                .iter()
                .position(|(n, _)| n == name)
                .expect("a connection for each component");
            let (_, fd) = host.client.connections.remove(at);
            let mut side = Side::new(
                Connection::from_socket(UnixStream::from(fd)).unwrap(),
                Client::default(),
            );
            self.settle(&mut [&mut side]);
            sides.push(side);
        }
        (host, sides)
    }

    /// The keyboard's panels of a component's connection.
    fn panels(&mut self, side: &mut Side<Client>) -> CupertinoKeyboardPanelsV1 {
        let qh = side.queue.handle();
        let panels = side
            .client
            .shell
            .as_ref()
            .expect("the shell global")
            .get_keyboard_panels(&qh, ());
        self.settle(&mut [side]);
        panels
    }

    /// An ordinary client's window, mapped, given the keyboard, with its text input enabled at a caret.
    fn text_window(&mut self) -> (Side<Client>, ZwpTextInputV3) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        insert_client(&self.state.display_handle, ours, ClientState::default()).unwrap();
        let mut side = Side::new(Connection::from_socket(theirs).unwrap(), Client::default());
        self.settle(&mut [&mut side]);
        let qh = side.queue.handle();
        let c = &side.client;
        let surface = c.compositor.as_ref().unwrap().create_surface(&qh, ());
        let xdg = c
            .wm_base
            .as_ref()
            .unwrap()
            .get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg.get_toplevel(&qh, ());
        toplevel.set_title("keyboard panels".into());
        toplevel.set_app_id("org.cupertino.test.text".into());
        let text_input = c
            .text_inputs
            .as_ref()
            .expect("the text-input manager is offered to applications")
            .get_text_input(c.seat.as_ref().unwrap(), &qh, ());
        surface.commit();
        self.settle(&mut [&mut side]);
        assert!(side.client.configured, "the window is configured");
        let size = (300, 200);
        let buffer = shm_buffer(side.client.shm.as_ref().unwrap(), &qh, size);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, size.0, size.1);
        surface.commit();
        let deadline = Instant::now() + WAIT;
        while !side.client.entered {
            assert!(
                Instant::now() < deadline,
                "the window's text input gets the focus"
            );
            self.settle(&mut [&mut side]);
        }
        text_input.enable();
        text_input.set_surrounding_text("e".into(), 1, 1);
        text_input.set_cursor_rectangle(40, 40, 2, 18);
        text_input.commit();
        let deadline = Instant::now() + WAIT;
        while !self.state.overlays.accent.bridge {
            assert!(
                Instant::now() < deadline,
                "the server's input method hears the field"
            );
            self.settle(&mut [&mut side]);
        }
        (side, text_input)
    }

    /// A layer surface of \`side\` with \`namespace\`, \`size\` points on the overlay layer at the output's top
    /// left or top right, showing a buffer.
    fn layer(
        &mut self,
        side: &mut Side<Client>,
        namespace: &str,
        size: (i32, i32),
        right: bool,
    ) -> (
        wl_surface::WlSurface,
        zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    ) {
        let qh = side.queue.handle();
        let c = &side.client;
        let surface = c.compositor.as_ref().unwrap().create_surface(&qh, ());
        let layer = c.layer_shell.as_ref().unwrap().get_layer_surface(
            &surface,
            c.output.as_ref(),
            zwlr_layer_shell_v1::Layer::Overlay,
            namespace.into(),
            &qh,
            (),
        );
        layer.set_size(size.0 as u32, size.1 as u32);
        let side_anchor = if right {
            zwlr_layer_surface_v1::Anchor::Right
        } else {
            zwlr_layer_surface_v1::Anchor::Left
        };
        layer.set_anchor(zwlr_layer_surface_v1::Anchor::Top | side_anchor);
        surface.commit();
        self.settle(&mut [side]);
        let buffer = shm_buffer(side.client.shm.as_ref().unwrap(), &qh, size);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, size.0, size.1);
        surface.commit();
        self.settle(&mut [side]);
        (surface, layer)
    }
}

/// A \`width\` × \`height\` buffer of opaque grey.
fn shm_buffer<D>(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<D>,
    (width, height): (i32, i32),
) -> wl_buffer::WlBuffer
where
    D: Dispatch<wl_shm_pool::WlShmPool, ()> + Dispatch<wl_buffer::WlBuffer, ()> + 'static,
{
    let len = width * height * 4;
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    file.write_all(&vec![0x80; len as usize]).unwrap();
    let pool = shm.create_pool(file.as_fd(), len, qh, ());
    let buffer = pool.create_buffer(
        0,
        width,
        height,
        width * 4,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );
    pool.destroy();
    buffer
}

/// The protocol error the server ended \`side\` with: its interface and code.
fn protocol_error(side: &Side<Client>) -> Option<(String, u32)> {
    side.conn
        .protocol_error()
        .map(|e| (e.object_interface, e.code))
}

/// A real key going down or up, as a device delivers it.
fn key(rig: &mut Rig, evdev: u32, pressed: bool) {
    let state = if pressed {
        KeyState::Pressed
    } else {
        KeyState::Released
    };
    let time = rig.state.now() as u32;
    rig.state
        .key_after_delays(Keycode::new(evdev + 8), state, time);
}

#[test]
fn a_connection_without_the_role_gets_the_permission_error() {
    let mut rig = Rig::new("permission");
    let (mut host, mut sides) = rig.components(&["dock"]);
    let mut dock = sides.pop().unwrap();
    let _panels = rig.panels(&mut dock);
    rig.settle(&mut [&mut host, &mut dock]);
    assert_eq!(
        protocol_error(&dock),
        Some((
            "cupertino_shell_v1".to_owned(),
            cupertino_shell_v1::Error::Permission as u32
        ))
    );
    assert!(
        !rig.state.overlays.panels.bound(),
        "the dock's object draws nothing"
    );
}

#[test]
fn a_second_object_is_the_role_error() {
    let mut rig = Rig::new("role");
    let (mut host, mut sides) = rig.components(&["osd"]);
    let mut osd = sides.pop().unwrap();
    let _first = rig.panels(&mut osd);
    assert_eq!(
        protocol_error(&osd),
        None,
        "the on-screen displays hold the role"
    );
    assert!(rig.state.overlays.panels.bound());
    let _second = rig.panels(&mut osd);
    rig.settle(&mut [&mut host, &mut osd]);
    assert_eq!(
        protocol_error(&osd),
        Some((
            "cupertino_shell_v1".to_owned(),
            cupertino_shell_v1::Error::Role as u32
        ))
    );
}

#[test]
fn a_pick_of_another_picker_or_past_the_list_inserts_nothing() {
    let mut rig = Rig::new("pick");
    let (mut host, mut sides) = rig.components(&["osd"]);
    let mut osd = sides.pop().unwrap();
    let panels = rig.panels(&mut osd);
    let (mut window, _text_input) = rig.text_window();

    assert!(rig.state.accent_open('e'), "the picker opens for e");
    rig.settle(&mut [&mut host, &mut osd, &mut window]);
    let serial = rig
        .state
        .overlays
        .panels
        .open_serial()
        .expect("the panel was told");
    assert_eq!(
        osd.client.panels,
        [
            "accent è".to_owned(),
            "accent é".to_owned(),
            "accent ê".to_owned(),
            format!("accents_changed {serial} -1"),
        ]
    );

    // a picker that is not the open one, and a candidate past the list
    panels.accent_pick(serial.wrapping_add(1), 1);
    panels.accent_pick(serial.wrapping_sub(1), 0);
    panels.accent_pick(serial, 3);
    panels.accent_pick(serial, u32::MAX);
    rig.settle(&mut [&mut host, &mut osd, &mut window]);
    assert!(
        window.client.committed.is_empty(),
        "nothing was inserted: {:?}",
        window.client.committed
    );
    assert!(rig.state.overlays.accent.is_open(), "the picker stays open");
    assert_eq!(rig.state.overlays.panels.open_serial(), Some(serial));
    assert_eq!(protocol_error(&osd), None);

    // the open picker's second candidate goes into the field
    panels.accent_pick(serial, 1);
    rig.settle(&mut [&mut host, &mut osd, &mut window]);
    assert_eq!(window.client.committed, ["é"]);
    assert!(!rig.state.overlays.accent.is_open());
    assert_eq!(
        osd.client.panels.last(),
        Some(&format!("accents_closed {serial}"))
    );

    // once the picker closed, its serial picks nothing
    panels.accent_pick(serial, 0);
    rig.settle(&mut [&mut host, &mut osd, &mut window]);
    assert_eq!(window.client.committed, ["é"]);
}

#[test]
fn the_panels_connection_gets_no_key_of_a_switch() {
    let mut rig = Rig::new("keys");
    let (mut host, mut sides) = rig.components(&["osd"]);
    let mut osd = sides.pop().unwrap();
    let _panels = rig.panels(&mut osd);
    let (mut window, _text_input) = rig.text_window();
    assert_eq!(rig.state.overlays.inputs.current(), Some("us"));
    window.client.keys.clear();

    // Control held, the space bar pressed and let go: the switch opens on the panel; Control up commits it
    key(&mut rig, KEY_CTRL, true);
    key(&mut rig, KEY_SPACE, true);
    key(&mut rig, KEY_SPACE, false);
    rig.settle(&mut [&mut host, &mut osd, &mut window]);
    assert!(
        rig.state.overlays.inputs.switch_open(),
        "the switch is open while Control is held"
    );
    let opened = osd.client.panels.clone();
    assert!(
        opened.first().is_some_and(|e| e.starts_with("source us "))
            && opened.get(1).is_some_and(|e| e.starts_with("source de "))
            && opened.get(2).map(String::as_str) == Some("sources_changed 1"),
        "the list, the source to switch to selected: {opened:?}"
    );
    key(&mut rig, KEY_CTRL, false);
    rig.settle(&mut [&mut host, &mut osd, &mut window]);
    assert_eq!(
        osd.client.panels.last().map(String::as_str),
        Some("sources_closed")
    );
    assert_eq!(
        rig.state.overlays.inputs.current(),
        Some("de"),
        "the release switched the source"
    );

    assert!(
        osd.client.keys.is_empty(),
        "the panels' connection got keys: {:?}",
        osd.client.keys
    );
    assert!(
        !window.client.keys.contains(&KEY_SPACE),
        "the chord's key reached the window: {:?}",
        window.client.keys
    );
}

#[test]
fn the_input_method_global_is_offered_to_no_component() {
    let mut rig = Rig::new("input-method");
    let (_host, sides) = rig.components(&["osd", "dock"]);
    let (window, _text_input) = rig.text_window();
    for side in sides.iter().chain([&window]) {
        assert!(
            !side
                .client
                .globals
                .iter()
                .any(|g| g == "zwp_input_method_manager_v2"),
            "offered: {:?}",
            side.client.globals
        );
    }
    assert!(
        window
            .client
            .globals
            .iter()
            .any(|g| g == "zwp_text_input_manager_v3"),
        "the text-input manager is the applications'"
    );
}

#[test]
fn a_surface_of_the_panels_namespace_without_the_role_is_no_panel() {
    let mut rig = Rig::new("namespace");
    let (mut host, mut sides) = rig.components(&["osd", "dock"]);
    let mut dock = sides.pop().unwrap();
    let mut osd = sides.pop().unwrap();
    let _panels = rig.panels(&mut osd);
    let size = (120, 60);
    let _fake = rig.layer(&mut dock, ACCENTS_NAMESPACE, size, false);
    let _real = rig.layer(&mut osd, ACCENTS_NAMESPACE, size, true);
    let on_fake = (f64::from(size.0) / 2.0, f64::from(size.1) / 2.0);
    let on_real = (
        f64::from(OUTPUT_SIZE.0 - size.0 / 2),
        f64::from(size.1) / 2.0,
    );
    const BTN_LEFT: u32 = 0x110;

    assert!(rig.state.accent_open('e'));
    rig.settle(&mut [&mut host, &mut osd, &mut dock]);
    let serial = rig.state.overlays.panels.open_serial().unwrap();
    // a press on the panel of the connection that holds the role leaves the picker open
    rig.state.scripted_click(on_real, BTN_LEFT);
    rig.settle(&mut [&mut host, &mut osd, &mut dock]);
    assert!(
        rig.state.overlays.accent.is_open(),
        "a press on the picker's panel keeps it"
    );
    // a press on the dock's surface of the same namespace closes it
    rig.state.scripted_click(on_fake, BTN_LEFT);
    rig.settle(&mut [&mut host, &mut osd, &mut dock]);
    assert!(
        !rig.state.overlays.accent.is_open(),
        "the dock's surface is no panel"
    );
    assert_eq!(
        osd.client.panels.last(),
        Some(&format!("accents_closed {serial}"))
    );
}

#[test]
fn the_server_draws_no_panel() {
    let mut rig = Rig::new("draws");
    rig.state.overlays.hold_modifiers = true;
    rig.settle(&mut []);
    let bare = rig.image();

    // without the on-screen displays: a switch shows nothing, and a held letter arms no picker
    rig.state.input_source_step(false, Step::Previous);
    rig.settle(&mut []);
    assert!(rig.state.overlays.inputs.switch_open());
    assert!(rig.image() == bare, "the server drew the switcher");
    assert!(!rig.state.accent_open('e'), "no picker without its panel");
    rig.state.input_sources_key(0xff1b);
    rig.settle(&mut []);

    // with a panels' object that draws nothing: the switch and the picker are open, and the output is unchanged
    let (mut host, mut sides) = rig.components(&["osd"]);
    let mut osd = sides.pop().unwrap();
    let _panels = rig.panels(&mut osd);
    rig.state.input_source_step(false, Step::Previous);
    assert!(rig.state.accent_open('e'));
    rig.settle(&mut [&mut host, &mut osd]);
    assert!(rig.state.overlays.inputs.switch_open() && rig.state.overlays.accent.is_open());
    assert!(osd
        .client
        .panels
        .iter()
        .any(|e| e.starts_with("sources_changed")));
    assert!(osd
        .client
        .panels
        .iter()
        .any(|e| e.starts_with("accents_changed")));
    assert!(rig.image() == bare, "the server drew a panel");
}
