//! The authentication prompt among other overlay surfaces: it is drawn and hit before an overlay surface mapped after
//! it, and the keyboard stays with it while it is shown, until the session lock takes it.

mod common;

use std::os::unix::net::UnixStream;
use std::time::Instant;

use common::desktop::Desktop;
use common::Client;
use cupertino_scene::{SceneLayer, Source};
use cupertino_server::authorization::is_prompt;
use cupertino_server::render::scene::compose_scene;
use cupertino_server::Cupertino;
use smithay::backend::input::ButtonState;
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::layer_map_for_output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::shell::wlr_layer::Layer;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::KeyboardInteractivity;

fn wait_until(d: &mut Desktop, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
    let end = Instant::now() + common::HANG_GUARD;
    while !cond(&d.state) {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        d.pump();
    }
}

/// The overlay surfaces with a buffer, bottom to top as mapped: (surface, whether it is the prompt's).
fn overlays(s: &Cupertino) -> Vec<(WlSurface, bool)> {
    layer_map_for_output(&s.outputs[0].output)
        .layers_on(Layer::Overlay)
        .map(|l| l.wl_surface().clone())
        .filter(|surface| {
            with_renderer_surface_state(surface, |state| state.buffer().is_some()).unwrap_or(false)
        })
        .map(|surface| {
            let prompt = is_prompt(&surface);
            (surface, prompt)
        })
        .collect()
}

/// The prompt (120 x 80 points, exclusive keyboard), then an application's overlay surface of `size` mapped after it,
/// both in the top left corner of the output.
fn prompt_then_overlay(
    d: &mut Desktop,
    size: (u32, u32),
    keyboard: KeyboardInteractivity,
) -> (Client, Client) {
    let prompt = Client::overlay(
        common::prompt_agent(&d.state),
        "prompt",
        (120, 80),
        KeyboardInteractivity::Exclusive,
    );
    wait_until(d, "the prompt to be shown", |s| s.prompt_shown());
    let other = Client::overlay(
        UnixStream::connect(d.socket()).unwrap(),
        "other",
        size,
        keyboard,
    );
    wait_until(d, "the other overlay surface to be shown", |s| {
        overlays(s).len() == 2
    });
    assert_eq!(
        overlays(&d.state).iter().map(|o| o.1).collect::<Vec<_>>(),
        [true, false],
        "the other surface was mapped after the prompt"
    );
    (prompt, other)
}

fn keyboard_focus(s: &Cupertino) -> Option<WlSurface> {
    s.seat.get_keyboard().unwrap().current_focus()
}

#[test]
fn the_prompt_is_drawn_and_hit_above_an_overlay_surface_mapped_after_it() {
    let mut d = Desktop::new(1.0);
    let _clients = prompt_then_overlay(&mut d, (120, 80), KeyboardInteractivity::None);

    let mut backend = d.state.backend.take().unwrap();
    let front_to_back: Vec<bool> = d
        .state
        .with_scene(backend.renderer(), 0, |r, mut p| {
            let composed = compose_scene(r, &mut p);
            composed
                .frame
                .items()
                .iter()
                .filter(|(layer, _)| *layer == SceneLayer::LayerOverlay)
                .filter_map(|(_, item)| match item.source {
                    Source::SurfaceTree { id, .. } => {
                        Some(is_prompt(&composed.resources.surfaces[id.0.slot as usize]))
                    }
                    _ => None,
                })
                .collect()
        })
        .expect("render resources");
    d.state.backend = Some(backend);
    assert_eq!(
        front_to_back,
        [true, false],
        "front to back: the prompt first"
    );

    let (hit, _) = d
        .state
        .surface_under((60.0, 40.0).into())
        .expect("a surface");
    assert!(is_prompt(&hit), "the pointer goes to the prompt");
}

