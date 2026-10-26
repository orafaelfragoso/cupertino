//! Input-source switcher: most-recently-used ordering, HUD interaction and timing.
//!
//! Behaviour:
//!
//! * two desktop shortcuts drive the switcher: "select previous source" and "select next source in the menu"
//!   (default Ctrl+Space and Ctrl+Alt+Space, see [`crate::shortcuts::SystemAction::InputSourcePrevious`]); a key with
//!   no modifier (the globe key) switches immediately;
//! * the first press opens a HUD and moves the selection one step; further presses move it further; the HUD stays up
//!   while the shortcut's modifiers are held and the displayed source is committed when they are released;
//! * the HUD lists the sources in *historical usage order* (recent first, then the remaining ones in menu order);
//! * Escape closes the HUD without switching;
//! * the HUD panel fades out in 0.3 s; a watchdog timer of the same length orders the panel out if the animation
//!   does not finish.
//!
//! HUD material (blur recipe), shadow and icon artwork are unknown; the geometry constants below are the
//! numeric values found in the HUD code (their role is derived from the method they live in).

/// Fade-out duration and watchdog interval of the HUD panel, seconds.
pub const HUD_FADE_OUT_S: f64 = 0.3;
/// Corner radius of the rounded HUD content view, points.
pub const HUD_CORNER_RADIUS: f64 = 24.0;
/// Row height of the source list, points.
pub const HUD_ROW_HEIGHT: f64 = 40.0;
/// Lower bound of the computed content width, points.
pub const HUD_MIN_WIDTH: f64 = 167.0;
/// Content padding, points.
pub const HUD_PADDING: f64 = 10.0;
/// White level / alpha of the panel background tint (pattern image of a rounded rectangle filled with white at 0.35 alpha).
pub const HUD_TINT_WHITE_ALPHA: (f64, f64) = (1.0, 0.35);
/// Radius of the rounded rectangle of the background pattern image, points.
pub const HUD_BACKGROUND_PATTERN_RADIUS: f64 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Next,
    Previous,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitchEvent {
    None,
    /// Show the HUD with this order and selection.
    ShowHud {
        order: Vec<String>,
        selection: usize,
    },
    /// The selection moved.
    MoveSelection {
        selection: usize,
    },
    /// Select this source now (and close the HUD if it is open).
    Commit {
        id: String,
    },
    /// Close the HUD without switching.
    Cancel,
}

#[derive(Clone, Debug)]
struct Hud {
    order: Vec<String>,
    selection: usize,
    hold: bool,
}

/// MRU list plus HUD state machine.
#[derive(Clone, Debug, Default)]
pub struct Switcher {
    menu_order: Vec<String>,
    mru: Vec<String>,
    current: Option<String>,
    hud: Option<Hud>,
}

impl Switcher {
    pub fn new() -> Switcher {
        Switcher::default()
    }

    /// Replaces the enabled sources (menu order). Unknown ids drop out of the history.
    pub fn set_sources(&mut self, sources: Vec<String>) {
        self.mru.retain(|m| sources.contains(m));
        if let Some(c) = &self.current {
            if !sources.contains(c) {
                self.current = None;
            }
        }
        self.menu_order = sources;
    }

    /// Records that a source became current (outside the HUD too).
    pub fn set_current(&mut self, id: &str) {
        if !self.menu_order.iter().any(|s| s == id) {
            return;
        }
        self.mru.retain(|m| m != id);
        self.mru.insert(0, id.to_string());
        self.current = Some(id.to_string());
    }

    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// Display order: history first, remaining sources in menu order.
    pub fn historical_order(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .mru
            .iter()
            .filter(|m| self.menu_order.contains(m))
            .cloned()
            .collect();
        for s in &self.menu_order {
            if !v.contains(s) {
                v.push(s.clone());
            }
        }
        v
    }

    pub fn is_hud_open(&self) -> bool {
        self.hud.is_some()
    }

    pub fn selection(&self) -> Option<usize> {
        self.hud.as_ref().map(|h| h.selection)
    }

