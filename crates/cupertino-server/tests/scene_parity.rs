//! The migrated scene composition against the builder it replaces, on the same committed state: the same lowered
//! element variants in the same order, with the same kind, geometry, source rectangle, opacity and commit
//! progression, and a consistent correspondence between element identities. Pixels are the goldens' business; this
//! catches what a settled headless frame cannot, such as a live surface becoming a texture. The comparison runs
//! inside the compositor (`legacy_scene::assert_parity`), so every other test of a build with that cfg checks it too. Mesh, minimize and closing-ghost parity arrive with
//! `wp03-scene-effects`: until then those frames are built by the renderer and are not migrated.
#![cfg(scene_oracle)]

mod common;

use common::desktop::Desktop;
use common::overlays;
use cupertino_server::backend::{headless, Backend};
use cupertino_server::render::legacy_scene;
use cupertino_server::render::screenshot::render_to_image;
use smithay::backend::input::KeyState;

/// Every frame's elements are compared inside the compositor when built with `CUPERTINO_SCENE_ORACLE=1`. `compare`
/// also draws the frame by both builders and requires identical pixels; it returns how many elements there are.
fn compare(d: &mut Desktop, what: &str) -> usize {
    compare_on(d, what, 0)
}

fn compare_on(d: &mut Desktop, what: &str, output: usize) -> usize {
    let mut backend = d.state.backend.take().unwrap();
    let Backend::Headless(ref mut h) = backend else {
        panic!("headless backend required")
    };
    let migrated = d.state.build_output_elements(&mut h.renderer, output);
    let legacy = legacy_scene::take_recorded();
    let size = d.state.outputs[output].output.current_mode().unwrap().size;
    let draw = |renderer: &mut _, elements: &[_]| {
        render_to_image(
            renderer,
            elements,
            size,
            d.state.outputs[output].scale,
            headless::CLEAR,
        )
        .unwrap()
    };
    let (old, new) = (
        draw(&mut h.renderer, &legacy),
        draw(&mut h.renderer, &migrated),
    );
    d.state.backend = Some(backend);
    assert!(old == new, "{what}: the frames differ in pixels");
    migrated.len()
}

#[test]
fn an_empty_desktop_matches() {
    for scale in [1.0, 1.5] {
        let mut d = Desktop::new(scale);
        d.at(1000);
        assert!(compare(&mut d, "empty desktop") > 0);
        d.at(1016);
        compare(&mut d, "empty desktop, next frame");
    }
}

#[test]
fn windows_at_rest_match_at_integer_and_fractional_scale() {
    for scale in [1.0, 1.25, 2.0] {
        let mut d = Desktop::new(scale);
        d.window();
        d.window();
        d.at(1000);
        compare(&mut d, "two windows opened");
        d.at(3000);
        assert!(compare(&mut d, "two windows at rest") > 4);
        d.at(3016);
        compare(&mut d, "two windows at rest, next frame");
    }
}

#[test]
fn a_window_matches_while_it_opens() {
    let mut d = Desktop::new(1.0);
    d.window();
    for ms in [0u32, 40, 80, 160, 320, 640] {
        d.at(ms);
        compare(&mut d, &format!("opening at {ms} ms"));
    }
}

#[test]
fn a_locked_session_matches() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.at(2000);
    compare(&mut d, "before the lock");
    let _locker = d.lock();
    d.at(2100);
    compare(&mut d, "locked");
    d.at(2200);
    compare(&mut d, "locked, next frame");
}

#[test]
fn the_overview_matches_while_it_opens_and_at_rest() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.window();
    d.at(2000);
    d.state.pointer_moved_to((500.0, 350.0).into(), 2000, None);
    // XKB keycodes are evdev + 8: Ctrl+Up enters the overview through the key router.
    for (code, pressed) in [(37u32, true), (111, true), (111, false), (37, false)] {
        d.state.keyboard_event_now(
            code.into(),
            if pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            },
            2100,
        );
    }
    assert!(
        d.state.shell.overview_open(),
        "the key sequence opens the overview"
    );
    for ms in [2100u32, 2160, 2260, 2460, 3200] {
        d.at(ms);
        compare(&mut d, &format!("overview at {ms} ms"));
    }
}

#[test]
fn the_pointer_matches_wherever_it_is() {
    for scale in [1.0, 1.5] {
        let mut d = Desktop::new(scale);
        d.window();
        d.at(2000);
        for (x, y) in [(10.0, 10.0), (500.0, 350.0), (999.0, 699.0)] {
            d.state.pointer_moved_to((x, y).into(), 2000, None);
            d.at(2016);
            compare(&mut d, &format!("pointer at {x},{y}"));
        }
    }
}

