//! Program and configuration tests for W5.5:
//! - `shell` section of `compositor.json`: `components`, `off`, `alone`, system-only `components`, login session
//!   empty components, a `processes` list or the section as a list ignored with a warning, and `BuiltIn::Login` (user +
//!   greeter).
//! - `cupertino_overview_manager_v1` / `cupertino_overview_v1`: receives initial `area`, `workspace`,
//!   `done`; handles `add_workspace`, `switch_workspace`, `remove_workspace`, and `toggle` delivery.

use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_defaults::compositor::CompositorSettings;
use cupertino_protocols::client::host::cupertino_host_component_v1::{
    self, CupertinoHostComponentV1,
};
use cupertino_protocols::client::host::cupertino_host_v1::{self, CupertinoHostV1};
use cupertino_protocols::client::overview::cupertino_overview_manager_v1::{
    self, CupertinoOverviewManagerV1,
};
use cupertino_protocols::client::overview::cupertino_overview_v1::{
    self, CupertinoOverviewV1, Mode,
};
use cupertino_server::authorization::{Holder, ShellRole, DETACHED_LIFETIME};
use cupertino_server::config::{BuiltIn, LayerTrust};
use cupertino_server::host::HostKind;
use cupertino_server::state::new_output;
use cupertino_server::{Config, Cupertino};
use smithay::output::{Mode as OutputMode, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::Transform;
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum};

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
            name,
            interface,
            version,
        } = event
        {
            if interface == "cupertino_host_v1" {
                state.host = Some(reg.bind::<CupertinoHostV1, _, _>(name, version.min(1), qh, ()));
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

#[derive(Default)]
struct OverviewClient {
    output: Option<wl_output::WlOutput>,
    manager: Option<CupertinoOverviewManagerV1>,
    areas: Vec<(i32, i32, i32, i32)>,
    workspaces: Vec<(u32, u32)>,
    toggles: Vec<(Mode, String)>,
    done_count: usize,
}

impl Dispatch<wl_registry::WlRegistry, ()> for OverviewClient {
    fn event(
        state: &mut Self,
        reg: &wl_registry::WlRegistry,
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
                "wl_output" => {
                    state.output =
                        Some(reg.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ()));
                }
                "cupertino_overview_manager_v1" => {
                    state.manager = Some(reg.bind::<CupertinoOverviewManagerV1, _, _>(
                        name,
                        version.min(1),
                        qh,
                        (),
                    ));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for OverviewClient {
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

impl Dispatch<CupertinoOverviewManagerV1, ()> for OverviewClient {
    fn event(
        _: &mut Self,
        _: &CupertinoOverviewManagerV1,
        _: cupertino_overview_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<CupertinoOverviewV1, ()> for OverviewClient {
    fn event(
        state: &mut Self,
        _: &CupertinoOverviewV1,
        event: cupertino_overview_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_overview_v1::Event::Area {
                x,
                y,
                width,
                height,
            } => state.areas.push((x, y, width, height)),
            cupertino_overview_v1::Event::Workspace {
                id,
                state: WEnum::Value(bits),
            } => state.workspaces.push((id, bits.bits())),
            cupertino_overview_v1::Event::Toggle {
                mode: WEnum::Value(m),
                app_id,
            } => state.toggles.push((m, app_id)),
            cupertino_overview_v1::Event::Done => state.done_count += 1,
            _ => {}
        }
    }
}

#[test]
fn shell_section_rules_and_builtin_login() {
    // System file sets components, off, alone.
    let (sys, errs) = CompositorSettings::parse(
        r#"{"shell": {"components": ["menubar", "dock", "overview", "bogus"], "off": ["dock"], "alone": ["overview"]}}"#,
        None,
    );
    assert!(errs.is_empty(), "{errs:?}");
    let mut cfg = Config::default();
    let problems = cfg.apply_privileged(&sys, LayerTrust::System);
    assert!(problems.iter().any(|p| p.contains("bogus")));
    assert_eq!(
        cfg.active_shell_components(false),
        vec!["menubar", "overview"]
    );
    assert_eq!(cfg.shared_host_components(false), vec!["menubar"]);
    assert_eq!(cfg.alone_host_components(false), vec!["overview"]);
    // Login session always runs zero components.
    assert!(cfg.active_shell_components(true).is_empty());

    // User file cannot override `components`, but may set `off` and `alone`.
    let (user, errs) = CompositorSettings::parse(
        r#"{"shell": {"components": ["dock"], "off": [], "alone": ["menubar"]}}"#,
        None,
    );
    assert!(errs.is_empty());
    let uprobs = cfg.apply_privileged(&user, LayerTrust::User);
    assert!(uprobs.iter().any(|p| p.contains("shell.components")));
    assert_eq!(
        cfg.active_shell_components(false),
        vec!["menubar", "dock", "overview"]
    );
    assert_eq!(cfg.alone_host_components(false), vec!["menubar"]);

    // A list of processes, beside the components or as the whole section, is ignored with one warning: nothing of it
    // runs, and the server starts the login itself.
    for text in [
        r#"{"shell": [{"name": "login", "command": ["/usr/bin/cupertino-login"], "roles": ["session", "session_lock"]}]}"#,
        r#"{"shell": {"components": ["menubar", "dock", "overview"], "processes": [{"name": "bar", "command": ["/usr/bin/bar"]}]}}"#,
    ] {
        let (legacy, errs) = CompositorSettings::parse(text, None);
        assert!(errs.is_empty(), "{text}: {errs:?}");
        let lprobs = cfg.apply_privileged(&legacy, LayerTrust::System);
        assert_eq!(
            lprobs
                .iter()
                .filter(|p| p.starts_with("shell.processes: ignored"))
                .count(),
            1,
            "{text}: {lprobs:?}"
        );
        assert_eq!(
            cfg.active_shell_components(false),
            vec!["menubar", "dock", "overview"],
            "{text}"
        );
    }

    let user_login = BuiltIn::Login.process(std::path::Path::new("/usr/bin/cupertino-login"));
    assert!(user_login.roles.contains(&ShellRole::SessionLock));
    assert!(!user_login.roles.contains(&ShellRole::SessionAgent));
    let greeter = BuiltIn::login_greeter_process(std::path::Path::new("/usr/bin/cupertino-login"));
    assert_eq!(greeter.roles, vec![ShellRole::Session]);
    assert!(greeter.command.iter().any(|a| a == "--greeter"));
}

#[test]
fn overview_v1_delivers_area_workspaces_and_toggle() {
    let mut event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
    let display: Display<Cupertino> = Display::new().unwrap();
    let config = Config::default();
    let mut state = Cupertino::new(
        display,
        event_loop.handle(),
        event_loop.get_signal(),
        config,
        false,
        &Assets::builtin(),
    )
    .unwrap();
    let output_id = state.add_output(new_output(
        "HEADLESS-1",
        PhysicalProperties {
            size: (400, 300).into(),
            subpixel: Subpixel::None,
            make: "Cupertino".into(),
            model: "Test".into(),
        },
        OutputMode {
            size: (1280, 800).into(),
            refresh: 60_000,
        },
        1.0,
        Transform::Normal,
        (0, 0),
    ));

    let pump = |el: &mut EventLoop<'static, Cupertino>,
                st: &mut Cupertino,
                conn: &Connection,
                q: &mut EventQueue<HostClient>,
                hc: &mut HostClient| {
        for _ in 0..6 {
            let _ = conn.flush();
            let _ = el.dispatch(Some(Duration::from_millis(2)), st);
            st.tick();
            let _ = st.display_handle.flush_clients();
            if let Some(g) = conn.prepare_read() {
                let _ = g.read();
            }
            let _ = q.dispatch_pending(hc);
        }
    };

    let host_fd = state
        .attach_host_connection(
            HostKind::Shared,
            vec!["overview"],
            Holder::Detached(Instant::now() + DETACHED_LIFETIME),
        )
        .unwrap();
    let hconn = Connection::from_socket(UnixStream::from(host_fd)).unwrap();
    let mut hq: EventQueue<HostClient> = hconn.new_event_queue();
    let hqh = hq.handle();
    let mut hst = HostClient::default();
    let _reg = hconn.display().get_registry(&hqh, ());
    pump(&mut event_loop, &mut state, &hconn, &mut hq, &mut hst);

    let comp = hst
        .host
        .as_ref()
        .unwrap()
        .get_component("overview".into(), &hqh, "overview".into());
    comp.connect();
    pump(&mut event_loop, &mut state, &hconn, &mut hq, &mut hst);
    let (_, fd) = hst.connections.remove(0);

    let oconn = Connection::from_socket(UnixStream::from(fd)).unwrap();
    let mut oq: EventQueue<OverviewClient> = oconn.new_event_queue();
    let oqh = oq.handle();
    let mut ost = OverviewClient::default();
    let _oreg = oconn.display().get_registry(&oqh, ());
    for _ in 0..8 {
        let _ = oconn.flush();
        let _ = event_loop.dispatch(Some(Duration::from_millis(2)), &mut state);
        state.tick();
        let _ = state.display_handle.flush_clients();
        if let Some(g) = oconn.prepare_read() {
            let _ = g.read();
        }
        let _ = oq.dispatch_pending(&mut ost);
    }

    let ov = ost
        .manager
        .as_ref()
        .unwrap()
        .get_overview(ost.output.as_ref().unwrap(), &oqh, ());
    for _ in 0..6 {
        let _ = oconn.flush();
        let _ = event_loop.dispatch(Some(Duration::from_millis(2)), &mut state);
        state.tick();
        let _ = state.display_handle.flush_clients();
        if let Some(g) = oconn.prepare_read() {
            let _ = g.read();
        }
        let _ = oq.dispatch_pending(&mut ost);
    }

    assert_eq!(ost.areas, vec![(0, 0, 1280, 800)]);
    assert_eq!(ost.workspaces.len(), 1);
    assert!(ost.done_count >= 1);

    // Add a workspace and trigger toggle.
    ov.add_workspace();
    assert!(state.toggle_overview_v1(
        output_id,
        cupertino_protocols::server::overview::cupertino_overview_v1::Mode::All,
        ""
    ));
    for _ in 0..6 {
        let _ = oconn.flush();
        let _ = event_loop.dispatch(Some(Duration::from_millis(2)), &mut state);
        state.tick();
        let _ = state.display_handle.flush_clients();
        if let Some(g) = oconn.prepare_read() {
            let _ = g.read();
        }
        let _ = oq.dispatch_pending(&mut ost);
    }
    assert_eq!(state.wm.workspaces(output_id).len(), 2);
    assert_eq!(ost.toggles, vec![(Mode::All, String::new())]);
}
