//! A theme's colours before state resolution: one colour per role, plus the state colours no role holds, expanded into
//! every role and state so [`ColorTokens::get`]'s normal fallback never erases a selected or disabled state.

use crate::{Color, ColorRole, ColorTokens, InteractionVariant, StateVariant};

/// The colours of state treatments that no role holds in its normal state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StateColors<C> {
    /// Control and text-field border on hover and press.
    pub emphasized_border: C,
    pub pressed_default_fill: C,
    pub pressed_destructive_fill: C,
    /// Track of a switch that is on.
    pub selected_switch_track: C,
    pub pressed_switch_track: C,
    pub pressed_selected_switch_track: C,
    pub disabled_switch_track: C,
    pub disabled_selected_switch_track: C,
    pub disabled_switch_thumb: C,
}

/// One appearance's colours, `normal` in [`ColorRole::ALL`] order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Palette<C> {
    pub normal: [(ColorRole, C); ColorRole::ALL.len()],
    pub states: StateColors<C>,
}

impl<C: Copy> Palette<C> {
    pub fn try_map<D, E>(&self, mut f: impl FnMut(C) -> Result<D, E>) -> Result<Palette<D>, E> {
        let mut normal = Vec::with_capacity(self.normal.len());
        for (role, c) in self.normal {
            normal.push((role, f(c)?));
        }
        let s = &self.states;
        Ok(Palette {
            normal: normal
                .try_into()
                .unwrap_or_else(|_| unreachable!("one colour per role")),
            states: StateColors {
                emphasized_border: f(s.emphasized_border)?,
                pressed_default_fill: f(s.pressed_default_fill)?,
                pressed_destructive_fill: f(s.pressed_destructive_fill)?,
                selected_switch_track: f(s.selected_switch_track)?,
                pressed_switch_track: f(s.pressed_switch_track)?,
                pressed_selected_switch_track: f(s.pressed_selected_switch_track)?,
                disabled_switch_track: f(s.disabled_switch_track)?,
                disabled_selected_switch_track: f(s.disabled_selected_switch_track)?,
                disabled_switch_thumb: f(s.disabled_switch_thumb)?,
            },
        })
    }
}

impl Palette<[u8; 4]> {
    pub fn colors(&self) -> Palette<Color> {
        self.try_map(|c| Ok::<_, std::convert::Infallible>(Color::from_rgba8(c)))
            .unwrap_or_else(|never| match never {})
    }
}

impl Palette<Color> {
    pub fn resolve(&self) -> ColorTokens {
        ColorTokens::new(StateVariant::ALL.into_iter().flat_map(|state| {
            ColorRole::ALL.map(|role| ((role, state), self.resolved(role, state)))
        }))
    }

    fn normal(&self, role: ColorRole) -> Color {
        self.normal[role as usize].1
    }