#[test]
fn the_input_source_hud_matches_while_it_shows_and_fades() {
    for scale in [1.0, 1.5] {
        let mut d = Desktop::new(scale);
        d.window();
        d.at(2000);
        d.state.overlays.inputs.hud = Some(cupertino_server::kbd::HudView {
            order: vec!["us".into(), "de".into()],
            labels: vec!["ABC".into(), "Deutsch".into()],
            selection: 1,
            fading_since: None,
        });
        d.at(2016);
        assert!(compare(&mut d, "hud shown") > 6);
        if let Some(hud) = d.state.overlays.inputs.hud.as_mut() {
            hud.fading_since = Some(2.016);
        }
        for ms in [2100u32, 2200] {
            d.at(ms);
            compare(&mut d, &format!("hud fading at {ms} ms"));
        }
    }
}

#[test]
fn the_activation_fade_matches_over_the_desktop_and_over_a_lock() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.at(2000);
    d.state.session_activated();
    for ms in [2000u32, 2100, 2300] {
        d.at(ms);
        compare(&mut d, &format!("fade over the desktop at {ms} ms"));
    }
    let _locker = d.lock();
    d.state.session_activated();
    for ms in [3000u32, 3100, 3300] {
        d.at(ms);
        compare(&mut d, &format!("fade over the lock at {ms} ms"));
    }
}

fn wait_until(d: &mut Desktop, what: &str, mut ready: impl FnMut(&Desktop) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !ready(d) {
        d.pump();
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
    }
}

#[test]
fn a_popup_over_a_window_matches_at_integer_and_fractional_scale() {
    for scale in [1.0, 1.5] {
        let mut d = Desktop::new(scale);
        let _client = overlays::popup_window(d.socket());
        wait_until(&mut d, "the popup", |d| {
            d.state.wm.key_window().is_some_and(|id| {
                d.state.entry(id).is_some_and(|e| {
                    smithay::desktop::PopupManager::popups_for_surface(e.surface()).count() > 0
                })
            })
        });
        for ms in [1000u32, 1100, 3000] {
            d.at(ms);
            // cursor, popup, window and wallpaper at least
            assert!(compare(&mut d, &format!("popup at {ms} ms")) >= 4);
        }
    }
}

#[test]
fn a_layer_surface_matches_at_integer_and_fractional_scale() {
    for scale in [1.0, 1.5] {
        let mut d = Desktop::new(scale);
        let _client = overlays::layer_surface(d.socket(), (200, 100), 30);
        wait_until(&mut d, "the layer surface", |d| {
            let map = smithay::desktop::layer_map_for_output(&d.state.outputs[0].output);
            let shown = map.layers().any(|l| !l.bbox_with_popups().is_empty());
            shown
        });
        for ms in [1000u32, 1100] {
            d.at(ms);
            compare(&mut d, &format!("layer surface at {ms} ms"));
        }
    }
}

#[test]
fn a_drag_icon_matches_at_integer_and_fractional_scale() {
    for scale in [1.0, 1.5] {
        let mut d = Desktop::new(scale);
        let _client = overlays::layer_surface(d.socket(), (64, 64), 0);
        wait_until(&mut d, "the icon surface", |d| {
            let map = smithay::desktop::layer_map_for_output(&d.state.outputs[0].output);
            let shown = map.layers().any(|l| !l.bbox_with_popups().is_empty());
            shown
        });
        let icon = {
            let map = smithay::desktop::layer_map_for_output(&d.state.outputs[0].output);
            let surface = map.layers().next().unwrap().wl_surface().clone();
            surface
        };
        d.state.drag_icon = Some((icon, (4, 6).into()));
        d.state.pointer_moved_to((300.5, 200.25).into(), 1000, None);
        for ms in [1000u32, 1100] {
            d.at(ms);
            compare(&mut d, &format!("drag icon at {ms} ms"));
        }
    }
}

#[test]
fn two_outputs_match_with_the_second_off_the_device_pixel_grid() {
    let display = |name: &str, size| headless::HeadlessDisplay {
        name: name.into(),
        size,
        physical_mm: (0, 0),
    };
    let mut d = Desktop::with_displays(
        &[
            display("headless-1", (1001, 700)),
            display("headless-2", (800, 600)),
        ],
        1.5,
    );
    assert_eq!(d.state.outputs.len(), 2);
    let origin = d.state.outputs[1].rect.x;
    assert!(
        (origin * 1.5).fract() != 0.0,
        "the second output starts at {origin} points, on whole device pixels"
    );
    d.window();
    d.state
        .pointer_moved_to((origin + 100.0, 100.0).into(), 1000, None);
    for ms in [1000u32, 1100, 3000] {
        d.at(ms);
        compare_on(&mut d, &format!("first output at {ms} ms"), 0);
        compare_on(&mut d, &format!("second output at {ms} ms"), 1);
    }
}

