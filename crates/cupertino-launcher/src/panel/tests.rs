use super::*;
use crate::engine::tests_support::{app, test_launcher};
use crate::frecency::Frecency;
use cupertino_text_field::{KeyInput, Keymap, Response, TextFieldKeys};
use std::time::Instant;

fn now(t: f64) -> Now {
    Now {
        mono: t,
        unix: 1_000_000.0 + t,
    }
}

fn open() -> (Panel, Launcher) {
    let l = test_launcher();
    let mut p = Panel::new();
    p.show(&l, now(0.0));
    (p, l)
}

/// A key press as the app handles it: the panel's keys first, the rest through the query field (standard chords).
fn press(p: &mut Panel, l: &mut Launcher, key: Key, mods: Mods) -> PanelEvent {
    let event = p.handle_key(key, mods, l, now(0.0));
    if event != PanelEvent::Edit {
        return event;
    }
    let (code, character) = match key {
        Key::Char(c) => (0, Some(c)),
        Key::Backspace => (14, None),
        Key::Delete => (111, None),
        Key::Left => (105, None),
        Key::Right => (106, None),
        Key::Home => (102, None),
        Key::End => (107, None),
        other => unreachable!("{other:?} is a panel key"),
    };
    let input = KeyInput::new(
        code,
        mods.shift,
        mods.ctrl,
        mods.alt,
        mods.primary,
        character,
    );
    match p.edit(l, now(0.0), |f| {
        f.key(&mut Keymap::standard(), input, Instant::now())
    }) {
        Response::Ignored => PanelEvent::None,
        _ => PanelEvent::Changed,
    }
}

fn type_str(p: &mut Panel, l: &mut Launcher, s: &str) {
    for c in s.chars() {
        press(p, l, Key::Char(c), Mods::NONE);
    }
}

#[test]
fn typing_updates_query_cursor_and_results() {
    let (mut p, mut l) = open();
    assert!(p.is_visible() && p.results().is_empty());
    type_str(&mut p, &mut l, "fire");
    assert_eq!(p.query(), "fire");
    assert_eq!(p.field().caret(), 4);
    assert_eq!(p.selected_item().unwrap().title, "Firefox");
    assert_eq!(p.selected(), 0);
}

#[test]
fn editing_in_the_middle_and_unicode() {
    let (mut p, mut l) = open();
    type_str(&mut p, &mut l, "caf");
    press(&mut p, &mut l, Key::Char('é'), Mods::NONE);
    assert_eq!(p.query(), "café");
    press(&mut p, &mut l, Key::Left, Mods::NONE);
    press(&mut p, &mut l, Key::Left, Mods::NONE);
    assert_eq!(p.field().caret(), 2);
    press(&mut p, &mut l, Key::Char('X'), Mods::NONE);
    assert_eq!(p.query(), "caXfé");
    press(&mut p, &mut l, Key::Backspace, Mods::NONE);
    press(&mut p, &mut l, Key::Delete, Mods::NONE);
    assert_eq!((p.query(), p.field().caret()), ("caé", 2));
    press(&mut p, &mut l, Key::End, Mods::NONE);
    press(&mut p, &mut l, Key::Backspace, Mods::NONE);
    assert_eq!(p.query(), "ca");
    press(&mut p, &mut l, Key::Home, Mods::NONE);
    press(&mut p, &mut l, Key::Backspace, Mods::NONE);
    assert_eq!((p.query(), p.field().caret()), ("ca", 0));
}

#[test]
fn editing_chords_come_from_the_standard_table() {
    let (mut p, mut l) = open();
    p.set_query("libre office writer", &l, now(0.0));
    press(&mut p, &mut l, Key::Backspace, Mods::ALT);
    assert_eq!(p.query(), "libre office ");
    press(&mut p, &mut l, Key::Backspace, Mods::ALT);
    assert_eq!(p.query(), "libre ");
    press(&mut p, &mut l, Key::Left, Mods::ALT);
    assert_eq!(p.field().caret(), 0);
    press(&mut p, &mut l, Key::Right, Mods::ALT);
    assert_eq!(p.field().caret(), 5);
    press(&mut p, &mut l, Key::Char('k'), Mods::CTRL);
    assert_eq!(p.query(), "libre");
    press(&mut p, &mut l, Key::Char('a'), Mods::CTRL);
    assert_eq!(p.field().caret(), 0);
    press(&mut p, &mut l, Key::Char('e'), Mods::CTRL);
    assert_eq!(p.field().caret(), 5);
    press(&mut p, &mut l, Key::Char('b'), Mods::CTRL);
    press(&mut p, &mut l, Key::Char('k'), Mods::CTRL);
    assert_eq!((p.query(), p.field().caret()), ("libr", 4));
    assert_eq!(
        p.results().is_empty(),
        l.query("libr", now(0.0).unix).is_empty(),
        "the query ran again"
    );
}

