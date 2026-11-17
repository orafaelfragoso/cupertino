//! Logical modifiers, physical keys, profiles, fn / globe key handling.

use cupertino_keyboard::modifiers::{
    symbol_selectors, GlobeOutcome, Modifier, ModifierMap, Mods, Physical, Profile, RawMods,
    GLOBE_DEFAULT_REPEAT_MS, GLOBE_TIMEOUT_MS,
};
use cupertino_keyboard::{FnUsage, GlobeTap};

#[test]
fn mods_union_contains_and_without() {
    let m = Mods::SHIFT | Mods::ALT;
    assert!(m.contains(Mods::SHIFT));
    assert!(m.contains(Mods::SHIFT | Mods::ALT));
    assert!(!m.contains(Mods::CTRL));
    assert_eq!(m.without(Mods::SHIFT), Mods::ALT);
    assert!(Mods::NONE.is_empty());
    assert!(!m.is_empty());
}

#[test]
fn mods_display_in_chord_order() {
    let all = Mods::SHIFT | Mods::PRIMARY | Mods::CTRL | Mods::ALT | Mods::FN;
    assert_eq!(all.to_string(), "Primary+Ctrl+Alt+Shift+Fn");
    assert_eq!(Mods::NONE.to_string(), "none");
}

#[test]
fn mods_iterate_in_modifier_order() {
    let m = Mods::FN | Mods::ALT | Mods::SHIFT | Mods::PRIMARY;
    let order: Vec<Modifier> = m.iter().collect();
    assert_eq!(
        order,
        vec![
            Modifier::Shift,
            Modifier::Primary,
            Modifier::Alt,
            Modifier::Fn
        ]
    );
}

#[test]
fn mods_collect_from_modifiers() {
    let m: Mods = [Modifier::Shift, Modifier::Ctrl].into_iter().collect();
    assert_eq!(m, Mods::SHIFT | Mods::CTRL);
    assert_eq!(Mods::from(Modifier::Alt), Mods::ALT);
    assert_eq!(Mods::PRIMARY.bits(), 8);
}

#[test]
fn cupertino_profile_makes_super_the_primary_key() {
    let p = Profile::cupertino();
    assert_eq!(p.name, "cupertino");
    assert_eq!(p.role(Physical::Super), Modifier::Primary);
    assert_eq!(p.role(Physical::Ctrl), Modifier::Ctrl);
    assert_eq!(p.primary(), Physical::Super);
    assert!(p.alt_selects_symbols);
}

#[test]
fn classic_pc_profile_swaps_ctrl_and_super() {
    let p = Profile::classic_pc();
    assert_eq!(p.name, "classic-pc");
    assert_eq!(p.role(Physical::Ctrl), Modifier::Primary);
    assert_eq!(p.role(Physical::Super), Modifier::Ctrl);
    assert_eq!(p.primary(), Physical::Ctrl);
    assert!(!p.alt_selects_symbols);
}

#[test]
fn shift_alt_and_fn_keep_their_role_under_every_profile() {
    for p in [Profile::cupertino(), Profile::classic_pc()] {
        assert_eq!(p.role(Physical::Shift), Modifier::Shift);
        assert_eq!(p.role(Physical::Alt), Modifier::Alt);
        assert_eq!(p.role(Physical::Fn), Modifier::Fn);
    }
}

#[test]
fn profiles_are_permutations() {
    for p in [Profile::cupertino(), Profile::classic_pc()] {
        for m in Modifier::ALL {
            assert_eq!(p.role(p.physical_for(m)), m);
        }
        for k in Physical::ALL {
            assert_eq!(p.physical_for(p.role(k)), k);
        }
    }
}

#[test]
fn profile_by_name_finds_both_and_rejects_others() {
    assert_eq!(Profile::by_name("cupertino"), Some(Profile::cupertino()));
    assert_eq!(Profile::by_name("classic-pc"), Some(Profile::classic_pc()));
    assert_eq!(Profile::by_name("other"), None);
}

#[test]
fn translate_maps_physical_state_to_logical_modifiers() {
    let raw = RawMods::SUPER | RawMods::SHIFT;
    assert_eq!(
        Profile::cupertino().translate(raw),
        Mods::PRIMARY | Mods::SHIFT
    );
    assert_eq!(
        Profile::classic_pc().translate(raw),
        Mods::CTRL | Mods::SHIFT
    );
    assert_eq!(
        Profile::cupertino().translate(RawMods::default()),
        Mods::NONE
    );
}

