//! The Kit window's sidebar symbols, run headless: the accent while the window is the one in use, a context menu
//! holding the keyboard included, and grey that still stands out from the selection once it is not.

#![cfg(feature = "app")]

use crate::common;

use common::{place, Run};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_kit::core::Role;
use cupertino_kit::geometry::{Point, Rect};
use cupertino_kit::platform::{MouseButton, WindowEvent, WindowStates};
use cupertino_kit::theme::{Appearance, ThemePreferences};

/// A window on `home/Documents` in `appearance`, Documents (selected) and Downloads in the Favorites.
fn run(appearance: Appearance) -> Run {
    let home = tempfile::tempdir().unwrap();
    let documents = home.path().join("Documents");
    let downloads = home.path().join("Downloads");
    std::fs::create_dir_all(&documents).unwrap();
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::write(documents.join("a.txt"), "a").unwrap();
    let places = vec![
        place("Documents", &documents, PlaceKind::Documents),
        place("Downloads", &downloads, PlaceKind::Downloads),
    ];
    let preferences = ThemePreferences {
        appearance,
        reduce_transparency: false,
        reduce_motion: false,
    };
    let mut run = Run::start_with(
        home,
        documents,
        places,
        Notices::start(None),
        None,
        preferences,
    );
    run.wait_until(|run| run.rows().len() == 1);
    run
}

/// The latest frame's pixel of the browser window at `at`, in points; the tests run at scale 1.
fn pixel(run: &mut Run, at: Point) -> [u8; 4] {
    let window = run.window;
    let frames = run.driver.window_system_mut().frames(window).unwrap();
    let frame = frames.last().unwrap();
    let i = (at.y as usize * frame.width as usize + at.x as usize) * 4;
    frame.pixels[i..i + 4].try_into().unwrap()
}

/// The pixels of the symbol's slot at the leading end of the sidebar row `name`.
fn symbol(run: &mut Run, name: &str) -> (Rect, Vec<[u8; 4]>) {
    let row = run.bounds(Role::ListItem, name);
    let mut pixels = Vec::new();
    // clear of the selection's rounded edge and of the label, which starts past the symbol
    for y in (row.min_y() + 6.0) as i64..(row.max_y() - 6.0) as i64 {
        for x in (row.min_x() + 16.0) as i64..(row.min_x() + 42.0) as i64 {
            pixels.push(pixel(run, Point::new(x as f64, y as f64)));
        }
    }
    (row, pixels)
}

/// Whether `pixel` is a strong blue: the accent, not a grey.
fn accent(pixel: &[u8; 4]) -> bool {
    let [r, g, b, _] = pixel.map(i32::from);
    b - r > 80 && b - g > 40
}

/// How far `pixel` is from `other`, by its farthest channel.
fn distance(pixel: &[u8; 4], other: &[u8; 4]) -> u8 {
    pixel
        .iter()
        .zip(other)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap()
}

fn assert_accent(run: &mut Run, when: &str) {
    for name in ["Documents", "Downloads"] {
        let (_, pixels) = symbol(run, name);
        assert!(
            pixels.iter().any(accent),
            "{when}: the symbol of {name} is the accent"
        );
    }
}

