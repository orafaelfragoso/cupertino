//! The text-editing binding table, its override text and the chord-sequence resolver.

use cupertino_keyboard::{BindingTable, Chord, Resolution, Resolver, TextAction};

fn chord(s: &str) -> Chord {
    Chord::parse(s).unwrap()
}

fn seq(s: &str) -> Vec<Chord> {
    s.split(' ').map(chord).collect()
}

#[test]
fn the_standard_table_binds_the_platform_editing_chords() {
    let t = BindingTable::standard();
    for (chord_text, action) in [
        ("Ctrl+K", TextAction::DeleteToEndOfParagraph),
        ("Ctrl+A", TextAction::MoveToBeginningOfParagraph),
        ("Alt+Left", TextAction::MoveWordLeft),
        ("Alt+Shift+Left", TextAction::MoveWordLeftAndModifySelection),
        ("Primary+Left", TextAction::MoveToLeftEndOfLine),
        (
            "Primary+Shift+Left",
            TextAction::MoveToLeftEndOfLineAndModifySelection,
        ),
    ] {
        assert_eq!(
            t.get(&[chord(chord_text)]),
            Some(&[action][..]),
            "{chord_text}"
        );
    }
}

#[test]
fn bind_get_and_unbind_work_on_sequences() {
    let mut t = BindingTable::empty();
    assert!(t.is_empty());
    let s = seq("Ctrl+X Ctrl+C");
    t.bind(s.clone(), vec![TextAction::Noop]);
    assert_eq!(t.len(), 1);
    assert_eq!(t.get(&s), Some(&[TextAction::Noop][..]));
    assert_eq!(t.iter().count(), 1);
    assert!(t.unbind(&s));
    assert!(!t.unbind(&s));
    assert!(t.is_empty());
}

#[test]
fn overrides_bind_replace_and_unbind_rows() {
    let standard_rows = BindingTable::standard().len();
    for (start, text, sequence, actions, rows) in [
        (
            BindingTable::empty(),
            "Ctrl+K = DeleteToEndOfParagraph",
            "Ctrl+K",
            Some(vec![TextAction::DeleteToEndOfParagraph]),
            1,
        ),
        (
            BindingTable::empty(),
            "Alt+Left = MoveWordLeft, MoveLeft",
            "Alt+Left",
            Some(vec![TextAction::MoveWordLeft, TextAction::MoveLeft]),
            1,
        ),
        (
            BindingTable::empty(),
            "Ctrl+X Ctrl+C = Noop",
            "Ctrl+X Ctrl+C",
            Some(vec![TextAction::Noop]),
            1,
        ),
        (
            BindingTable::empty(),
            "Ctrl+K = movewordleft",
            "Ctrl+K",
            Some(vec![TextAction::MoveWordLeft]),
            1,
        ),
        (
            BindingTable::empty(),
            "# whole line\n\n  \nCtrl+K = Noop # trailing\n",
            "Ctrl+K",
            Some(vec![TextAction::Noop]),
            1,
        ),
        (
            BindingTable::standard(),
            "Ctrl+K = Noop",
            "Ctrl+K",
            Some(vec![TextAction::Noop]),
            standard_rows,
        ),
        (
            BindingTable::standard(),
            "Ctrl+K = UNBIND",
            "Ctrl+K",
            None,
            standard_rows - 1,
        ),
    ] {
        let mut t = start;
        assert_eq!(t.apply_overrides(text), Vec::new(), "{text:?}");
        assert_eq!(
            t.get(&seq(sequence)).map(<[TextAction]>::to_vec),
            actions,
            "{text:?}"
        );
        assert_eq!(t.len(), rows, "{text:?}");
    }
    for a in TextAction::ALL {
        let mut t = BindingTable::empty();
        assert!(
            t.apply_overrides(&format!("Ctrl+K = {}", a.name()))
                .is_empty(),
            "{}",
            a.name()
        );
    }
}

#[test]
fn override_problems_are_reported_by_line_and_the_rest_still_applies() {
    for (text, problems, rows) in [
        (
            "Ctrl+K = Noop\nCtrl+J Noop",
            vec![(2, "expected 'chord = action'")],
            1,
        ),
        (
            "Hyper+K = Noop\nCtrl+Foo = Noop\nCtrl+J = Noop",
            vec![(1, "unknown modifier 'hyper'"), (2, "unknown key 'Foo'")],
            1,
        ),
        (
            "Ctrl+K = Noop, Frobnicate",
            vec![(1, "unknown action 'Frobnicate'")],
            0,
        ),
        (" = Noop", vec![(1, "empty chord")], 0),
    ] {
        let mut t = BindingTable::empty();
        let errs = t.apply_overrides(text);
        let got: Vec<(usize, &str)> = errs.iter().map(|e| (e.line, e.message.as_str())).collect();
        assert_eq!(got, problems, "{text:?}");
        assert_eq!(t.len(), rows, "{text:?}");
    }
}

#[test]
fn the_resolver_completes_single_chords_and_sequences() {
    let mut t = BindingTable::empty();
    t.bind(seq("Ctrl+X Ctrl+C"), vec![TextAction::Noop]);
    t.bind(seq("Ctrl+K"), vec![TextAction::DeleteToEndOfParagraph]);
    let mut r = Resolver::new(t);

    assert_eq!(
        r.feed(chord("Ctrl+K")),
        Resolution::Actions(vec![TextAction::DeleteToEndOfParagraph])
    );
    assert!(!r.is_pending());
    assert_eq!(r.feed(chord("Ctrl+Q")), Resolution::NoMatch, "unbound");

    assert_eq!(r.feed(chord("Ctrl+X")), Resolution::Pending);
    assert!(r.is_pending());
    assert_eq!(
        r.feed(chord("Ctrl+C")),
        Resolution::Actions(vec![TextAction::Noop]),
        "sequence completes"
    );

    r.feed(chord("Ctrl+X"));
    assert_eq!(
        r.feed(chord("Ctrl+K")),
        Resolution::Actions(vec![TextAction::DeleteToEndOfParagraph]),
        "a chord that breaks the sequence is retried as a fresh start"
    );
    assert!(!r.is_pending());

    r.feed(chord("Ctrl+X"));
    r.reset();
    assert!(!r.is_pending());
    assert_eq!(
        r.feed(chord("Ctrl+C")),
        Resolution::NoMatch,
        "reset drops the pending sequence"
    );

    r.table_mut().bind(seq("Tab"), vec![TextAction::InsertTab]);
    assert_eq!(r.table().len(), 3);
    assert_eq!(
        r.feed(chord("Tab")),
        Resolution::Actions(vec![TextAction::InsertTab]),
        "an edited table is used at once"
    );
}