#[test]
fn selection_does_not_wrap_and_pages() {
    // no wrap-around; Up on the first result hands the keyboard back to the field.
    let (mut p, mut l) = open();
    type_str(&mut p, &mut l, "e");
    let n = p.results().len();
    assert!(n >= 3, "{n}");
    assert_eq!(p.focus(), Focus::Field);
    p.handle_key(Key::Up, Mods::NONE, &mut l, now(0.0));
    assert_eq!(p.selected(), 0, "up from the first result does not wrap");
    assert_eq!(p.focus(), Focus::Field);
    p.handle_key(Key::Down, Mods::NONE, &mut l, now(0.0));
    assert_eq!((p.selected(), p.focus()), (1, Focus::List));
    p.handle_key(Key::Up, Mods::NONE, &mut l, now(0.0));
    assert_eq!(p.selected(), 0);
    p.handle_key(Key::Char('n'), Mods::CTRL, &mut l, now(0.0));
    p.handle_key(Key::Char('n'), Mods::CTRL, &mut l, now(0.0));
    assert_eq!(p.selected(), 2);
    p.handle_key(Key::Char('p'), Mods::CTRL, &mut l, now(0.0));
    assert_eq!(p.selected(), 1);
    p.handle_key(Key::PageDown, Mods::NONE, &mut l, now(0.0));
    assert_eq!(p.selected(), n - 1, "page moves clamp at the end");
    assert_eq!(
        p.handle_key(Key::PageDown, Mods::NONE, &mut l, now(0.0)),
        PanelEvent::None
    );
    p.handle_key(Key::PageUp, Mods::NONE, &mut l, now(0.0));
    assert_eq!(p.selected(), 0);
}

#[test]
fn empty_results_ignore_navigation() {
    let (mut p, mut l) = open();
    type_str(&mut p, &mut l, "qqqq");
    assert!(p.results().is_empty());
    assert_eq!(
        p.handle_key(Key::Down, Mods::NONE, &mut l, now(0.0)),
        PanelEvent::None
    );
    assert_eq!(
        p.handle_key(Key::Enter, Mods::NONE, &mut l, now(0.0)),
        PanelEvent::None
    );
    assert!(
        p.is_visible(),
        "Enter without a result keeps the panel open"
    );
}

#[test]
fn scrolling_follows_the_selection() {
    let many: Vec<_> = (0..40)
        .map(|i| app(&format!("t{i}"), &format!("Tool {i:02}"), "", "t"))
        .collect();
    let cfg = crate::engine::LauncherConfig {
        max_apps: 30,
        top_hit: false,
        ..Default::default()
    };
    let mut l = Launcher::new(cfg, many, Frecency::in_memory());
    let mut p = Panel::new();
    p.max_rows = 6;
    p.show(&l, now(0.0));
    type_str(&mut p, &mut l, "tool");
    assert_eq!(p.total_rows(), 31);
    assert_eq!(p.row_of(0), Some(1));
    assert_eq!(p.scroll_row(), 0);
    for _ in 0..8 {
        p.handle_key(Key::Down, Mods::NONE, &mut l, now(0.0));
    }
    assert_eq!(p.selected(), 8);
    let row = p.row_of(8).unwrap();
    assert!(
        p.scroll_row() <= row && row < p.scroll_row() + 6,
        "selected row {row} visible from {}",
        p.scroll_row()
    );
    for _ in 0..8 {
        p.handle_key(Key::Up, Mods::NONE, &mut l, now(0.0));
    }
    assert_eq!(p.scroll_row(), 0, "header comes back into view");
    p.handle_key(Key::Up, Mods::NONE, &mut l, now(0.0));
    assert_eq!(p.selected(), 0, "no wrap-around");
    for _ in 0..10 {
        p.handle_key(Key::PageDown, Mods::NONE, &mut l, now(0.0));
    }
    assert_eq!(p.selected(), 29);
    let row = p.row_of(29).unwrap();
    assert!(row < p.scroll_row() + 6 && row >= p.scroll_row());
    assert_eq!(p.scroll_row(), 31 - 6, "bounded by the last row");
}

#[test]
fn tab_completes_to_the_selected_title() {
    let (mut p, mut l) = open();
    type_str(&mut p, &mut l, "fire");
    assert_eq!(
        p.handle_key(Key::Tab, Mods::NONE, &mut l, now(0.0)),
        PanelEvent::Changed
    );
    assert_eq!(p.query(), "Firefox");
    assert_eq!(p.field().caret(), 7);
    assert_eq!(
        p.handle_key(Key::Tab, Mods::NONE, &mut l, now(0.0)),
        PanelEvent::None
    );
}

