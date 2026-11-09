//! A date picker: the date in a field's recessed well, its year, month and day each a part the arrows step, and a
//! stepper beside the well that steps the part in use.

use crate::look::{self, paint_focus_ring};
use crate::{CalendarDate, DatePart, DatePicker};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, Change, ControlState, Environment, Event, EventCx,
    MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_platform::{Keysym, MouseButton, PointerEvent};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{Color, ColorRole, ControlFamily};

const LEFT: Keysym = Keysym(0xff51);
const UP: Keysym = Keysym(0xff52);
const RIGHT: Keysym = Keysym(0xff53);
const DOWN: Keysym = Keysym(0xff54);

/// The stepper's width and the gap between it and the well. Approximate design values.
const STEPPER: f64 = 16.0;
const STEPPER_GAP: f64 = 4.0;
/// A stepper chevron's width and stroke, and the inset of the line between its halves.
const CHEVRON: f64 = 7.0;
const CHEVRON_STROKE: f64 = 1.25;
const DIVIDER_INSET: f64 = 3.0;
/// The part in use, while the picker has the focus: the text selection's colour behind it, this much wider each side.
const PART_PAD: f64 = 1.0;

impl CalendarDate {
    /// Days in `month` of `year`, by the proleptic Gregorian calendar.
    pub fn days_in_month(year: i32, month: u8) -> u8 {
        match month {
            2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        }
    }

    /// This date with `part` moved by `by`, wrapping within its range as a date picker's arrows do: the day within
    /// the month, the month within the year; the year moves freely. The day then fits the month it lands in.
    pub fn stepped(self, part: DatePart, by: i32) -> CalendarDate {
        let wrap = |value: i32, count: i32| (value - 1 + by).rem_euclid(count) + 1;
        let mut date = self;
        match part {
            DatePart::Year => date.year = self.year.saturating_add(by),
            DatePart::Month => date.month = wrap(i32::from(self.month), 12) as u8,
            DatePart::Day => {
                let days = i32::from(Self::days_in_month(self.year, self.month));
                date.day = wrap(i32::from(self.day), days) as u8;
            }
        }
        date.day = date.day.min(Self::days_in_month(date.year, date.month));
        date
    }

    /// The date as `YYYY-MM-DD`.
    pub fn text(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

impl DatePart {
    /// The part's characters in [`CalendarDate::text`].
    fn span(self) -> std::ops::Range<usize> {
        match self {
            DatePart::Year => 0..4,
            DatePart::Month => 5..7,
            DatePart::Day => 8..10,
        }
    }

    const ALL: [DatePart; 3] = [DatePart::Year, DatePart::Month, DatePart::Day];

    fn index(self) -> usize {
        Self::ALL.iter().position(|&p| p == self).unwrap_or(0)
    }
}

/// The node state of a [`DatePicker`]: the part the arrows and the stepper move, and the stepper's half held down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DatePickerState {
    part: DatePart,
    pressed: Option<bool>,
}

impl Default for DatePickerState {
    fn default() -> Self {
        DatePickerState {
            part: DatePart::Day,
            pressed: None,
        }
    }
}

/// The well and the stepper in a node of `size`, the stepper at the reading direction's end.
fn parts(size: Size, env: &Environment) -> (Rect, Rect) {
    let height = look::geometry(&env.theme, ControlFamily::DatePicker).height;
    let top = (size.height - height) / 2.0;
    let well = size.width - STEPPER - STEPPER_GAP;
    let rtl = env.direction == cupertino_ui_layout::LayoutDirection::Rtl;
    let (well_x, stepper_x) = if rtl {
        (STEPPER + STEPPER_GAP, 0.0)
    } else {
        (0.0, well + STEPPER_GAP)
    };
    (
        Rect::new(well_x, top, well, height),
        Rect::new(stepper_x, top, STEPPER, height),
    )
}

impl<M: Clone + 'static> DatePicker<M> {
    fn set(&self, date: CalendarDate, cx: &mut EventCx<'_, M>) {
        if date != self.date {
            if let Some(on_change) = &self.on_change {
                cx.emit(on_change(date));
            }
        }
    }

    /// Where each part's text starts and ends in the well.
    fn part_spans(&self, env: &Environment, well: Rect) -> Vec<(DatePart, f64, f64)> {
        let text = self.date.text();
        let inset = env.theme.metrics.field_inset;
        let width = |s: &str| look::label(env, s).map_or(0.0, |l| l.size().width);
        DatePart::ALL
            .iter()
            .map(|&part| {
                let span = part.span();
                let start = well.min_x() + inset + width(&text[..span.start]);
                (part, start, start + width(&text[span]))
            })
            .collect()
    }
}