#[test]
fn the_keyboard_stays_with_the_prompt_until_the_lock_takes_it_and_returns_at_the_unlock() {
    let mut d = Desktop::new(1.0);
    d.window();
    let (_prompt, _other) =
        prompt_then_overlay(&mut d, (120, 80), KeyboardInteractivity::Exclusive);
    let prompt = overlays(&d.state).into_iter().find(|o| o.1).unwrap().0;
    d.pump();
    assert_eq!(
        keyboard_focus(&d.state),
        Some(prompt.clone()),
        "another surface's exclusive keyboard does not take it from the prompt"
    );
    assert_eq!(d.state.exclusive_layer, Some(prompt.clone()));

    let locker = d.lock();
    wait_until(&mut d, "the lock surface to take the keyboard", |s| {
        keyboard_focus(s).is_some_and(|focus| s.is_lock_surface(&focus))
    });
    // what commits of the prompt under the lock do: it lets the keyboard go, then asks for it again
    d.state.layer_keyboard_exclusivity(&prompt, false);
    d.state.layer_keyboard_exclusivity(&prompt, true);
    assert!(
        keyboard_focus(&d.state).is_some_and(|focus| d.state.is_lock_surface(&focus)),
        "the prompt's claim under the lock does not take the keyboard from the lock"
    );
    locker.unlock();
    wait_until(&mut d, "the unlock", |s| !s.session_lock.is_active());
    assert_eq!(keyboard_focus(&d.state), Some(prompt));
}

#[test]
fn a_click_on_another_overlay_surface_leaves_the_keyboard_with_the_prompt() {
    let mut d = Desktop::new(1.0);
    let (_prompt, _other) =
        prompt_then_overlay(&mut d, (400, 300), KeyboardInteractivity::OnDemand);
    let prompt = overlays(&d.state).into_iter().find(|o| o.1).unwrap().0;
    // beside the prompt, on the other surface
    let at = (300.0, 200.0);
    let (hit, _) = d.state.surface_under(at.into()).expect("a surface");
    assert!(!is_prompt(&hit));
    d.state.pointer_moved_to(at.into(), 0, None);
    d.state
        .pointer_button(cupertino_server::events::BTN_LEFT, ButtonState::Pressed, 0);
    d.state
        .pointer_button(cupertino_server::events::BTN_LEFT, ButtonState::Released, 0);
    d.pump();
    assert_eq!(keyboard_focus(&d.state), Some(prompt));
}

/// A press outside an engaged dock bar or menu bar takes the keyboard back from it; the prompt and the lock screen are
/// not released that way: a press on a window leaves the keyboard with the prompt, and with the lock surface under the
/// lock.
#[test]
fn a_press_on_a_window_leaves_the_keyboard_with_the_prompt_and_the_lock() {
    let mut d = Desktop::new(1.0);
    d.window();
    let (_prompt, _other) = prompt_then_overlay(&mut d, (120, 80), KeyboardInteractivity::None);
    let prompt = overlays(&d.state).into_iter().find(|o| o.1).unwrap().0;
    let window = d.state.wm.windows().next().unwrap().id;
    let f = d.state.wm.window(window).unwrap().geometry();
    let at = (f.x + f.w - 20.0, f.y + f.h - 20.0);
    let (hit, _) = d.state.surface_under(at.into()).expect("a surface");
    assert!(!is_prompt(&hit), "the press lands on the window");
    let press = |d: &mut Desktop| {
        d.state.pointer_moved_to(at.into(), 0, None);
        d.state
            .pointer_button(cupertino_server::events::BTN_LEFT, ButtonState::Pressed, 0);
        d.state
            .pointer_button(cupertino_server::events::BTN_LEFT, ButtonState::Released, 0);
        d.pump();
    };
    press(&mut d);
    assert_eq!(keyboard_focus(&d.state), Some(prompt.clone()));
    assert_eq!(d.state.exclusive_layer, Some(prompt.clone()));
    assert_eq!(d.state.released_layer, None);

    let _locker = d.lock();
    wait_until(&mut d, "the lock surface to take the keyboard", |s| {
        keyboard_focus(s).is_some_and(|focus| s.is_lock_surface(&focus))
    });
    press(&mut d);
    assert!(
        keyboard_focus(&d.state).is_some_and(|focus| d.state.is_lock_surface(&focus)),
        "the lock surface keeps the keyboard"
    );
    assert_eq!(d.state.exclusive_layer, Some(prompt));
    assert_eq!(d.state.released_layer, None);
}