#[test]
fn enter_activates_the_selection_and_closes() {
    let (mut p, mut l) = open();
    type_str(&mut p, &mut l, "gimp");
    let ev = p.handle_key(Key::Enter, Mods::NONE, &mut l, now(1.0));
    let PanelEvent::Activate(Activation::Run(cmd)) = ev else {
        panic!("{ev:?}")
    };
    assert_eq!(cmd.program, "gimp-2.10");
    assert!(!p.is_visible());
    assert_eq!(l.frecency.launches("app:gimp.desktop"), 1);
    assert_eq!(
        p.handle_key(Key::Char('x'), Mods::NONE, &mut l, now(1.0)),
        PanelEvent::None,
        "keys are ignored while hidden"
    );
}

#[test]
fn alt_digit_activates_the_nth_result() {
    let (mut p, mut l) = open();
    type_str(&mut p, &mut l, "e");
    let second = p.results().get(1).unwrap().title.clone();
    let ev = p.handle_key(Key::Char('2'), Mods::ALT, &mut l, now(0.0));
    assert!(matches!(ev, PanelEvent::Activate(_)));
    let recorded = l.frecency.top(5, 1_000_000.0);
    assert_eq!(
        recorded.len(),
        1,
        "the second result ({second}) was recorded"
    );
    let (mut p, mut l) = open();
    type_str(&mut p, &mut l, "gimp");
    assert_eq!(
        p.handle_key(Key::Char('9'), Mods::ALT, &mut l, now(0.0)),
        PanelEvent::None,
        "no ninth result"
    );
}

#[test]
fn calculator_result_activation_copies() {
    let (mut p, mut l) = open();
    type_str(&mut p, &mut l, "2+2");
    let ev = p.handle_key(Key::Enter, Mods::NONE, &mut l, now(0.0));
    assert_eq!(
        ev,
        PanelEvent::Activate(Activation::CopyToClipboard("4".into()))
    );
}

#[test]
fn escape_closes_and_animation_runs() {
    let (mut p, mut l) = open();
    assert!(p.progress(0.0) < 0.01);
    assert!(p.is_animating(0.1));
    assert!((p.progress(3.0) - 1.0).abs() < 1e-6);
    assert_eq!(
        p.handle_key(Key::Escape, Mods::NONE, &mut l, now(3.0)),
        PanelEvent::Close
    );
    assert!(!p.is_visible());
    assert!(p.needs_draw(3.05), "still animating out");
    assert!(p.progress(6.0) < 0.01);
    assert!(!p.needs_draw(6.0));
    p.toggle(&l, now(6.0));
    assert!(p.is_visible());
    p.toggle(&l, now(6.1));
    assert!(!p.is_visible());
}

#[test]
fn pointer_selection_and_click() {
    let (mut p, mut l) = open();
    type_str(&mut p, &mut l, "e");
    assert_eq!(p.set_selected(2), PanelEvent::Changed);
    assert_eq!(p.selected(), 2);
    assert_eq!(p.set_selected(2), PanelEvent::None);
    assert_eq!(p.set_selected(999), PanelEvent::None);
    assert!(matches!(
        p.activate_index(1, &mut l, now(0.0)),
        PanelEvent::Activate(_)
    ));
    assert_eq!(p.activate_index(999, &mut l, now(0.0)), PanelEvent::None);
}

#[test]
fn refresh_keeps_selection_when_possible() {
    let (mut p, mut l) = open();
    type_str(&mut p, &mut l, "e");
    p.handle_key(Key::Down, Mods::NONE, &mut l, now(0.0));
    p.refresh(&l, now(1.0));
    assert_eq!(p.selected(), 1);
    p.set_query("zzzzzz", &l, now(0.0));
    p.refresh(&l, now(1.0));
    assert_eq!(p.selected(), 0);
}

#[test]
fn control_characters_are_not_inserted() {
    let (mut p, mut l) = open();
    assert_eq!(
        press(&mut p, &mut l, Key::Char('\n'), Mods::NONE),
        PanelEvent::None
    );
    assert_eq!(p.query(), "");
    assert_eq!(
        press(&mut p, &mut l, Key::Char('z'), Mods::CTRL),
        PanelEvent::None
    );
}

#[test]
fn escape_clears_the_query_before_it_closes() {
    // unmodified Escape clears a non-empty field, the next one dismisses ("Escape twice").
    let (mut p, mut l) = open();
    type_str(&mut p, &mut l, "fire");
    assert_eq!(
        p.handle_key(Key::Escape, Mods::NONE, &mut l, now(1.0)),
        PanelEvent::Changed
    );
    assert_eq!(p.query(), "");
    assert!(p.is_visible());
    assert_eq!(
        p.handle_key(Key::Escape, Mods::NONE, &mut l, now(2.0)),
        PanelEvent::Close
    );
    assert!(!p.is_visible());
}