impl<M: Clone + 'static> Widget<M> for DatePicker<M> {
    type State = DatePickerState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> DatePickerState {
        DatePickerState::default()
    }

    fn update(&self, old: &Self, _: &mut DatePickerState) -> Change {
        if self.date != old.date || self.enabled != old.enabled || self.label != old.label {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &DatePickerState, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let theme = &cx.env().theme;
        let geometry = look::geometry(theme, ControlFamily::DatePicker);
        let target = theme.metrics.min_target;
        Measured {
            size: Size::new(
                geometry.width.unwrap_or_default().max(target.width),
                geometry.height.max(target.height),
            ),
            baseline: None,
        }
    }

    fn event(
        &self,
        state: &mut DatePickerState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        match *event {
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) if self.enabled && cx.node() == cx.target() => {
                let Some(bounds) = cx.bounds() else {
                    return Propagation::Continue;
                };
                let env = cx.env().clone();
                let local = Point::new(position.x - bounds.min_x(), position.y - bounds.min_y());
                let (well, stepper) = parts(bounds.size, &env);
                cx.request_focus();
                if stepper.contains(local) {
                    let up = local.y < stepper.min_y() + stepper.height() / 2.0;
                    state.pressed = Some(up);
                    cx.capture_pointer();
                    self.set(self.date.stepped(state.part, if up { 1 } else { -1 }), cx);
                } else if let Some(&(part, _, _)) =
                    self.part_spans(&env, well).iter().min_by(|a, b| {
                        let distance =
                            |s: &(DatePart, f64, f64)| (local.x - (s.1 + s.2) / 2.0).abs();
                        distance(a).total_cmp(&distance(b))
                    })
                {
                    state.part = part;
                }
                cx.request_paint();
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Button { pressed: false, .. }, _)
                if state.pressed.is_some() =>
            {
                state.pressed = None;
                cx.release_pointer();
                cx.request_paint();
                Propagation::Stop
            }
            Event::Cancel if state.pressed.is_some() => {
                state.pressed = None;
                cx.request_paint();
                Propagation::Continue
            }
            Event::Key(key, _) if self.enabled && key.pressed && cx.node() == cx.target() => {
                if !key.mods.is_empty() {
                    return Propagation::Continue;
                }
                let index = state.part.index();
                let rtl = cx.env().direction == cupertino_ui_layout::LayoutDirection::Rtl;
                let (back, forth) = if rtl { (RIGHT, LEFT) } else { (LEFT, RIGHT) };
                match key.keysym {
                    UP => self.set(self.date.stepped(state.part, 1), cx),
                    DOWN => self.set(self.date.stepped(state.part, -1), cx),
                    k if k == back && index > 0 => state.part = DatePart::ALL[index - 1],
                    k if k == forth && index + 1 < DatePart::ALL.len() => {
                        state.part = DatePart::ALL[index + 1]
                    }
                    _ => return Propagation::Continue,
                }
                cx.request_paint();
                Propagation::Stop
            }
            Event::Action(ref request) if self.enabled => {
                match request.action {
                    Action::Increment => self.set(self.date.stepped(state.part, 1), cx),
                    Action::Decrement => self.set(self.date.stepped(state.part, -1), cx),
                    Action::Focus => cx.request_focus(),
                    _ => return Propagation::Continue,
                }
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, state: &DatePickerState, cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let theme = &env.theme;
        let base = ControlState {
            enabled: self.enabled,
            focused: cx.is_focused(),
            focus_visible: cx.focus_visible(),
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let (well, stepper) = parts(cx.size(), &env);
        let radius = theme.metrics.field_radius;
        let (fill, edge) = crate::field::well(theme, &base);
        look::fill(cx, well, radius, fill);
        look::stroke(
            cx,
            well.inset(Insets::uniform(0.5)),
            radius - 0.5,
            1.0,
            edge,
        );
        if base.focused {
            if let Some(&(_, from, to)) = self
                .part_spans(&env, well)
                .iter()
                .find(|(part, _, _)| *part == state.part)
            {
                let band = Rect::new(
                    from - PART_PAD,
                    well.min_y() + 3.0,
                    to - from + 2.0 * PART_PAD,
                    well.height() - 6.0,
                );
                let selection = look::color(theme, ColorRole::SelectedTextBackground, &base);
                look::fill(cx, band, 2.0, selection);
            }
        }
        let text = look::color(theme, ColorRole::Text, &base).map(|c| {
            if self.enabled {
                c
            } else {
                Color { a: c.a * 0.45, ..c }
            }
        });
        if let Some(label) = look::label(&env, &self.date.text()) {
            let x = well.min_x() + theme.metrics.field_inset;
            look::glyphs(
                cx,
                &label,
                Point::new(x, look::top(&label, well, &env)),
                text,
            );
        }
        // the stepper: an ordinary button's quiet bezel split by a faint line, a chevron in each half
        let stepper_radius = (STEPPER / 2.0).min(radius);
        look::fill(
            cx,
            stepper,
            stepper_radius,
            Some(look::quiet(theme, look::BEZEL, &base)),
        );
        let mid = stepper.min_y() + stepper.height() / 2.0;
        if let Some(pressed) = state.pressed {
            let half = if pressed {
                Rect::new(
                    stepper.min_x(),
                    stepper.min_y(),
                    stepper.width(),
                    stepper.height() / 2.0,
                )
            } else {
                Rect::new(
                    stepper.min_x(),
                    mid,
                    stepper.width(),
                    stepper.height() / 2.0,
                )
            };
            cx.push(PaintOp::PushClip(Shape::RoundedRect {
                rect: stepper,
                radius: stepper_radius,
            }));
            let step = look::quiet(
                theme,
                look::BEZEL,
                &ControlState {
                    pressed: true,
                    ..base
                },
            );
            let rest = look::quiet(theme, look::BEZEL, &base);
            look::fill(
                cx,
                half,
                0.0,
                Some(Color {
                    a: step.a - rest.a,
                    ..step
                }),
            );
            cx.push(PaintOp::PopClip);
        }
        look::fill(
            cx,
            Rect::new(
                stepper.min_x() + DIVIDER_INSET,
                mid - 0.5,
                stepper.width() - 2.0 * DIVIDER_INSET,
                1.0,
            ),
            0.0,
            look::color(theme, ColorRole::Separator, &base),
        );
        if let Some(color) = look::color(theme, ColorRole::ControlText, &base) {
            let centre_x = stepper.min_x() + stepper.width() / 2.0;
            for (up, y) in [
                (true, mid - stepper.height() / 4.0),
                (false, mid + stepper.height() / 4.0),
            ] {
                cx.push(PaintOp::Stroke {
                    shape: look::chevron(Point::new(centre_x, y), CHEVRON, up),
                    width: CHEVRON_STROKE,
                    paint: Paint::Solid(color),
                });
            }
        }
        paint_focus_ring(cx, well, radius);
    }

    fn semantics(&self, _: &DatePickerState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::SpinButton);
        cx.name(self.label.clone());
        cx.value(AccessibleValue::Text {
            value: self.date.text(),
            editing: None,
        });
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
            cx.action(Action::Focus);
            cx.action(Action::Increment);
            cx.action(Action::Decrement);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u8, day: u8) -> CalendarDate {
        CalendarDate { year, month, day }
    }

    #[test]
    fn each_part_steps_within_its_range_and_the_day_fits_the_month() {
        let d = date(2026, 1, 31);
        assert_eq!(
            d.stepped(DatePart::Day, 1),
            date(2026, 1, 1),
            "the day wraps in its month"
        );
        assert_eq!(
            d.stepped(DatePart::Month, 1),
            date(2026, 2, 28),
            "31 January to February"
        );
        assert_eq!(
            date(2024, 1, 31).stepped(DatePart::Month, 1),
            date(2024, 2, 29)
        );
        assert_eq!(
            date(2026, 12, 5).stepped(DatePart::Month, 1),
            date(2026, 1, 5)
        );
        assert_eq!(
            date(2026, 1, 5).stepped(DatePart::Month, -1),
            date(2026, 12, 5)
        );
        assert_eq!(
            date(2024, 2, 29).stepped(DatePart::Year, 1),
            date(2025, 2, 28)
        );
        assert_eq!(
            date(2026, 3, 1).stepped(DatePart::Day, -1),
            date(2026, 3, 31)
        );
        assert_eq!(date(2026, 7, 11).text(), "2026-07-11");
    }
}