#[test]
fn ctrl_and_super_together_translate_to_primary_and_ctrl_under_both_profiles() {
    let raw = RawMods::CTRL | RawMods::SUPER;
    for p in [Profile::cupertino(), Profile::classic_pc()] {
        assert_eq!(p.translate(raw), Mods::PRIMARY | Mods::CTRL);
    }
}

#[test]
fn display_names_the_key_that_carries_a_role() {
    assert_eq!(Profile::cupertino().display(Modifier::Primary), "Super");
    assert_eq!(Profile::classic_pc().display(Modifier::Primary), "Ctrl");
    assert_eq!(Profile::classic_pc().display(Modifier::Ctrl), "Super");
}

#[test]
fn profile_implements_modifier_map() {
    let p = Profile::cupertino();
    assert_eq!(p.physical(Modifier::Primary), Some(Physical::Super));
    assert_eq!(p.logical(Physical::Super), Some(Modifier::Primary));
}

#[test]
fn raw_mods_contain_what_was_combined() {
    let r = RawMods::from(Physical::Alt) | RawMods::SHIFT;
    assert!(r.contains(RawMods::ALT));
    assert!(r.contains(RawMods::SHIFT));
    assert!(!r.contains(RawMods::SUPER));
}

#[test]
fn primary_alone_ignores_shift_for_the_symbol() {
    assert_eq!(
        symbol_selectors(Mods::PRIMARY | Mods::SHIFT),
        (false, false)
    );
}

#[test]
fn primary_with_alt_keeps_alt_and_shift() {
    assert_eq!(symbol_selectors(Mods::PRIMARY | Mods::ALT), (false, true));
    assert_eq!(
        symbol_selectors(Mods::PRIMARY | Mods::ALT | Mods::SHIFT),
        (true, true)
    );
}

#[test]
fn without_primary_shift_and_alt_select_symbols() {
    assert_eq!(symbol_selectors(Mods::SHIFT), (true, false));
    assert_eq!(symbol_selectors(Mods::ALT), (false, true));
    assert_eq!(symbol_selectors(Mods::NONE), (false, false));
}

#[test]
fn fn_usage_round_trips_the_preference_values() {
    for v in 0..=3 {
        assert_eq!(FnUsage::from_pref(v).map(FnUsage::to_pref), Some(v));
    }
    assert_eq!(FnUsage::from_pref(4), None);
    assert_eq!(FnUsage::from_pref(-1), None);
    assert_eq!(FnUsage::from_pref(2), Some(FnUsage::ShowEmojiPicker));
}

#[test]
fn fn_usage_defaults_to_input_source_change_with_two_sources() {
    assert_eq!(FnUsage::default_for(2), FnUsage::ChangeInputSource);
    assert_eq!(FnUsage::default_for(5), FnUsage::ChangeInputSource);
    assert_eq!(FnUsage::default_for(1), FnUsage::DoNothing);
    assert_eq!(FnUsage::default_for(0), FnUsage::DoNothing);
}

#[test]
fn globe_quick_release_is_a_tap() {
    let mut g = GlobeTap::default();
    assert_eq!(g.initial_repeat_ms, GLOBE_DEFAULT_REPEAT_MS);
    g.down(1000.0);
    assert_eq!(g.up(1200.0), GlobeOutcome::Tap);
}

#[test]
fn globe_release_after_the_threshold_is_debounced() {
    let mut g = GlobeTap::new(250.0);
    g.down(0.0);
    assert_eq!(g.up(250.0), GlobeOutcome::Tap);
    g.down(0.0);
    assert_eq!(g.up(251.0), GlobeOutcome::Debounced);
}

#[test]
fn globe_threshold_is_capped_at_two_seconds() {
    assert_eq!(GlobeTap::new(5000.0).threshold_ms(), GLOBE_TIMEOUT_MS);
    assert_eq!(GlobeTap::new(300.0).threshold_ms(), 300.0);
}

#[test]
fn globe_release_without_press_or_after_another_key_is_ignored() {
    let mut g = GlobeTap::default();
    assert_eq!(g.up(10.0), GlobeOutcome::Ignored);
    g.down(0.0);
    g.other_key();
    assert_eq!(g.up(10.0), GlobeOutcome::Ignored);
}
