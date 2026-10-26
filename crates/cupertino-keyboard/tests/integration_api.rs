//! Mirrors the integration snippet of the README.

use cupertino_keyboard::*;

#[test]
fn compositor_flow() {
    let cfg = RepeatConfig::from_pref_units(25.0, 6.0);
    let RepeatInfo { rate, delay } = cfg.repeat_info();
    assert_eq!((rate, delay), (10, 416));

    let layout = Layout::from_json(include_str!("test_layout.json")).unwrap();
    let out = generate(&layout, &GenOptions { keyboard: Keyboard::Iso, ..Default::default() }).unwrap();
    assert!(out.keymap.starts_with("xkb_keymap"));

    let profile = ModifierProfile::cupertino();
    let mods = profile.translate(RawMods::SUPER | RawMods::SHIFT);
    assert_eq!(mods, Mods::PRIMARY | Mods::SHIFT);
    let shortcuts = ShortcutSet::reference_defaults();
    assert_eq!(shortcuts.lookup(&Chord::new(mods, Key::Char('4'))), Some(SystemAction::ScreenshotSelectionToFile));

    let mut resolver = Resolver::new(BindingTable::standard());
    assert_eq!(resolver.feed(Chord::parse("Alt+Left").unwrap()), Resolution::Actions(vec![TextAction::MoveWordLeft]));

    let mut sw = Switcher::new();
    sw.set_sources(vec!["abc".into(), "fr".into(), "de".into()]);
    sw.set_current("abc");
    assert!(matches!(sw.press(Step::Next, true), SwitchEvent::ShowHud { selection: 1, .. }));
    assert_eq!(sw.modifiers_released(), SwitchEvent::Commit { id: "fr".into() });
}

#[test]
fn picker_with_hand_authored_table() {
    let mut table = AccentTable::new();
    table.insert('o', accent::Direction::Left, ["o", "ò", "ó"].iter().map(|s| s.to_string()).collect());
    let mut p = AccentPicker::new(AccentConfig::default(), table);
    let ev = accent::KeyDown { ch: Some('o'), key: accent::PickerKey::Char, repeat: true, mods: Mods::NONE };
    assert!(matches!(p.key_down(&ev, "any"), AccentEvent::Show { .. }));
    let two = accent::KeyDown { ch: Some('2'), key: accent::PickerKey::Char, repeat: false, mods: Mods::NONE };
    assert_eq!(p.key_down(&two, "any"), AccentEvent::Commit { text: "ó".into() });
}
