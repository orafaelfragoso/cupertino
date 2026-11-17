//! The input-source switcher and its HUD interaction.

use cupertino_keyboard::switcher::{
    fade_alpha, hud_content_size, HUD_FADE_OUT_S, HUD_MIN_WIDTH, HUD_PADDING, HUD_ROW_HEIGHT,
};
use cupertino_keyboard::{Step, SwitchEvent, Switcher};

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn switcher() -> Switcher {
    let mut sw = Switcher::new();
    sw.set_sources(ids(&["abc", "fr", "de"]));
    sw.set_current("abc");
    sw
}

fn commit(id: &str) -> SwitchEvent {
    SwitchEvent::Commit { id: id.into() }
}

#[test]
fn history_orders_sources_most_recent_first_and_follows_the_source_list() {
    let mut sw = switcher();
    assert_eq!(sw.historical_order(), ids(&["abc", "fr", "de"]));
    sw.set_current("de");
    assert_eq!(sw.historical_order(), ids(&["de", "abc", "fr"]));
    sw.set_current("zz");
    assert_eq!(
        sw.current(),
        Some("de"),
        "an unknown source is not recorded"
    );
    sw.set_current("fr");
    sw.set_sources(ids(&["abc", "de"]));
    assert_eq!(sw.current(), None, "a removed source is no longer current");
    assert_eq!(
        sw.historical_order(),
        ids(&["de", "abc"]),
        "fr leaves the history"
    );
}

#[test]
fn fewer_than_two_sources_never_switch() {
    let mut sw = Switcher::new();
    sw.set_sources(ids(&["abc"]));
    assert_eq!(sw.press(Step::Next, true), SwitchEvent::None);
    assert_eq!(sw.press_menu(Step::Next, false), SwitchEvent::None);
}

#[test]
fn a_press_without_held_modifiers_switches_at_once() {
    let mut sw = switcher();
    assert_eq!(sw.press(Step::Next, false), commit("fr"));
    assert!(!sw.is_hud_open());
    assert_eq!(sw.current(), Some("fr"));
}

#[test]
fn a_held_press_opens_the_hud_and_further_presses_step_with_wrapping() {
    for (step, opened_on) in [(Step::Next, 1), (Step::Previous, 2)] {
        let mut sw = switcher();
        assert_eq!(
            sw.press(step, true),
            SwitchEvent::ShowHud {
                order: ids(&["abc", "fr", "de"]),
                selection: opened_on
            },
            "{step:?}"
        );
        assert!(sw.is_hud_open());
        assert_eq!(sw.selection(), Some(opened_on));
    }
    let mut sw = switcher();
    sw.press(Step::Next, true);
    for selection in [2, 0] {
        assert_eq!(
            sw.press(Step::Next, true),
            SwitchEvent::MoveSelection { selection }
        );
    }
}

#[test]
fn the_open_hud_ends_by_release_commit_or_escape() {
    let mut sw = switcher();
    assert_eq!(sw.modifiers_released(), SwitchEvent::None, "no HUD open");
    assert_eq!(sw.escape(), SwitchEvent::None, "no HUD open");

    sw.press(Step::Next, true);
    assert_eq!(sw.modifiers_released(), commit("fr"));
    assert!(!sw.is_hud_open());
    assert_eq!(sw.current(), Some("fr"));
    assert_eq!(sw.historical_order(), ids(&["fr", "abc", "de"]));

    sw.press(Step::Next, true);
    assert_eq!(sw.commit(), commit("abc"), "commit picks the displayed row");

    sw.press(Step::Next, true);
    assert_eq!(sw.escape(), SwitchEvent::Cancel);
    assert!(!sw.is_hud_open());
    assert_eq!(sw.current(), Some("abc"), "escape does not switch");
}

#[test]
fn hovering_a_row_moves_the_selection_when_it_exists() {
    let mut sw = switcher();
    assert_eq!(sw.select(1), SwitchEvent::None, "no HUD open");
    sw.press(Step::Next, true);
    assert_eq!(sw.select(2), SwitchEvent::MoveSelection { selection: 2 });
    assert_eq!(sw.select(3), SwitchEvent::None, "no such row");
    assert_eq!(sw.selection(), Some(2));
}

#[test]
fn the_menu_shortcut_walks_menu_order_not_history() {
    let mut sw = switcher();
    sw.set_current("de");
    sw.set_current("abc");
    // History: abc, de, fr; menu: abc, fr, de.
    assert_eq!(sw.press_menu(Step::Next, false), commit("fr"));

    let mut sw = switcher();
    sw.set_current("de");
    sw.set_current("abc");
    assert_eq!(
        sw.press_menu(Step::Next, true),
        SwitchEvent::ShowHud {
            order: ids(&["abc", "de", "fr"]),
            selection: 2
        }
    );
    assert_eq!(
        sw.press_menu(Step::Next, true),
        SwitchEvent::MoveSelection { selection: 0 }
    );
}

#[test]
fn hud_fade_is_linear_and_clamped() {
    for (t, alpha) in [
        (-1.0, 1.0),
        (0.0, 1.0),
        (HUD_FADE_OUT_S / 2.0, 0.5),
        (HUD_FADE_OUT_S, 0.0),
        (5.0, 0.0),
    ] {
        assert!((fade_alpha(t) - alpha).abs() < 1e-12, "t = {t}");
    }
}

#[test]
fn hud_content_size_pads_rows_and_enforces_the_minimum_width() {
    let (w, h) = hud_content_size(3, 50.0, 30.0);
    assert_eq!(w, HUD_MIN_WIDTH);
    assert_eq!(h, 3.0 * HUD_ROW_HEIGHT + 2.0 * HUD_PADDING);
    let (wide, _) = hud_content_size(3, 300.0, 30.0);
    assert_eq!(wide, 300.0 + 30.0 + 2.0 * HUD_PADDING);
}
