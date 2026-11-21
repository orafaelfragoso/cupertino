//! Compositor materials under a window: a list past the protocol's limits is refused before the compositor is asked
//! for any, and a refused list leaves the window with none.

use crate::wire;

use cupertino_geometry::{Rect, Size};
use cupertino_keyboard::modifiers::Profile;
use cupertino_ui_core::platform::{
    DecorationMode, MaterialRegion, WindowId, WindowRequest, WindowSpec, WindowSystem,
};
use cupertino_ui_wayland::WaylandWindowSystem;
use wire::{Harness, Offer};

fn spec() -> WindowSpec {
    WindowSpec {
        title: "Materials".into(),
        app_id: "org.cupertino.tests.Materials".into(),
        size: Size::new(200.0, 100.0),
        min_size: None,
        max_size: None,
        decorations: DecorationMode::None,
        foreign_parent: None,
        input_region: None,
    }
}

fn regions(count: usize) -> Vec<MaterialRegion> {
    (0..count)
        .map(|i| MaterialRegion {
            material: "sidebar".into(),
            rect: Rect::new(i as f64, 0.0, 10.0, 10.0),
            radius: 0.0,
            alpha: 1.0,
            glass: None,
            raised: false,
        })
        .collect()
}

fn backdrops_asked(server: &Harness) -> usize {
    server
        .requests()
        .iter()
        .filter(|r| r.contains("get_backdrop"))
        .count()
}

fn flush(system: &mut WaylandWindowSystem) {
    system
        .dispatch(Some(std::time::Duration::from_millis(20)), &mut Vec::new())
        .unwrap();
}

fn open(system: &mut WaylandWindowSystem) -> WindowId {
    let window = system.create_window(&spec()).unwrap();
    flush(system);
    window
}

#[test]
fn a_list_past_the_protocols_limits_is_refused_before_any_backdrop_is_asked_for_and_leaves_none() {
    let (server, conn) = Harness::start(Offer {
        app_effects: Some(21),
        ..Offer::NONE
    });
    let mut system = WaylandWindowSystem::new(conn, Profile::cupertino());
    let first = open(&mut system);

    system
        .request(first, WindowRequest::Materials(regions(3)))
        .unwrap();
    flush(&mut system);
    server.wait_for("get_backdrop", 3);
    assert!(
        system
            .request(first, WindowRequest::Materials(regions(65)))
            .is_err(),
        "65 on a surface"
    );
    flush(&mut system);
    assert_eq!(backdrops_asked(&server), 3, "none was asked for");
    system
        .request(first, WindowRequest::Materials(regions(2)))
        .unwrap();
    flush(&mut system);
    server.wait_for("get_backdrop", 5);
    assert_eq!(
        backdrops_asked(&server),
        5,
        "the three it had were taken away with the refused list"
    );

    system
        .request(first, WindowRequest::Materials(regions(64)))
        .unwrap();
    let second = open(&mut system);
    system
        .request(second, WindowRequest::Materials(regions(64)))
        .unwrap();
    let third = open(&mut system);
    assert!(
        system
            .request(third, WindowRequest::Materials(regions(1)))
            .is_err(),
        "the client holds 128 backdrops over all its windows"
    );
    system
        .request(first, WindowRequest::Materials(Vec::new()))
        .unwrap();
    system
        .request(third, WindowRequest::Materials(regions(1)))
        .unwrap();
}
