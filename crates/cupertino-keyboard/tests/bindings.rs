//! The text-editing binding table, its override text and the chord-sequence resolver.

use cupertino_keyboard::modifiers::Mods;
use cupertino_keyboard::{BindingTable, Chord, Key, NamedKey, Resolution, Resolver, TextAction};

fn chord(s: &str) -> Chord {
    Chord::parse(s).unwrap()
}

#[test]
fn standard_table_has_a_hundred_rows_over_seventy_five_actions() {
    let t = BindingTable::standard();
    assert_eq!(t.len(), 100);
    let mut used: Vec<TextAction> = t.iter().flat_map(|(_, a)| a.iter().copied()).collect();
    used.sort_unstable();
    used.dedup();
    assert_eq!(used.len(), 75);
}

#[test]
fn noop_is_the_one_action_the_standard_table_leaves_unused() {
    let t = BindingTable::standard();
    let used = |a: TextAction| t.iter().any(|(_, acts)| acts.contains(&a));
    let unused: Vec<TextAction> = TextAction::ALL
        .iter()
        .copied()
        .filter(|a| !used(*a))
        .collect();
    assert_eq!(unused, vec![TextAction::Noop]);
}

#[test]
fn standard_table_binds_emacs_style_control_chords() {
    let t = BindingTable::standard();
    assert_eq!(
        t.get(&[chord("Ctrl+K")]),
        Some(&[TextAction::DeleteToEndOfParagraph][..])
    );
    assert_eq!(
        t.get(&[chord("Ctrl+A")]),
        Some(&[TextAction::MoveToBeginningOfParagraph][..])
    );
}

#[test]
fn standard_table_binds_alt_arrows_to_word_moves() {
    let t = BindingTable::standard();
    assert_eq!(
        t.get(&[chord("Alt+Left")]),
        Some(&[TextAction::MoveWordLeft][..])
    );
    assert_eq!(
        t.get(&[chord("Alt+Shift+Left")]),
        Some(&[TextAction::MoveWordLeftAndModifySelection][..])
    );
}

#[test]
fn standard_table_binds_primary_arrows_to_line_ends_and_shift_extends_selection() {
    let t = BindingTable::standard();
    assert_eq!(
        t.get(&[chord("Primary+Left")]),
        Some(&[TextAction::MoveToLeftEndOfLine][..])
    );
    assert_eq!(
        t.get(&[chord("Primary+Shift+Left")]),
        Some(&[TextAction::MoveToLeftEndOfLineAndModifySelection][..])
    );
}

#[test]
fn text_action_names_round_trip_case_insensitively() {
    for a in TextAction::ALL {
        assert_eq!(TextAction::from_name(a.name()), Some(*a));
    }
    assert_eq!(
        TextAction::from_name("movewordleft"),
        Some(TextAction::MoveWordLeft)
    );
    assert_eq!(TextAction::from_name("NoSuchAction"), None);
}

#[test]
fn bind_get_and_unbind_work_on_sequences() {
    let mut t = BindingTable::empty();
    assert!(t.is_empty());
    let seq = vec![chord("Ctrl+X"), chord("Ctrl+C")];
    t.bind(seq.clone(), vec![TextAction::Noop]);
    assert_eq!(t.len(), 1);
    assert_eq!(t.get(&seq), Some(&[TextAction::Noop][..]));
    assert_eq!(t.iter().count(), 1);
    assert!(t.unbind(&seq));
    assert!(!t.unbind(&seq));
    assert!(t.is_empty());
}

#[test]
fn override_binds_one_action() {
    let mut t = BindingTable::empty();
    let errs = t.apply_overrides("Ctrl+K = DeleteToEndOfParagraph");
    assert!(errs.is_empty());
    assert_eq!(
        t.get(&[chord("Ctrl+K")]),
        Some(&[TextAction::DeleteToEndOfParagraph][..])
    );
}

#[test]
fn override_binds_several_actions_in_order() {
    let mut t = BindingTable::empty();
    let errs = t.apply_overrides("Alt+Left = MoveWordLeft, MoveLeft");
    assert!(errs.is_empty());
    assert_eq!(
        t.get(&[chord("Alt+Left")]),
        Some(&[TextAction::MoveWordLeft, TextAction::MoveLeft][..])
    );
}

#[test]
fn override_binds_a_chord_sequence() {
    let mut t = BindingTable::empty();
    assert!(t.apply_overrides("Ctrl+X Ctrl+C = Noop").is_empty());
    assert_eq!(
        t.get(&[chord("Ctrl+X"), chord("Ctrl+C")]),
        Some(&[TextAction::Noop][..])
    );
}

#[test]
fn override_replaces_a_standard_row() {
    let mut t = BindingTable::standard();
    let rows = t.len();
    assert!(t.apply_overrides("Ctrl+K = Noop").is_empty());
    assert_eq!(t.get(&[chord("Ctrl+K")]), Some(&[TextAction::Noop][..]));
    assert_eq!(t.len(), rows);
}