    /// A switch shortcut was pressed. `modifiers_held` tells whether the shortcut includes modifiers that stay down
    /// (then the HUD waits for their release); without modifiers the switch is immediate.
    pub fn press(&mut self, step: Step, modifiers_held: bool) -> SwitchEvent {
        let n = self.menu_order.len();
        if n < 2 {
            return SwitchEvent::None;
        }
        if let Some(h) = self.hud.as_mut() {
            h.selection = advance(h.selection, n, step);
            return SwitchEvent::MoveSelection {
                selection: h.selection,
            };
        }
        let order = self.historical_order();
        let cur = self
            .current
            .as_ref()
            .and_then(|c| order.iter().position(|o| o == c))
            .unwrap_or(0);
        let sel = advance(cur, n, step);
        if !modifiers_held {
            let id = order[sel].clone();
            self.set_current(&id);
            return SwitchEvent::Commit { id };
        }
        self.hud = Some(Hud {
            order: order.clone(),
            selection: sel,
            hold: true,
        });
        SwitchEvent::ShowHud {
            order,
            selection: sel,
        }
    }

    /// The "next source in the menu" shortcut: like [`Switcher::press`], but the first press selects the source that
    /// follows the current one in *menu* order instead of the previously used one (the second shortcut
    /// of the pair; which order it walks is derived from its name). While the HUD is open it steps like `press`.
    pub fn press_menu(&mut self, step: Step, modifiers_held: bool) -> SwitchEvent {
        let n = self.menu_order.len();
        if n < 2 {
            return SwitchEvent::None;
        }
        if self.hud.is_some() {
            return self.press(step, modifiers_held);
        }
        let order = self.historical_order();
        let cur_id = self.current.clone().unwrap_or_else(|| order[0].clone());
        let menu_pos = self
            .menu_order
            .iter()
            .position(|s| *s == cur_id)
            .unwrap_or(0);
        let target = &self.menu_order[advance(menu_pos, n, step)];
        let sel = order.iter().position(|o| o == target).unwrap_or(0);
        if !modifiers_held {
            let id = order[sel].clone();
            self.set_current(&id);
            return SwitchEvent::Commit { id };
        }
        self.hud = Some(Hud {
            order: order.clone(),
            selection: sel,
            hold: true,
        });
        SwitchEvent::ShowHud {
            order,
            selection: sel,
        }
    }

    /// Mouse hover / click on a row.
    pub fn select(&mut self, index: usize) -> SwitchEvent {
        match self.hud.as_mut() {
            Some(h) if index < h.order.len() => {
                h.selection = index;
                SwitchEvent::MoveSelection { selection: index }
            }
            _ => SwitchEvent::None,
        }
    }

    /// The modifiers of the shortcut were released: commit the displayed source.
    pub fn modifiers_released(&mut self) -> SwitchEvent {
        match self.hud.take() {
            Some(h) if h.hold => {
                let id = h.order[h.selection].clone();
                self.set_current(&id);
                SwitchEvent::Commit { id }
            }
            _ => SwitchEvent::None,
        }
    }

    /// Commit with the keyboard (Return) or mouse while the HUD is open.
    pub fn commit(&mut self) -> SwitchEvent {
        self.modifiers_released()
    }

    pub fn escape(&mut self) -> SwitchEvent {
        match self.hud.take() {
            Some(_) => SwitchEvent::Cancel,
            None => SwitchEvent::None,
        }
    }
}

fn advance(cur: usize, n: usize, step: Step) -> usize {
    match step {
        Step::Next => (cur + 1) % n,
        Step::Previous => (cur + n - 1) % n,
    }
}

/// Alpha of the HUD during fade-out: 1 → 0 linearly over [`HUD_FADE_OUT_S`] (the animation curve is not known,
/// approximate as linear).
pub fn fade_alpha(elapsed_s: f64) -> f64 {
    (1.0 - elapsed_s / HUD_FADE_OUT_S).clamp(0.0, 1.0)
}