#[test]
fn list_focus_keys_return_to_the_field() {
    // Tab, Delete and Escape inside the list only hand the keyboard back to the field.
    for key in [Key::Tab, Key::Backspace, Key::Delete, Key::Escape] {
        let (mut p, mut l) = open();
        type_str(&mut p, &mut l, "e");
        p.handle_key(Key::Down, Mods::NONE, &mut l, now(0.0));
        assert_eq!(p.focus(), Focus::List);
        let q = p.query().to_string();
        assert_eq!(
            p.handle_key(key, Mods::NONE, &mut l, now(0.0)),
            PanelEvent::Changed,
            "{key:?}"
        );
        assert_eq!(p.focus(), Focus::Field);
        assert_eq!(p.query(), q, "{key:?} must not edit the query");
        assert!(p.is_visible());
    }
}

#[test]
fn leaving_the_list_upwards_selects_the_query_and_typing_replaces_it() {
    let (mut p, mut l) = open();
    type_str(&mut p, &mut l, "fire");
    p.handle_key(Key::Up, Mods::NONE, &mut l, now(0.0));
    assert!(p.query_selected());
    type_str(&mut p, &mut l, "cal");
    assert_eq!(p.query(), "cal");
    assert!(!p.query_selected());
}

#[test]
fn space_previews_only_with_list_focus() {
    let (mut p, mut l) = open();
    type_str(&mut p, &mut l, "e");
    press(&mut p, &mut l, Key::Char(' '), Mods::NONE);
    assert_eq!(
        p.query(),
        "e ",
        "space types while the field has the keyboard"
    );
    let (mut p, mut l) = open();
    type_str(&mut p, &mut l, "e");
    p.handle_key(Key::Down, Mods::NONE, &mut l, now(0.0));
    assert_eq!(
        p.handle_key(Key::Char(' '), Mods::NONE, &mut l, now(0.0)),
        PanelEvent::Command(PanelCommand::Preview)
    );
    assert_eq!(p.query(), "e");
}

#[test]
fn primary_shortcuts_are_commands() {
    let cases = [
        (Key::Enter, PanelCommand::Reveal),
        (Key::Char('r'), PanelCommand::Reveal),
        (Key::Char('b'), PanelCommand::WebSearch),
        (Key::Char('i'), PanelCommand::Properties),
        (Key::Char('y'), PanelCommand::Preview),
        (Key::Char('k'), PanelCommand::OpenReference),
        (Key::Char('l'), PanelCommand::SelectDefinition),
        (Key::Char('d'), PanelCommand::OpenDefinition),
    ];
    for (key, cmd) in cases {
        let (mut p, mut l) = open();
        type_str(&mut p, &mut l, "fire");
        assert_eq!(
            p.handle_key(key, Mods::PRIMARY, &mut l, now(0.0)),
            PanelEvent::Command(cmd),
            "{key:?}"
        );
        assert!(p.is_visible(), "{key:?} keeps the panel open");
        assert_eq!(p.query(), "fire");
    }
    for (key, cmd) in [
        (Key::Char(','), PanelCommand::OpenSettings),
        (Key::Char('h'), PanelCommand::Help),
    ] {
        let (mut p, mut l) = open();
        assert_eq!(
            p.handle_key(key, Mods::PRIMARY, &mut l, now(0.0)),
            PanelEvent::Command(cmd)
        );
        assert!(!p.is_visible(), "{key:?} closes the panel");
    }
    // Shift / Alt / Ctrl in addition: not a panel shortcut.
    let (mut p, mut l) = open();
    let m = Mods {
        shift: true,
        ..Mods::PRIMARY
    };
    assert_eq!(press(&mut p, &mut l, Key::Char('b'), m), PanelEvent::None);
}

#[test]
fn primary_up_down_jump_between_sections() {
    // first result of the next / previous section.
    let (mut p, mut l) = open();
    type_str(&mut p, &mut l, "e");
    let sections = p.results().sections.len();
    if sections < 2 {
        return;
    }
    let first_of = |p: &Panel, s: usize| -> usize {
        p.results().sections[..s]
            .iter()
            .map(|x| x.items.len())
            .sum()
    };
    assert_eq!(p.selected(), 0);
    p.handle_key(Key::Down, Mods::PRIMARY, &mut l, now(0.0));
    assert_eq!(p.selected(), first_of(&p, 1));
    assert_eq!(p.focus(), Focus::List);
    p.handle_key(Key::Up, Mods::PRIMARY, &mut l, now(0.0));
    assert_eq!(p.selected(), 0);
    assert_eq!(
        p.handle_key(Key::Up, Mods::PRIMARY, &mut l, now(0.0)),
        PanelEvent::None,
        "no section before the first"
    );
}