#[test]
fn override_unbind_removes_a_standard_row_case_insensitively() {
    let mut t = BindingTable::standard();
    assert!(t.apply_overrides("Ctrl+K = UNBIND").is_empty());
    assert_eq!(t.get(&[chord("Ctrl+K")]), None);
    assert_eq!(t.len(), 99);
}

#[test]
fn override_ignores_comments_and_blank_lines() {
    let mut t = BindingTable::empty();
    let errs = t.apply_overrides("# whole line\n\n  \nCtrl+K = Noop # trailing\n");
    assert!(errs.is_empty());
    assert_eq!(t.len(), 1);
}

#[test]
fn override_line_without_equals_is_reported_with_its_line_number() {
    let mut t = BindingTable::empty();
    let errs = t.apply_overrides("Ctrl+K = Noop\nCtrl+J Noop");
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].line, 2);
    assert_eq!(errs[0].message, "expected 'chord = action'");
    assert_eq!(t.len(), 1);
}

#[test]
fn override_bad_chord_is_reported_and_parsing_continues() {
    let mut t = BindingTable::empty();
    let errs = t.apply_overrides("Hyper+K = Noop\nCtrl+Foo = Noop\nCtrl+J = Noop");
    assert_eq!(errs.len(), 2);
    assert_eq!(errs[0].line, 1);
    assert_eq!(errs[0].message, "unknown modifier 'hyper'");
    assert_eq!(errs[1].line, 2);
    assert_eq!(errs[1].message, "unknown key 'Foo'");
    assert_eq!(t.len(), 1);
}

#[test]
fn override_unknown_action_is_reported_and_nothing_is_bound() {
    let mut t = BindingTable::empty();
    let errs = t.apply_overrides("Ctrl+K = Noop, Frobnicate");
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].line, 1);
    assert_eq!(errs[0].message, "unknown action 'Frobnicate'");
    assert!(t.is_empty());
}

#[test]
fn override_with_an_empty_chord_is_reported() {
    let mut t = BindingTable::empty();
    let errs = t.apply_overrides(" = Noop");
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].message, "empty chord");
}

#[test]
fn resolver_returns_the_actions_of_a_single_chord() {
    let mut r = Resolver::new(BindingTable::standard());
    assert_eq!(
        r.feed(chord("Alt+Left")),
        Resolution::Actions(vec![TextAction::MoveWordLeft])
    );
    assert!(!r.is_pending());
}

#[test]
fn resolver_reports_no_match_for_an_unbound_chord() {
    let mut r = Resolver::new(BindingTable::empty());
    assert_eq!(r.feed(chord("Ctrl+Q")), Resolution::NoMatch);
}

#[test]
fn resolver_waits_in_a_sequence_and_completes_it() {
    let mut t = BindingTable::empty();
    t.bind(
        vec![chord("Ctrl+X"), chord("Ctrl+C")],
        vec![TextAction::Noop],
    );
    let mut r = Resolver::new(t);
    assert_eq!(r.feed(chord("Ctrl+X")), Resolution::Pending);
    assert!(r.is_pending());
    assert_eq!(
        r.feed(chord("Ctrl+C")),
        Resolution::Actions(vec![TextAction::Noop])
    );
    assert!(!r.is_pending());
}

#[test]
fn resolver_retries_a_broken_sequence_chord_as_a_fresh_start() {
    let mut t = BindingTable::empty();
    t.bind(
        vec![chord("Ctrl+X"), chord("Ctrl+C")],
        vec![TextAction::Noop],
    );
    t.bind(
        vec![chord("Ctrl+K")],
        vec![TextAction::DeleteToEndOfParagraph],
    );
    let mut r = Resolver::new(t);
    assert_eq!(r.feed(chord("Ctrl+X")), Resolution::Pending);
    assert_eq!(
        r.feed(chord("Ctrl+K")),
        Resolution::Actions(vec![TextAction::DeleteToEndOfParagraph])
    );
    assert!(!r.is_pending());
}

#[test]
fn resolver_reset_drops_a_pending_sequence() {
    let mut t = BindingTable::empty();
    t.bind(
        vec![chord("Ctrl+X"), chord("Ctrl+C")],
        vec![TextAction::Noop],
    );
    let mut r = Resolver::new(t);
    r.feed(chord("Ctrl+X"));
    r.reset();
    assert!(!r.is_pending());
    assert_eq!(r.feed(chord("Ctrl+C")), Resolution::NoMatch);
}

#[test]
fn resolver_exposes_and_edits_its_table() {
    let mut r = Resolver::new(BindingTable::empty());
    r.table_mut().bind(
        vec![Chord::new(Mods::NONE, Key::Named(NamedKey::Tab))],
        vec![TextAction::InsertTab],
    );
    assert_eq!(r.table().len(), 1);
    assert_eq!(
        r.feed(Chord::new(Mods::NONE, Key::Named(NamedKey::Tab))),
        Resolution::Actions(vec![TextAction::InsertTab])
    );
}