    /// Selection applies first, then interaction; disabled overrides both.
    fn resolved(&self, role: ColorRole, state: StateVariant) -> Color {
        use ColorRole::*;
        use InteractionVariant::*;
        let s = &self.states;
        let selected = state.selected;
        match (state.interaction, role) {
            (Disabled, ControlFill | DefaultButtonFill | DestructiveFill | TextBackground) => {
                self.normal(DisabledControlFill)
            }
            (Disabled, ControlBorder | TextFieldBorder) => self.normal(DisabledControlBorder),
            (
                Disabled,
                ControlText | DefaultButtonText | DestructiveText | Text | PlaceholderText,
            ) => self.normal(DisabledControlText),
            (Disabled, SwitchTrack) if selected => s.disabled_selected_switch_track,
            (Disabled, SwitchTrack) => s.disabled_switch_track,
            (Disabled, SwitchThumb) => s.disabled_switch_thumb,
            (Hover | Pressed, ControlBorder | TextFieldBorder) => s.emphasized_border,
            (Pressed, DefaultButtonFill) => s.pressed_default_fill,
            (Pressed, DestructiveFill) => s.pressed_destructive_fill,
            (Pressed, SwitchTrack) if selected => s.pressed_selected_switch_track,
            (Pressed, SwitchTrack) => s.pressed_switch_track,
            (Pressed, ControlFill) if !selected => self.normal(SecondarySelectedControlFill),
            (_, ControlFill) if selected && state.window_active => self.normal(SelectedControlFill),
            (_, ControlFill) if selected => self.normal(SecondarySelectedControlFill),
            (_, ControlText) if selected && state.window_active => self.normal(SelectedControlText),
            (_, ControlText) if selected => self.normal(SecondarySelectedControlText),
            (_, SwitchTrack) if selected => s.selected_switch_track,
            _ => self.normal(role),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::neutral;

    fn state(interaction: InteractionVariant, selected: bool, window_active: bool) -> StateVariant {
        StateVariant {
            interaction,
            selected,
            window_active,
        }
    }

    #[test]
    fn every_table_lists_each_role_once_in_order() {
        for palette in [
            neutral::LIGHT,
            neutral::DARK,
            neutral::HIGH_CONTRAST_LIGHT,
            neutral::HIGH_CONTRAST_DARK,
        ] {
            let roles = palette.normal.map(|(role, _)| role);
            assert_eq!(roles, ColorRole::ALL);
        }
        for (i, role) in ColorRole::ALL.into_iter().enumerate() {
            assert_eq!(role as usize, i);
        }
    }

    #[test]
    fn every_role_has_an_explicit_colour_in_every_state() {
        let colors = neutral::LIGHT.colors().resolve();
        assert_eq!(
            colors.map.len(),
            ColorRole::ALL.len() * StateVariant::ALL.len()
        );
    }

    #[test]
    fn disabled_wins_over_selection_and_press_wins_over_hover() {
        use ColorRole::*;
        use InteractionVariant::*;
        let palette = neutral::LIGHT.colors();
        let colors = palette.resolve();
        let get = |role, state| colors.get(role, state).unwrap();
        for selected in [false, true] {
            assert_eq!(
                get(ControlFill, state(Disabled, selected, true)),
                palette.normal(DisabledControlFill)
            );
            assert_eq!(
                get(Text, state(Disabled, selected, true)),
                palette.normal(DisabledControlText)
            );
        }
        assert_eq!(
            get(SwitchTrack, state(Disabled, true, true)),
            palette.states.disabled_selected_switch_track
        );
        assert_eq!(
            get(DefaultButtonFill, state(Pressed, false, true)),
            palette.states.pressed_default_fill
        );
        assert_eq!(
            get(DefaultButtonFill, state(Hover, false, true)),
            palette.normal(DefaultButtonFill)
        );
        assert_eq!(
            get(ControlBorder, state(Hover, false, true)),
            palette.states.emphasized_border
        );
    }

    #[test]
    fn selection_follows_window_activity_and_survives_press() {
        use ColorRole::*;
        use InteractionVariant::*;
        let palette = neutral::DARK.colors();
        let colors = palette.resolve();
        let get = |role, state| colors.get(role, state).unwrap();
        assert_eq!(
            get(ControlFill, state(Normal, true, true)),
            palette.normal(SelectedControlFill)
        );
        assert_eq!(
            get(ControlText, state(Normal, true, false)),
            palette.normal(SecondarySelectedControlText)
        );
        assert_eq!(
            get(ControlFill, state(Pressed, true, true)),
            palette.normal(SelectedControlFill)
        );
        assert_eq!(
            get(ControlFill, state(Pressed, false, true)),
            palette.normal(SecondarySelectedControlFill)
        );
        assert_eq!(
            get(SwitchTrack, state(Normal, true, false)),
            palette.states.selected_switch_track
        );
        assert_eq!(
            get(SwitchTrack, state(Normal, false, false)),
            palette.normal(SwitchTrack)
        );
    }
}