#[test]
fn the_favorites_symbols_stay_the_accent_while_a_context_menu_has_the_keyboard() {
    for appearance in [Appearance::Light, Appearance::Dark] {
        let mut run = run(appearance);
        assert_accent(&mut run, &format!("{appearance:?}, active"));

        let table = run.bounds(Role::Table, &run.title());
        let at = Point::new(table.center().x, table.max_y() - 20.0);
        run.press_at(at, MouseButton::Right);
        let menu = run.context_menu().expect("the folder's menu is open");
        // the compositor moves the keyboard into the menu, and the window stays activated
        let system = run.driver.window_system_mut();
        system
            .enqueue_window(run.window, WindowEvent::Focus(false))
            .unwrap();
        system
            .enqueue_window(menu, WindowEvent::Focus(true))
            .unwrap();
        run.settle();
        assert_accent(&mut run, &format!("{appearance:?}, with the menu open"));

        run.close_menu();
        assert!(run.context_menu().is_none());
        assert_accent(&mut run, &format!("{appearance:?}, the menu closed"));
        run.send(WindowEvent::Focus(true));
        assert_accent(&mut run, &format!("{appearance:?}, the keyboard back"));

        // and once another window is the one in use, the symbols are grey
        run.right_click_row("a.txt");
        let menu = run.context_menu().expect("the item's menu is open");
        let system = run.driver.window_system_mut();
        system
            .enqueue_window(run.window, WindowEvent::Focus(false))
            .unwrap();
        system
            .enqueue_window(menu, WindowEvent::Focus(true))
            .unwrap();
        run.settle();
        run.close_menu();
        let window = run.window;
        let system = run.driver.window_system_mut();
        system
            .configure(window, None, WindowStates::default())
            .unwrap();
        run.settle();
        for name in ["Documents", "Downloads"] {
            let (_, pixels) = symbol(&mut run, name);
            assert!(
                !pixels.iter().any(accent),
                "{appearance:?}, deactivated after the menu: {name} is no longer the accent"
            );
        }
    }
}

#[test]
fn an_inactive_windows_symbols_are_grey_and_the_selected_one_still_shows() {
    for appearance in [Appearance::Light, Appearance::Dark] {
        let mut run = run(appearance);
        let window = run.window;
        let system = run.driver.window_system_mut();
        system
            .configure(window, None, WindowStates::default())
            .unwrap();
        system
            .enqueue_window(window, WindowEvent::Focus(false))
            .unwrap();
        run.settle();
        for name in ["Documents", "Downloads"] {
            let (row, pixels) = symbol(&mut run, name);
            assert!(
                !pixels.iter().any(accent),
                "{appearance:?}: the symbol of {name} is no longer the accent"
            );
            // the row's own fill, past its label and clear of any accessory
            let fill = pixel(&mut run, Point::new(row.max_x() - 12.0, row.center().y));
            let farthest = pixels.iter().map(|p| distance(p, &fill)).max().unwrap();
            // it recedes with the labels, yet stays plainly visible on the row
            assert!(
                farthest > 60,
                "{appearance:?}: the symbol of {name} stands out from its row ({farthest} from {fill:?})"
            );
        }
    }
}

#[test]
fn the_symbols_turn_grey_when_the_keyboard_leaves_the_menu_for_another_application() {
    for appearance in [Appearance::Light, Appearance::Dark] {
        let mut run = run(appearance);
        run.right_click_row("a.txt");
        let menu = run.context_menu().expect("the item's menu is open");
        let system = run.driver.window_system_mut();
        system
            .enqueue_window(run.window, WindowEvent::Focus(false))
            .unwrap();
        system
            .enqueue_window(menu, WindowEvent::Focus(true))
            .unwrap();
        run.settle();
        assert_accent(&mut run, &format!("{appearance:?}, with the menu open"));

        // a press on another application takes the keyboard and dismisses the menu; this compositor tells the window
        // nothing more, neither a keyboard enter nor a configure (the harness runs a turn per event, so the leave comes
        // first, while the menu is still open to hear it)
        let system = run.driver.window_system_mut();
        system
            .enqueue_window(menu, WindowEvent::Focus(false))
            .unwrap();
        system.enqueue_window(menu, WindowEvent::PopupDone).unwrap();
        run.settle();
        assert!(run.context_menu().is_none());
        for name in ["Documents", "Downloads"] {
            let (_, pixels) = symbol(&mut run, name);
            assert!(
                !pixels.iter().any(accent),
                "{appearance:?}, the keyboard gone elsewhere: {name} is no longer the accent"
            );
        }
    }
}
