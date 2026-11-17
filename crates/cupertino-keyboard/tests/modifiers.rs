//! Logical modifiers, physical keys, profiles, fn / globe key handling.

use cupertino_keyboard::modifiers::{
    symbol_selectors, GlobeOutcome, Modifier, ModifierMap, Mods, Physical, Profile, RawMods,
    GLOBE_TIMEOUT_MS,
};
use cupertino_keyboard::{FnUsage, GlobeTap};

#[test]
fn mods_combine_compare_and_iterate_in_chord_order() {
    let m = Mods::SHIFT | Mods::ALT;
    assert!(m.contains(Mods::SHIFT | Mods::ALT));
    assert!(!m.contains(Mods::CTRL));
    assert_eq!(m.without(Mods::SHIFT), Mods::ALT);
    assert!(Mods::NONE.is_empty() && !m.is_empty());
    let collected: Mods = [Modifier::Shift, Modifier::Ctrl].into_iter().collect();
    assert_eq!(collected, Mods::SHIFT | Mods::CTRL);

    let all = Mods::FN | Mods::ALT | Mods::SHIFT | Mods::PRIMARY | Mods::CTRL;
    assert_eq!(all.to_string(), "Primary+Ctrl+Alt+Shift+Fn");
    assert_eq!(Mods::NONE.to_string(), "none");
    let order: Vec<Modifier> = (Mods::FN | Mods::ALT | Mods::SHIFT | Mods::PRIMARY)
        .iter()
        .collect();
    assert_eq!(
        order,
        [
            Modifier::Shift,
            Modifier::Primary,
            Modifier::Alt,
            Modifier::Fn
        ]
    );

    let raw = RawMods::from(Physical::Alt) | RawMods::SHIFT;
    assert!(raw.contains(RawMods::ALT) && raw.contains(RawMods::SHIFT));
    assert!(!raw.contains(RawMods::SUPER));
}

#[test]
fn profiles_assign_the_primary_role_and_are_permutations() {
    for (profile, name, primary, ctrl_key, alt_symbols) in [
        (
            Profile::cupertino(),
            "cupertino",
            Physical::Super,
            Physical::Ctrl,
            true,
        ),
        (
            Profile::classic_pc(),
            "classic-pc",
            Physical::Ctrl,
            Physical::Super,
            false,
        ),
    ] {
        assert_eq!(Profile::by_name(name), Some(profile.clone()), "{name}");
        assert_eq!(profile.primary(), primary, "{name}");
        assert_eq!(profile.role(primary), Modifier::Primary, "{name}");
        assert_eq!(profile.role(ctrl_key), Modifier::Ctrl, "{name}");
        assert_eq!(profile.physical(Modifier::Primary), Some(primary), "{name}");
        assert_eq!(profile.logical(primary), Some(Modifier::Primary), "{name}");
        assert_eq!(profile.alt_selects_symbols, alt_symbols, "{name}");
        for (key, role) in [
            (Physical::Shift, Modifier::Shift),
            (Physical::Alt, Modifier::Alt),
            (Physical::Fn, Modifier::Fn),
        ] {
            assert_eq!(profile.role(key), role, "{name}: {key:?} keeps its role");
        }
        for m in Modifier::ALL {
            assert_eq!(profile.role(profile.physical_for(m)), m, "{name}: {m:?}");
        }
        for k in Physical::ALL {
            assert_eq!(profile.physical_for(profile.role(k)), k, "{name}: {k:?}");
        }
    }
    assert_eq!(Profile::by_name("other"), None);
}

#[test]
fn profiles_translate_physical_state_and_name_the_key_of_a_role() {
    let cup = Profile::cupertino();
    let pc = Profile::classic_pc();
    for (profile, raw, mods, row) in [
        (
            &cup,
            RawMods::SUPER | RawMods::SHIFT,
            Mods::PRIMARY | Mods::SHIFT,
            "cupertino Super+Shift",
        ),
        (
            &pc,
            RawMods::SUPER | RawMods::SHIFT,
            Mods::CTRL | Mods::SHIFT,
            "classic Super+Shift",
        ),
        (&cup, RawMods::default(), Mods::NONE, "nothing held"),
        (
            &cup,
            RawMods::CTRL | RawMods::SUPER,
            Mods::PRIMARY | Mods::CTRL,
            "cupertino Ctrl+Super",
        ),
        (
            &pc,
            RawMods::CTRL | RawMods::SUPER,
            Mods::PRIMARY | Mods::CTRL,
            "classic Ctrl+Super",
        ),
    ] {
        assert_eq!(profile.translate(raw), mods, "{row}");
    }
    for (profile, role, key) in [
        (&cup, Modifier::Primary, "Super"),
        (&pc, Modifier::Primary, "Ctrl"),
        (&pc, Modifier::Ctrl, "Super"),
    ] {
        assert_eq!(profile.display(role), key, "{} {role:?}", profile.name);
    }
}

#[test]
fn symbol_selectors_drop_shift_under_primary_alone() {
    // (shift selects, alt selects)
    for (mods, selectors) in [
        (Mods::PRIMARY | Mods::SHIFT, (false, false)),
        (Mods::PRIMARY | Mods::ALT, (false, true)),
        (Mods::PRIMARY | Mods::ALT | Mods::SHIFT, (true, true)),
        (Mods::SHIFT, (true, false)),
        (Mods::ALT, (false, true)),
        (Mods::NONE, (false, false)),
    ] {
        assert_eq!(symbol_selectors(mods), selectors, "{mods}");
    }
}

#[test]
fn fn_usage_reads_the_preference_and_defaults_by_source_count() {
    for v in 0..=3 {
        assert_eq!(
            FnUsage::from_pref(v).map(FnUsage::to_pref),
            Some(v),
            "pref {v}"
        );
    }
    assert_eq!(FnUsage::from_pref(2), Some(FnUsage::ShowEmojiPicker));
    for v in [4, -1] {
        assert_eq!(FnUsage::from_pref(v), None, "pref {v}");
    }
    for (sources, usage) in [
        (0, FnUsage::DoNothing),
        (1, FnUsage::DoNothing),
        (2, FnUsage::ChangeInputSource),
        (5, FnUsage::ChangeInputSource),
    ] {
        assert_eq!(FnUsage::default_for(sources), usage, "{sources} sources");
    }
}

#[test]
fn globe_release_is_a_tap_only_when_quick_and_alone() {
    let mut quick = GlobeTap::default();
    quick.down(1000.0);
    assert_eq!(quick.up(1200.0), GlobeOutcome::Tap, "quick release");

    let mut g = GlobeTap::new(250.0);
    g.down(0.0);
    assert_eq!(g.up(250.0), GlobeOutcome::Tap, "release at the threshold");
    g.down(0.0);
    assert_eq!(
        g.up(251.0),
        GlobeOutcome::Debounced,
        "release past the threshold"
    );

    let mut g = GlobeTap::default();
    assert_eq!(g.up(10.0), GlobeOutcome::Ignored, "release without a press");
    g.down(0.0);
    g.other_key();
    assert_eq!(g.up(10.0), GlobeOutcome::Ignored, "another key in between");

    assert_eq!(GlobeTap::new(5000.0).threshold_ms(), GLOBE_TIMEOUT_MS);
    assert_eq!(GlobeTap::new(300.0).threshold_ms(), 300.0);
}
