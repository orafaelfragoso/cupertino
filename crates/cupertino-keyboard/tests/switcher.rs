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

#[test]
fn history_comes_first_then_menu_order() {
    let mut sw = switcher();
    assert_eq!(sw.historical_order(), ids(&["abc", "fr", "de"]));
    sw.set_current("de");
    assert_eq!(sw.historical_order(), ids(&["de", "abc", "fr"]));
    assert_eq!(sw.current(), Some("de"));
}

#[test]
fn unknown_sources_are_not_recorded_as_current() {
    let mut sw = switcher();
    sw.set_current("zz");
    assert_eq!(sw.current(), Some("abc"));
}

#[test]
fn replacing_the_sources_drops_removed_ones_from_history_and_current() {
    let mut sw = switcher();
    sw.set_current("fr");
    sw.set_sources(ids(&["abc", "de"]));
    assert_eq!(sw.current(), None);
    assert_eq!(sw.historical_order(), ids(&["abc", "de"]));
}

#[test]
fn fewer_than_two_sources_never_switch() {
    let mut sw = Switcher::new();
    sw.set_sources(ids(&["abc"]));
    assert_eq!(sw.press(Step::Next, true), SwitchEvent::None);
    assert_eq!(sw.press_menu(Step::Next, false), SwitchEvent::None);
}

#[test]
fn a_press_with_held_modifiers_opens_the_hud_one_step_on() {
    let mut sw = switcher();
    assert_eq!(
        sw.press(Step::Next, true),
        SwitchEvent::ShowHud {
            order: ids(&["abc", "fr", "de"]),
            selection: 1
        }
    );
    assert!(sw.is_hud_open());
    assert_eq!(sw.selection(), Some(1));
}

#[test]
fn previous_wraps_to_the_last_source() {
    let mut sw = switcher();
    assert_eq!(
        sw.press(Step::Previous, true),
        SwitchEvent::ShowHud {
            order: ids(&["abc", "fr", "de"]),
            selection: 2
        }
    );
}

#[test]
fn further_presses_move_the_selection_and_wrap() {
    let mut sw = switcher();
    sw.press(Step::Next, true);
    assert_eq!(
        sw.press(Step::Next, true),
        SwitchEvent::MoveSelection { selection: 2 }
    );
    assert_eq!(
        sw.press(Step::Next, true),
        SwitchEvent::MoveSelection { selection: 0 }
    );
}

#[test]
fn releasing_the_modifiers_commits_the_displayed_source() {
    let mut sw = switcher();
    sw.press(Step::Next, true);
    assert_eq!(
        sw.modifiers_released(),
        SwitchEvent::Commit { id: "fr".into() }
    );
    assert!(!sw.is_hud_open());
    assert_eq!(sw.current(), Some("fr"));
    assert_eq!(sw.historical_order(), ids(&["fr", "abc", "de"]));
}

#[test]
fn a_press_without_modifiers_switches_immediately() {
    let mut sw = switcher();
    assert_eq!(
        sw.press(Step::Next, false),
        SwitchEvent::Commit { id: "fr".into() }
    );
    assert!(!sw.is_hud_open());
    assert_eq!(sw.current(), Some("fr"));
}

#[test]
fn escape_closes_the_hud_without_switching() {
    let mut sw = switcher();
    sw.press(Step::Next, true);
    assert_eq!(sw.escape(), SwitchEvent::Cancel);
    assert_eq!(sw.current(), Some("abc"));
    assert_eq!(sw.escape(), SwitchEvent::None);
}

#[test]
fn releasing_modifiers_with_no_hud_does_nothing() {
    let mut sw = switcher();
    assert_eq!(sw.modifiers_released(), SwitchEvent::None);
}

#[test]
fn commit_while_the_hud_is_open_selects_the_displayed_source() {
    let mut sw = switcher();
    sw.press(Step::Next, true);
    assert_eq!(sw.commit(), SwitchEvent::Commit { id: "fr".into() });
}

#[test]
fn hovering_a_row_moves_the_selection_when_it_exists() {
    let mut sw = switcher();
    assert_eq!(sw.select(1), SwitchEvent::None);
    sw.press(Step::Next, true);
    assert_eq!(sw.select(2), SwitchEvent::MoveSelection { selection: 2 });
    assert_eq!(sw.select(3), SwitchEvent::None);
    assert_eq!(sw.selection(), Some(2));
}

#[test]
fn the_menu_shortcut_walks_menu_order_not_history() {
    let mut sw = switcher();
    sw.set_current("de");
    sw.set_current("abc");
    // history: abc, de, fr; menu: abc, fr, de
    assert_eq!(
        sw.press_menu(Step::Next, false),
        SwitchEvent::Commit { id: "fr".into() }
    );
}

#[test]
fn the_menu_shortcut_opens_the_hud_on_the_menu_neighbour_and_then_steps() {
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
fn hud_fade_is_linear_over_its_duration() {
    assert_eq!(HUD_FADE_OUT_S, 0.3);
    assert_eq!(fade_alpha(0.0), 1.0);
    assert!((fade_alpha(0.15) - 0.5).abs() < 1e-12);
    assert_eq!(fade_alpha(0.3), 0.0);
    assert_eq!(fade_alpha(5.0), 0.0);
    assert_eq!(fade_alpha(-1.0), 1.0);
}

#[test]
fn hud_content_size_pads_rows_and_enforces_the_minimum_width() {
    let (w, h) = hud_content_size(3, 50.0, 30.0);
    assert_eq!(w, HUD_MIN_WIDTH);
    assert_eq!(h, 3.0 * HUD_ROW_HEIGHT + 2.0 * HUD_PADDING);
    let (wide, _) = hud_content_size(3, 300.0, 30.0);
    assert_eq!(wide, 300.0 + 30.0 + 2.0 * HUD_PADDING);
}