/// Content size of the HUD for `rows` sources whose widest label is `max_label_w` points wide
/// (`icon + gap` is added by the caller in `label_extra`).
pub fn hud_content_size(rows: usize, max_label_w: f64, label_extra: f64) -> (f64, f64) {
    let w = (max_label_w + label_extra + 2.0 * HUD_PADDING).max(HUD_MIN_WIDTH);
    let h = rows as f64 * HUD_ROW_HEIGHT + 2.0 * HUD_PADDING;
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sw() -> Switcher {
        let mut s = Switcher::new();
        s.set_sources(vec!["abc".into(), "fr".into(), "de".into(), "ja".into()]);
        s.set_current("abc");
        s
    }

    #[test]
    fn historical_order_puts_recent_first() {
        let mut s = sw();
        s.set_current("de");
        s.set_current("abc");
        assert_eq!(s.historical_order(), vec!["abc", "de", "fr", "ja"]);
        s.set_sources(vec!["abc".into(), "fr".into(), "ja".into()]);
        assert_eq!(s.historical_order(), vec!["abc", "fr", "ja"]);
    }

    #[test]
    fn hold_flow_commits_on_release() {
        let mut s = sw();
        s.set_current("fr");
        s.set_current("abc");
        let e = s.press(Step::Next, true);
        assert_eq!(
            e,
            SwitchEvent::ShowHud {
                order: vec!["abc".into(), "fr".into(), "de".into(), "ja".into()],
                selection: 1
            }
        );
        assert_eq!(
            s.press(Step::Next, true),
            SwitchEvent::MoveSelection { selection: 2 }
        );
        assert_eq!(
            s.press(Step::Previous, true),
            SwitchEvent::MoveSelection { selection: 1 }
        );
        assert_eq!(
            s.modifiers_released(),
            SwitchEvent::Commit { id: "fr".into() }
        );
        assert_eq!(s.current(), Some("fr"));
        assert!(!s.is_hud_open());
        assert_eq!(s.historical_order()[0], "fr");
    }

    #[test]
    fn previous_wraps() {
        let mut s = sw();
        assert_eq!(
            s.press(Step::Previous, true),
            SwitchEvent::ShowHud {
                order: s.historical_order(),
                selection: 3
            }
        );
    }

    #[test]
    fn immediate_switch_without_modifiers() {
        let mut s = sw();
        assert_eq!(
            s.press(Step::Next, false),
            SwitchEvent::Commit { id: "fr".into() }
        );
        assert!(!s.is_hud_open());
        assert_eq!(s.current(), Some("fr"));
    }

    #[test]
    fn escape_cancels_without_switch() {
        let mut s = sw();
        s.press(Step::Next, true);
        assert_eq!(s.escape(), SwitchEvent::Cancel);
        assert_eq!(s.current(), Some("abc"));
        assert_eq!(s.escape(), SwitchEvent::None);
    }

    #[test]
    fn single_source_never_switches() {
        let mut s = Switcher::new();
        s.set_sources(vec!["abc".into()]);
        assert_eq!(s.press(Step::Next, true), SwitchEvent::None);
    }

    #[test]
    fn mouse_selection_and_commit() {
        let mut s = sw();
        s.press(Step::Next, true);
        assert_eq!(s.select(3), SwitchEvent::MoveSelection { selection: 3 });
        assert_eq!(s.select(9), SwitchEvent::None);
        assert_eq!(s.commit(), SwitchEvent::Commit { id: "ja".into() });
    }

    #[test]
    fn menu_shortcut_walks_the_menu_order() {
        let mut s = sw();
        s.set_current("de");
        s.set_current("fr");
        // history: fr, de, abc, ja; the menu order after "fr" is "de" (menu: abc fr de ja)
        let e = s.press_menu(Step::Next, true);
        assert_eq!(
            e,
            SwitchEvent::ShowHud {
                order: vec!["fr".into(), "de".into(), "abc".into(), "ja".into()],
                selection: 1
            }
        );
        assert_eq!(
            s.modifiers_released(),
            SwitchEvent::Commit { id: "de".into() }
        );
        // the menu order after "de" is "ja"
        assert_eq!(
            s.press_menu(Step::Next, false),
            SwitchEvent::Commit { id: "ja".into() }
        );
        assert_eq!(
            s.press_menu(Step::Previous, false),
            SwitchEvent::Commit { id: "de".into() }
        );
        let mut one = Switcher::new();
        one.set_sources(vec!["abc".into()]);
        assert_eq!(one.press_menu(Step::Next, true), SwitchEvent::None);
    }

    #[test]
    fn hud_metrics() {
        assert_eq!(hud_content_size(2, 60.0, 30.0), (167.0, 100.0));
        assert_eq!(hud_content_size(3, 200.0, 30.0).0, 250.0);
        assert!((fade_alpha(0.15) - 0.5).abs() < 1e-9);
        assert_eq!(fade_alpha(1.0), 0.0);
    }
}
