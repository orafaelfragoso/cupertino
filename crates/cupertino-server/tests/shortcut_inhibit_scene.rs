//! The notice that an application holds the shortcuts is composed on the compositor-overlay layer, in front of a
//! fullscreen window, and a locked frame leaves it out.

use crate::common;

use common::desktop::Desktop;
use cupertino_scene::{SceneKind, SceneLayer};
use cupertino_server::overlays::ShortcutNotice;
use cupertino_server::render::scene::compose_scene;
use cupertino_wm::Command;

fn composed(d: &mut Desktop) -> Vec<(SceneLayer, SceneKind)> {
    let mut backend = d.state.backend.take().unwrap();
    let items = d
        .state
        .with_scene(backend.renderer(), 0, |r, mut p| {
            compose_scene(r, &mut p)
                .frame
                .items()
                .iter()
                .map(|(layer, item)| (*layer, item.id.kind))
                .collect()
        })
        .expect("render resources");
    d.state.backend = Some(backend);
    items
}

fn show_notice(d: &mut Desktop) {
    let output = d.state.outputs[0].id;
    d.state.overlays.shortcut_notice = Some(ShortcutNotice {
        text: "org.example.vm is receiving your shortcuts. Press Super+Escape to get them back."
            .to_owned(),
        output,
        clause: "Press Super+Escape to get your shortcuts back.".to_owned(),
        short: "Super+Escape".to_owned(),
        owner: None,
        until: f64::MAX,
    });
}

#[test]
fn the_notice_is_in_front_of_a_fullscreen_window() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.at(3000);
    let window = d.state.wm.key_window().expect("a window");
    d.state.command(Command::Fullscreen {
        requested_by: cupertino_wm::Requester::Client,
        window,
        output: None,
    });
    d.at(6000);
    assert!(
        d.state.wm.window(window).unwrap().state().is_fullscreen(),
        "the window is fullscreen"
    );
    show_notice(&mut d);
    let items = composed(&mut d);
    let notice = items
        .iter()
        .position(|i| *i == (SceneLayer::CompositorOverlay, SceneKind::ShortcutNotice))
        .expect("the notice is composed");
    let content = items
        .iter()
        .position(|(_, kind)| *kind == SceneKind::WindowContent)
        .expect("the fullscreen window is composed");
    assert!(
        notice < content,
        "front to back: the notice, then the window"
    );
}

#[test]
fn a_locked_frame_leaves_the_notice_out() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.at(3000);
    let _locker = d.lock();
    d.state.session_activated();
    d.at(3050);
    show_notice(&mut d);
    let items = composed(&mut d);
    assert!(!items
        .iter()
        .any(|(_, kind)| *kind == SceneKind::ShortcutNotice));
}
