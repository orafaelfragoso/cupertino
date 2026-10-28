//! Modifier semantics: shortcut symbol selection and the fn / globe key. The profile that maps physical keys to logical
//! roles is `cupertino_modifiers::Profile`.
//!
//! the design layouts select symbols from eight tables addressed by shift, caps lock and
//! alt; shortcut matching ignores shift when the primary modifier is down;
//! the fn / globe key usage values and the tap debounce come from.

use cupertino_modifiers::Mods;

/// Which symbol-selecting modifiers apply when matching the symbol of a shortcut (on the design
/// layouts): with the primary modifier alone the unshifted symbol is used even if shift is down; primary+alt keeps
/// the alt symbol (with dead keys resolved to their spacing form); primary+alt+shift keeps alt+shift.
pub fn symbol_selectors(mods: Mods) -> (bool, bool) {
    let shift = mods.contains(Mods::SHIFT);
    let alt = mods.contains(Mods::ALT);
    if mods.contains(Mods::PRIMARY) && !alt {
        (false, false)
    } else {
        (shift, alt)
    }
}

/// Action of the fn / globe key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FnUsage {
    DoNothing,
    ChangeInputSource,
    ShowEmojiPicker,
    StartDictation,
}

impl FnUsage {
    /// Value 0..=3 of the preference; anything else is "not configured".
    pub fn from_pref(v: i64) -> Option<FnUsage> {
        match v {
            0 => Some(FnUsage::DoNothing),
            1 => Some(FnUsage::ChangeInputSource),
            2 => Some(FnUsage::ShowEmojiPicker),
            3 => Some(FnUsage::StartDictation),
            _ => None,
        }
    }

    pub fn to_pref(self) -> i64 {
        match self {
            FnUsage::DoNothing => 0,
            FnUsage::ChangeInputSource => 1,
            FnUsage::ShowEmojiPicker => 2,
            FnUsage::StartDictation => 3,
        }
    }

    /// Default when the preference is unset and several input sources can be selected: change input source
    /// (derived from the decision tree in the reference implementation).
    pub fn default_for(selectable_sources: usize) -> FnUsage {
        if selectable_sources >= 2 {
            FnUsage::ChangeInputSource
        } else {
            FnUsage::DoNothing
        }
    }
}

/// Upper bound of the globe tap window in ms (the internal timeout of the translator, 2.0 s).
pub const GLOBE_TIMEOUT_MS: f64 = 2000.0;
/// Initial-delay stand-in used before the key repeat delay is configured (250 ms).
pub const GLOBE_DEFAULT_REPEAT_MS: f64 = 250.0;

/// Result of releasing the globe / fn key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobeOutcome {
    /// Quick press: dispatch the globe key action.
    Tap,
    /// Held too long (longer than min(2 s, initial key-repeat delay)): swallowed.
    Debounced,
    /// Release without a matching press, or after another key was used with it.
    Ignored,
}

/// Globe key tap detector.
#[derive(Clone, Copy, Debug)]
pub struct GlobeTap {
    pub initial_repeat_ms: f64,
    down_at_ms: Option<f64>,
}

impl Default for GlobeTap {
    fn default() -> Self {
        GlobeTap {
            initial_repeat_ms: GLOBE_DEFAULT_REPEAT_MS,
            down_at_ms: None,
        }
    }
}

impl GlobeTap {
    pub fn new(initial_repeat_ms: f64) -> GlobeTap {
        GlobeTap {
            initial_repeat_ms,
            down_at_ms: None,
        }
    }

    pub fn threshold_ms(&self) -> f64 {
        GLOBE_TIMEOUT_MS.min(self.initial_repeat_ms)
    }

    pub fn down(&mut self, time_ms: f64) {
        self.down_at_ms = Some(time_ms);
    }

    /// Another key went down or up while the globe key was held: its release is no tap.
    pub fn other_key(&mut self) {
        self.down_at_ms = None;
    }

    pub fn up(&mut self, time_ms: f64) -> GlobeOutcome {
        match self.down_at_ms.take() {
            None => GlobeOutcome::Ignored,
            Some(t) if time_ms - t > self.threshold_ms() => GlobeOutcome::Debounced,
            Some(_) => GlobeOutcome::Tap,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_selection_with_primary() {
        assert_eq!(
            symbol_selectors(Mods::PRIMARY | Mods::SHIFT),
            (false, false)
        );
        assert_eq!(symbol_selectors(Mods::PRIMARY | Mods::ALT), (false, true));
        assert_eq!(
            symbol_selectors(Mods::PRIMARY | Mods::ALT | Mods::SHIFT),
            (true, true)
        );
        assert_eq!(symbol_selectors(Mods::SHIFT), (true, false));
    }

    #[test]
    fn fn_usage_values() {
        for v in 0..4 {
            assert_eq!(FnUsage::from_pref(v).unwrap().to_pref(), v);
        }
        assert_eq!(FnUsage::from_pref(4), None);
        assert_eq!(FnUsage::default_for(3), FnUsage::ChangeInputSource);
        assert_eq!(FnUsage::default_for(1), FnUsage::DoNothing);
    }

    #[test]
    fn globe_tap_debounce() {
        let mut g = GlobeTap::new(375.0);
        g.down(1000.0);
        assert_eq!(g.up(1300.0), GlobeOutcome::Tap);
        g.down(1000.0);
        assert_eq!(g.up(1400.0), GlobeOutcome::Debounced);
        assert_eq!(g.up(1500.0), GlobeOutcome::Ignored);
        g.down(1000.0);
        g.other_key();
        assert_eq!(
            g.up(1100.0),
            GlobeOutcome::Ignored,
            "a chord with another key is no tap"
        );
        let mut slow = GlobeTap::new(5000.0);
        assert_eq!(slow.threshold_ms(), 2000.0);
        slow.down(0.0);
        assert_eq!(slow.up(1999.0), GlobeOutcome::Tap);
    }
}