#[test]
fn the_power_dim_matches_while_it_ramps_and_when_asleep() {
    use cupertino_defaults::power::{PowerSettings, SourceTimers, Timer};
    use cupertino_server::display_power::RAMP_S;
    const SLEEP_S: f64 = 30.0;
    let timers = SourceTimers {
        display_sleep: Timer::After(std::time::Duration::from_secs(SLEEP_S as u64)),
        system_sleep: Timer::Never,
    };
    let config = cupertino_server::config::Config {
        power: PowerSettings {
            ac: timers,
            battery: timers,
            require_password_after: Timer::Never,
            ..PowerSettings::default()
        },
        ..cupertino_server::config::Config::default()
    };
    let display = headless::HeadlessDisplay {
        name: "headless-1".into(),
        size: (1000, 700),
        physical_mm: (0, 0),
    };
    for scale in [1.0, 1.5] {
        let mut d = Desktop::configured(config.clone(), std::slice::from_ref(&display), scale);
        d.window();
        d.state.fixed_time = Some(1000.0);
        d.state.power.set_on_battery(false);
        d.state.input_activity();
        // the dim starts 15 s before the display sleeps and ramps for RAMP_S
        let dim = 1000.0 + SLEEP_S - 15.0;
        for t in [
            1001.0,
            dim + RAMP_S / 2.0,
            dim + RAMP_S + 0.1,
            1000.0 + SLEEP_S + 1.0,
        ] {
            d.state.fixed_time = Some(t);
            for _ in 0..3 {
                d.pump();
            }
            assert_eq!(
                d.state.power_overlay_alpha().is_some(),
                t > dim + RAMP_S,
                "the dim overlay at {t} s"
            );
            compare(&mut d, &format!("power at {t} s, {scale}x"));
        }
    }
}

fn one_display() -> [headless::HeadlessDisplay; 1] {
    [headless::HeadlessDisplay {
        name: "headless-1".into(),
        size: (1000, 700),
        physical_mm: (0, 0),
    }]
}

#[test]
fn minimize_and_restore_match_by_pixels_for_every_effect() {
    use cupertino_effects::MinimizeKind;
    use cupertino_wm::{Command, Rect};
    for kind in [
        MinimizeKind::Warp,
        MinimizeKind::Funnel,
        MinimizeKind::Scale,
    ] {
        let mut config = cupertino_server::config::Config::default();
        config.effects.minimize = Some(kind);
        let mut d = Desktop::configured(config, &one_display(), 1.5);
        d.window();
        d.at(2000);
        let id = d.state.wm.key_window().expect("a window");
        let dock = Rect::new(440.0, 640.0, 64.0, 64.0);
        d.state.command(Command::Minimize {
            window: id,
            target: Some(dock),
        });
        let duration = kind.duration_s(false);
        let t0 = d.state.now();
        for fraction in [0.05, 0.3, 0.6, 0.9, 1.3] {
            d.state.fixed_time = Some(t0 + duration * fraction);
            d.state.tick();
            d.pump();
            compare(&mut d, &format!("{kind:?} minimize at {fraction}"));
        }
        d.state.command(Command::Restore { window: id });
        let t1 = d.state.now();
        for fraction in [0.1, 0.5, 0.9, 1.3] {
            d.state.fixed_time = Some(t1 + duration * fraction);
            d.state.tick();
            d.pump();
            compare(&mut d, &format!("{kind:?} restore at {fraction}"));
        }
    }
}

#[test]
fn a_closing_ghost_matches_while_it_fades() {
    for scale in [1.0, 1.5] {
        let mut d = Desktop::new(scale);
        d.state.fx.cfg.closing = true;
        d.window();
        d.at(2000);
        d.clients.clear();
        wait_until(&mut d, "the window to close", |d| {
            d.state.wm.windows().count() == 0
        });
        assert_eq!(d.state.fx.closing.len(), 1, "a ghost of the closed window");
        let t0 = d.state.now();
        for dt in [0.0, 0.05, 0.12, 0.2, 0.3] {
            d.state.fixed_time = Some(t0 + dt);
            d.state.tick();
            d.pump();
            compare(&mut d, &format!("ghost at +{dt} s, {scale}x"));
        }
    }
}
