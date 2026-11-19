//! Program tests for W5.3 (`cupertino_transaction_manager_v1`, `cupertino_transaction_v1`, `cupertino_hole_v1`):
//! - Two participants (`dock` + `menu_bar`) start on the same presented frame timestamp.
//! - A late participant joins in step at the original start timestamp after the deadline passed.
//! - A gesture-bound transaction (`overview`) tracks progress and settles with the release velocity (and supports
//!   reverse gestures).
//! - Window/workspace holes (`get_hole`) stack claims in commit order, fall back on destroy, and emit `source_closed`.

use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_protocols::client::host::cupertino_component_v1::{self, CupertinoComponentV1};
use cupertino_protocols::client::host::cupertino_host_component_v1::{
    self, CupertinoHostComponentV1,
};
use cupertino_protocols::client::host::cupertino_host_v1::{self, CupertinoHostV1};
use cupertino_protocols::client::layers::cupertino_layer_v1::{self, CupertinoLayerV1};
use cupertino_protocols::client::layers::cupertino_layers_v1::{self, CupertinoLayersV1};
use cupertino_protocols::client::transaction::cupertino_hole_v1::{
    self, CupertinoHoleV1, Source as HoleWireSource,
};
use cupertino_protocols::client::transaction::cupertino_transaction_manager_v1::{
    self, CupertinoTransactionManagerV1,
};
use cupertino_protocols::client::transaction::cupertino_transaction_v1::{
    self, CupertinoTransactionV1, Gesture, Outcome,
};
use cupertino_server::authorization::{Holder, DETACHED_LIFETIME};
use cupertino_server::host::HostKind;
use cupertino_server::state::new_output;
use cupertino_server::{Config, Cupertino};
use cupertino_wm::{OutputId, WindowId};
use smithay::output::{Mode as OutputMode, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::Transform;
use wayland_client::protocol::{wl_compositor, wl_output, wl_registry, wl_surface};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

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
struct CompClient {
    compositor: Option<wl_compositor::WlCompositor>,
    output: Option<wl_output::WlOutput>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    layers: Option<CupertinoLayersV1>,
    component_global: Option<CupertinoComponentV1>,
    tx_manager: Option<CupertinoTransactionManagerV1>,
    instance: Option<u32>,
    started: Vec<(u32, u32, u32)>,
    gesture_began: Vec<Gesture>,
    gesture_ended: Vec<Outcome>,
    finished: Vec<Outcome>,
    source_closed: usize,
}

impl Dispatch<wl_registry::WlRegistry, ()> for CompClient {
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
                "wl_compositor" => {
                    state.compositor = Some(reg.bind::<wl_compositor::WlCompositor, _, _>(
                        name,
                        version.min(6),
                        qh,
                        (),
                    ));
                }
                "wl_output" => {
                    state.output =
                        Some(reg.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ()));
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell =
                        Some(reg.bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1, _, _>(
                            name,
                            version.min(4),
                            qh,
                            (),
                        ));
                }
                "cupertino_layers_v1" => {
                    state.layers =
                        Some(reg.bind::<CupertinoLayersV1, _, _>(name, version.min(6), qh, ()));
                }
                "cupertino_component_v1" => {
                    state.component_global =
                        Some(reg.bind::<CupertinoComponentV1, _, _>(name, version.min(1), qh, ()));
                }
                "cupertino_transaction_manager_v1" => {
                    state.tx_manager = Some(reg.bind::<CupertinoTransactionManagerV1, _, _>(
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

impl Dispatch<wl_compositor::WlCompositor, ()> for CompClient {
    fn event(
        _: &mut Self,
        _: &wl_compositor::WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, ()> for CompClient {
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

impl Dispatch<wl_surface::WlSurface, ()> for CompClient {
    fn event(
        _: &mut Self,
        _: &wl_surface::WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for CompClient {
    fn event(
        _: &mut Self,
        _: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        _: zwlr_layer_shell_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for CompClient {
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

impl Dispatch<CupertinoLayersV1, ()> for CompClient {
    fn event(
        _: &mut Self,
        _: &CupertinoLayersV1,
        _: cupertino_layers_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<CupertinoLayerV1, ()> for CompClient {
    fn event(
        _: &mut Self,
        _: &CupertinoLayerV1,
        _: cupertino_layer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<CupertinoComponentV1, ()> for CompClient {
    fn event(
        state: &mut Self,
        _: &CupertinoComponentV1,
        event: cupertino_component_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let cupertino_component_v1::Event::Component { instance, .. } = event {
            state.instance = Some(instance);
        }
    }
}

impl Dispatch<CupertinoTransactionManagerV1, ()> for CompClient {
    fn event(
        _: &mut Self,
        _: &CupertinoTransactionManagerV1,
        _: cupertino_transaction_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<CupertinoTransactionV1, ()> for CompClient {
    fn event(
        state: &mut Self,
        _: &CupertinoTransactionV1,
        event: cupertino_transaction_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_transaction_v1::Event::Started {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            } => state.started.push((tv_sec_hi, tv_sec_lo, tv_nsec)),
            cupertino_transaction_v1::Event::GestureBegan {
                gesture: WEnum::Value(g),
            } => state.gesture_began.push(g),
            cupertino_transaction_v1::Event::GestureEnded {
                outcome: WEnum::Value(o),
            } => state.gesture_ended.push(o),
            cupertino_transaction_v1::Event::Finished {
                outcome: WEnum::Value(o),
            } => state.finished.push(o),
            _ => {}
        }
    }
}

impl Dispatch<CupertinoHoleV1, ()> for CompClient {
    fn event(
        state: &mut Self,
        _: &CupertinoHoleV1,
        event: cupertino_hole_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let cupertino_hole_v1::Event::SourceClosed = event {
            state.source_closed += 1;
        }
    }
}

struct Rig {
    event_loop: EventLoop<'static, Cupertino>,
    state: Cupertino,
    output_id: OutputId,
}

impl Rig {
    fn new() -> Self {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let mut config = Config::default();
        config.shell_processes.clear();
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            false,
            &Assets::builtin(),
        )
        .unwrap();
        let output = new_output(
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
        );
        let output_id = state.add_output(output);
        Self {
            event_loop,
            state,
            output_id,
        }
    }

    fn pump(&mut self) {
        let _ = self
            .event_loop
            .dispatch(Some(Duration::from_millis(2)), &mut self.state);
        self.state.tick();
        let _ = self.state.display_handle.flush_clients();
    }

    fn connect_host(
        &mut self,
        components: &[&'static str],
    ) -> (Connection, EventQueue<HostClient>, HostClient) {
        let host_fd = self
            .state
            .attach_host_connection(
                HostKind::Shared,
                components.to_vec(),
                Holder::Detached(Instant::now() + DETACHED_LIFETIME),
            )
            .unwrap();
        let conn = Connection::from_socket(UnixStream::from(host_fd)).unwrap();
        let mut queue: EventQueue<HostClient> = conn.new_event_queue();
        let qh = queue.handle();
        let mut client = HostClient::default();
        let _reg = conn.display().get_registry(&qh, ());
        for _ in 0..8 {
            let _ = conn.flush();
            self.pump();
            let _ = queue.dispatch_pending(&mut client);
            if let Some(guard) = conn.prepare_read() {
                let _ = guard.read();
            }
            let _ = queue.dispatch_pending(&mut client);
            if client.host.is_some() {
                break;
            }
        }
        (conn, queue, client)
    }

    fn spawn_component(
        &mut self,
        host_conn: &Connection,
        host_queue: &mut EventQueue<HostClient>,
        host_client: &mut HostClient,
        name: &str,
    ) -> (
        CupertinoHostComponentV1,
        Connection,
        EventQueue<CompClient>,
        CompClient,
    ) {
        let qh = host_queue.handle();
        let handle =
            host_client
                .host
                .as_ref()
                .unwrap()
                .get_component(name.into(), &qh, name.to_string());
        handle.connect();
        for _ in 0..8 {
            let _ = host_conn.flush();
            self.pump();
            if let Some(guard) = host_conn.prepare_read() {
                let _ = guard.read();
            }
            let _ = host_queue.dispatch_pending(host_client);
            if host_client.connections.iter().any(|(n, _)| n == name) {
                break;
            }
        }
        let pos = host_client
            .connections
            .iter()
            .position(|(n, _)| n == name)
            .expect("host should receive component connection fd");
        let (_, fd) = host_client.connections.remove(pos);
        let stream: UnixStream = fd.into();
        let conn = Connection::from_socket(stream).unwrap();
        let mut queue: EventQueue<CompClient> = conn.new_event_queue();
        let cqh = queue.handle();
        let mut client = CompClient::default();
        let _reg = conn.display().get_registry(&cqh, ());
        for _ in 0..10 {
            let _ = conn.flush();
            self.pump();
            if let Some(guard) = conn.prepare_read() {
                let _ = guard.read();
            }
            let _ = queue.dispatch_pending(&mut client);
            if client.tx_manager.is_some()
                && client.output.is_some()
                && client.instance.is_some()
                && client.layer_shell.is_some()
            {
                break;
            }
        }
        assert!(
            client.tx_manager.is_some(),
            "{name} should bind transaction manager"
        );
        assert!(client.output.is_some(), "{name} should bind wl_output");
        (handle, conn, queue, client)
    }

    fn pump_comp(
        &mut self,
        conn: &Connection,
        queue: &mut EventQueue<CompClient>,
        state: &mut CompClient,
    ) {
        let _ = conn.flush();
        self.pump();
        if let Some(guard) = conn.prepare_read() {
            let _ = guard.read();
        }
        let _ = queue.dispatch_pending(state);
    }

    fn create_layer_surface(
        &mut self,
        conn: &Connection,
        queue: &mut EventQueue<CompClient>,
        state: &mut CompClient,
        namespace: &str,
    ) -> (
        wl_surface::WlSurface,
        zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    ) {
        let qh = queue.handle();
        let surf = state.compositor.as_ref().unwrap().create_surface(&qh, ());
        let layer = state.layer_shell.as_ref().unwrap().get_layer_surface(
            &surf,
            state.output.as_ref(),
            zwlr_layer_shell_v1::Layer::Top,
            namespace.into(),
            &qh,
            (),
        );
        layer.set_size(200, 40);
        surf.commit();
        for _ in 0..4 {
            self.pump_comp(conn, queue, state);
        }
        (surf, layer)
    }
}

#[test]
fn two_participants_start_on_same_presented_frame() {
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["dock", "menubar"]);
    let (_hdock, dconn, mut dq, mut dst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "dock");
    let (_hbar, bconn, mut bq, mut bst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "menubar");

    let (dock_surf, _dock_layer) = rig.create_layer_surface(&dconn, &mut dq, &mut dst, "dock");
    let (bar_surf, _bar_layer) = rig.create_layer_surface(&bconn, &mut bq, &mut bst, "menubar");

    let dock_inst = dst.instance.unwrap();
    let dqh = dq.handle();
    let bqh = bq.handle();

    // Dock creates transaction serial 1 expecting menubar and stages its surface.
    let tx_dock =
        dst.tx_manager
            .as_ref()
            .unwrap()
            .create(1, dst.output.as_ref().unwrap(), &dqh, ());
    tx_dock.expect("menubar".into());
    tx_dock.stage(&dock_surf);
    tx_dock.submit();
    rig.pump_comp(&dconn, &mut dq, &mut dst);

    // Dock has not committed its staged surface yet, and menubar hasn't joined -> not started.
    assert!(dst.started.is_empty());

    // Menu bar joins and stages its surface, commits it, and submits.
    let tx_bar = bst.tx_manager.as_ref().unwrap().join(
        "dock".into(),
        dock_inst,
        1,
        bst.output.as_ref().unwrap(),
        &bqh,
        (),
    );
    tx_bar.stage(&bar_surf);
    tx_bar.submit();
    bar_surf.commit();
    rig.pump_comp(&bconn, &mut bq, &mut bst);

    // Still waiting for dock's staged surface commit!
    assert!(dst.started.is_empty());
    assert!(bst.started.is_empty());

    // Dock commits its staged surface -> both start on the exact same presented frame timestamp.
    dock_surf.commit();
    for _ in 0..6 {
        rig.pump_comp(&dconn, &mut dq, &mut dst);
        rig.pump_comp(&bconn, &mut bq, &mut bst);
    }

    assert_eq!(dst.started.len(), 1, "dock should receive started once");
    assert_eq!(bst.started.len(), 1, "menubar should receive started once");
    assert_eq!(
        dst.started[0], bst.started[0],
        "both participants must start on the exact same presented frame timestamp"
    );
}

#[test]
fn late_participant_joins_in_step() {
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["dock", "menubar"]);
    let (_hdock, dconn, mut dq, mut dst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "dock");
    let (_hbar, bconn, mut bq, mut bst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "menubar");

    let dock_inst = dst.instance.unwrap();
    let dqh = dq.handle();
    let bqh = bq.handle();

    let tx_dock =
        dst.tx_manager
            .as_ref()
            .unwrap()
            .create(7, dst.output.as_ref().unwrap(), &dqh, ());
    tx_dock.expect("menubar".into());
    tx_dock.set_deadline(20);
    tx_dock.submit();
    rig.pump_comp(&dconn, &mut dq, &mut dst);
    assert!(
        dst.started.is_empty(),
        "must wait for expected menubar before deadline"
    );

    // Advance past the 20 ms deadline.
    std::thread::sleep(Duration::from_millis(30));
    for _ in 0..4 {
        rig.pump_comp(&dconn, &mut dq, &mut dst);
    }
    assert_eq!(
        dst.started.len(),
        1,
        "dock should start when deadline passes"
    );
    let orig_ts = dst.started[0];

    // Advance wall clock slightly so a fresh timestamp would differ, then menubar joins late.
    std::thread::sleep(Duration::from_millis(15));
    let tx_bar = bst.tx_manager.as_ref().unwrap().join(
        "dock".into(),
        dock_inst,
        7,
        bst.output.as_ref().unwrap(),
        &bqh,
        (),
    );
    tx_bar.submit();
    for _ in 0..4 {
        rig.pump_comp(&bconn, &mut bq, &mut bst);
    }
    assert_eq!(bst.started.len(), 1, "late joiner should receive started");
    assert_eq!(
        bst.started[0], orig_ts,
        "late participant must receive the transaction's original start timestamp to join in step"
    );
}

#[test]
fn gesture_bound_transaction_settles_with_velocity() {
    let sample_after_40ms =
        |release_velocity: f64| -> (f64, Vec<Gesture>, Vec<Outcome>, Vec<Outcome>) {
            let mut rig = Rig::new();
            rig.state.fixed_time = Some(10.0);
            let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
            let (_hov, oconn, mut oq, mut ost) =
                rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
            let oqh = oq.handle();

            let tx =
                ost.tx_manager
                    .as_ref()
                    .unwrap()
                    .create(1, ost.output.as_ref().unwrap(), &oqh, ());
            tx.bind_gesture(Gesture::SwipeUp, 0.45, 1.0);
            tx.bind_reverse_gesture(Gesture::SwipeDown);
            tx.submit();
            for _ in 0..4 {
                rig.pump_comp(&oconn, &mut oq, &mut ost);
            }

            // Begin gesture -> update to 0.35 -> release toward completion with `release_velocity`.
            assert!(rig.state.transaction_gesture_begin(
            rig.output_id,
            cupertino_protocols::server::transaction::cupertino_transaction_v1::Gesture::SwipeUp
        ));
            assert!(rig.state.transaction_gesture_update(rig.output_id, 0.35));
            assert!(rig
                .state
                .transaction_gesture_end(rig.output_id, true, release_velocity));
            rig.pump_comp(&oconn, &mut oq, &mut ost);

            // Sample 40 ms into the spring settle.
            let p_40ms = rig
                .state
                .transactions
                .gesture_progress_on(rig.output_id, 10.04)
                .expect("gesture should still be settling at +40 ms");

            // Advance well past settle duration -> finished(Completed).
            rig.state.fixed_time = Some(13.0);
            for _ in 0..4 {
                rig.pump_comp(&oconn, &mut oq, &mut ost);
            }
            (p_40ms, ost.gesture_began, ost.gesture_ended, ost.finished)
        };

    let (slow_p, began_s, ended_s, fin_s) = sample_after_40ms(0.0);
    let (fast_p, began_f, ended_f, fin_f) = sample_after_40ms(6.0);

    assert_eq!(began_s, vec![Gesture::SwipeUp]);
    assert_eq!(ended_s, vec![Outcome::Completed]);
    assert_eq!(fin_s, vec![Outcome::Completed]);
    assert_eq!(began_f, vec![Gesture::SwipeUp]);
    assert_eq!(ended_f, vec![Outcome::Completed]);
    assert_eq!(fin_f, vec![Outcome::Completed]);
    assert!(
        fast_p > slow_p + 0.05,
        "higher release velocity must carry the spring further toward target (fast={fast_p}, slow={slow_p})"
    );
}

#[test]
fn holes_stack_claims_and_notify_source_closed() {
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    let (parent_surf, _parent_layer) =
        rig.create_layer_surface(&oconn, &mut oq, &mut ost, "overview");

    let oqh = oq.handle();
    let child1 = ost.compositor.as_ref().unwrap().create_surface(&oqh, ());
    let child2 = ost.compositor.as_ref().unwrap().create_surface(&oqh, ());
    let _layer1 = ost
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child1, &parent_surf, &oqh, ());
    let _layer2 = ost
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child2, &parent_surf, &oqh, ());
    let hole1 = ost.tx_manager.as_ref().unwrap().get_hole(&child1, &oqh, ());
    let hole2 = ost.tx_manager.as_ref().unwrap().get_hole(&child2, &oqh, ());

    hole1.set_source(HoleWireSource::Window, 42);
    hole1.set_claim(1);
    child1.commit();
    for _ in 0..4 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }

    let wid = WindowId(42);
    assert!(rig.state.transactions.is_window_claimed(wid));
    let first_id = rig.state.transactions.active_claim_for(wid).unwrap().id;

    // Second hole claims the same window -> becomes top of stack.
    hole2.set_source(HoleWireSource::Window, 42);
    hole2.set_claim(1);
    child2.commit();
    for _ in 0..4 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    let second_id = rig.state.transactions.active_claim_for(wid).unwrap().id;
    assert_ne!(first_id, second_id);

    // Destroying the top hole falls back to hole1.
    hole2.destroy();
    for _ in 0..4 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert_eq!(
        rig.state.transactions.active_claim_for(wid).unwrap().id,
        first_id
    );

    // Window closes -> claim clears and hole1 receives source_closed.
    rig.state.transactions.notify_window_closed(wid);
    for _ in 0..4 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert!(!rig.state.transactions.is_window_claimed(wid));
    assert_eq!(ost.source_closed, 1);

    // Keep test timestamps alive cleanly.
    let _ = Instant::now();
}
