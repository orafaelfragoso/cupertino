//! The controls of the panes (§6.2): what each holds, its frame inside a form row and how a pointer press, drag or
//! release changes it. Drawing is in `paint`. Metrics are the regular-size alignment heights of the design system
//! (design-system §3; field meaning); widths not in the record are approximate.

use crate::geom::{Point, Rect};

/// Control metrics.
pub mod metrics {
    /// Switch 38 × 22 (§6.2).
    pub const SWITCH: (f64, f64) = (38.0, 22.0);
    /// Pop-up button height and width (approximate).
    pub const POPUP: (f64, f64) = (170.0, 20.0);
    /// Segmented control height and one segment's width (approximate).
    pub const SEGMENTED_HEIGHT: f64 = 20.0;
    pub const SEGMENT_WIDTH: f64 = 72.0;
    /// Slider: track 4, knob 20 (§6.2); the slider's width (approximate) and the end labels under the track,
    /// 11 pt (approximate).
    pub const SLIDER_TRACK: f64 = 4.0;
    pub const SLIDER_KNOB: f64 = 20.0;
    pub const SLIDER_WIDTH: f64 = 220.0;
    pub const SLIDER_LABEL_HEIGHT: f64 = 13.0;
    pub const SLIDER_LABEL_SIZE: f64 = 11.0;
    /// Tick dots at the stops: 4 × 4 (for the control panel's stepped slider, reuse approximate).
    pub const TICK: f64 = 4.0;
    /// A segment's and a pop-up button's corner radius. approximate.
    pub const RADIUS: f64 = 5.0;
    /// The level slider of Sound and Displays: the control panel's, a 22 pt track whose knob is 22 effective, the hit
    /// frame 9 beyond the track; its width approximate.
    pub const LEVEL_TRACK: f64 = 22.0;
    pub const LEVEL_WIDTH: f64 = 220.0;
    pub const LEVEL_HIT_OUTSET: f64 = 9.0;
    /// The level slider writes at most this often while dragging (16.7 ms).
    pub const LEVEL_WRITE_INTERVAL: f64 = 1.0 / 60.0;
    /// A push button: 20 high (§6.2), its width approximate.
    pub const BUTTON: (f64, f64) = (96.0, 20.0);
    /// Between buttons side by side. approximate.
    pub const BUTTON_GAP: f64 = 8.0;
    /// A read-only value's width. approximate.
    pub const TEXT: (f64, f64) = (200.0, 16.0);
}

use metrics as m;

/// What a control holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Switch {
        on: bool,
    },
    Segmented {
        segments: Vec<&'static str>,
        selected: usize,
    },
    Popup {
        choices: Vec<String>,
        selected: usize,
    },
    /// A slider over 0…1, committed on release (§7.2); with `stops` it rests only on `stops` evenly spaced positions
    /// (0 … stops − 1), each with a tick; `marks` are further ticks of a slider without stops (the Dock sliders'
    /// marks); `labels` name its two ends.
    Slider {
        value: f64,
        stops: Option<usize>,
        marks: &'static [f64],
        labels: (&'static str, &'static str),
    },
    /// The control panel's level slider (volume, brightness): written while dragging, at most every 16.7 ms.
    Level {
        value: f64,
    },
    /// A push button.
    Button {
        title: String,
    },
    /// Push buttons side by side; a press commits the index of the one hit.
    Buttons {
        titles: Vec<String>,
    },
    /// A value that is only shown.
    Text {
        value: String,
    },
}

/// A value a control commits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Index(usize),
    Fraction(f64),
}

impl Control {
    /// The control's size in a row.
    pub fn size(&self) -> (f64, f64) {
        match self {
            Control::Switch { .. } => m::SWITCH,
            Control::Segmented { segments, .. } => (
                m::SEGMENT_WIDTH * segments.len() as f64,
                m::SEGMENTED_HEIGHT,
            ),
            Control::Popup { .. } => m::POPUP,
            Control::Slider { .. } => (m::SLIDER_WIDTH, m::SLIDER_KNOB + m::SLIDER_LABEL_HEIGHT),
            Control::Level { .. } => (m::LEVEL_WIDTH, m::LEVEL_TRACK),
            Control::Button { .. } => m::BUTTON,
            Control::Buttons { titles } => (
                titles.len() as f64 * (m::BUTTON.0 + m::BUTTON_GAP) - m::BUTTON_GAP,
                m::BUTTON.1,
            ),
            Control::Text { .. } => m::TEXT,
        }
    }

    /// The slider's position of `stop` among `stops`.
    pub fn stop_value(stop: usize, stops: usize) -> f64 {
        if stops <= 1 {
            0.0
        } else {
            stop as f64 / (stops - 1) as f64
        }
    }

    /// The stop nearest `value`.
    pub fn nearest_stop(value: f64, stops: usize) -> usize {
        ((value.clamp(0.0, 1.0) * (stops - 1) as f64).round() as usize).min(stops - 1)
    }
}

/// The span of the knob's centre along a slider in `frame`: from half a knob in from the left to half a knob in from
/// the right.
pub fn slider_span(frame: &Rect) -> (f64, f64) {
    (
        frame.x + m::SLIDER_KNOB / 2.0,
        frame.right() - m::SLIDER_KNOB / 2.0,
    )
}

/// The slider value under pointer x.
pub fn slider_value(frame: &Rect, x: f64) -> f64 {
    let (a, b) = slider_span(frame);
    ((x - a) / (b - a)).clamp(0.0, 1.0)
}

/// The knob's rectangle at `value`.
pub fn slider_knob(frame: &Rect, value: f64) -> Rect {
    let (a, b) = slider_span(frame);
    let cx = a + (b - a) * value.clamp(0.0, 1.0);
    Rect::new(
        cx - m::SLIDER_KNOB / 2.0,
        frame.y,
        m::SLIDER_KNOB,
        m::SLIDER_KNOB,
    )
}

/// The level under pointer x on a level slider in `frame`: `clamp((x − minX − 11) / (w − 22), 0, 1).
pub fn level_value(frame: &Rect, x: f64) -> f64 {
    ((x - frame.x - m::LEVEL_TRACK / 2.0) / (frame.w - m::LEVEL_TRACK)).clamp(0.0, 1.0)
}

/// The level slider's hit frame: 9 beyond the track.
pub fn level_hit(frame: &Rect) -> Rect {
    frame.inset(-m::LEVEL_HIT_OUTSET, -m::LEVEL_HIT_OUTSET)
}

/// A level slider's writes during a drag: the press writes at once (zero-distance drag), then at most one write every
/// [`metrics::LEVEL_WRITE_INTERVAL`] with the latest value, and the release writes the last one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LevelWriter {
    last_write: Option<f64>,
    /// A value that arrived inside the interval and waits for the next write.
    pending: Option<f64>,
}

impl LevelWriter {
    /// The pointer gave `value` at time `now`: the value to write now, if any.
    pub fn moved(&mut self, value: f64, now: f64) -> Option<f64> {
        match self.last_write {
            Some(t) if now - t < m::LEVEL_WRITE_INTERVAL => {
                self.pending = Some(value);
                None
            }
            _ => {
                self.last_write = Some(now);
                self.pending = None;
                Some(value)
            }
        }
    }

    /// When the waiting value is due, if one waits.
    pub fn due(&self) -> Option<f64> {
        self.pending
            .and(self.last_write.map(|t| t + m::LEVEL_WRITE_INTERVAL))
    }

    /// The timer of [`LevelWriter::due`] fired at `now`: the waiting value to write.
    pub fn tick(&mut self, now: f64) -> Option<f64> {
        let due = self.due()?;
        if now < due {
            return None;
        }
        self.last_write = Some(now);
        self.pending.take()
    }

    /// The release at `value`: always written, the last write of the drag.
    pub fn released(&mut self, value: f64) -> f64 {
        *self = LevelWriter::default();
        value
    }
}

/// A level of 0.001 or less mutes (below the step the slider is silent).
pub const MUTE_BELOW: f64 = 0.001;

/// A pointer interaction with one control.
#[derive(Clone, Debug, PartialEq)]
pub enum Press {
    /// A switch or segmented control: the value commits on the press (§7.2).
    Commit(Value),
    /// A pop-up button: its menu opens.
    OpenMenu,
    /// A slider: the knob jumps to the pointer and follows it; the value is shown and committed on release.
    Drag(f64),
    /// A level slider: the level under the pointer, written at once and while dragging.
    Level(f64),
}

/// What a press at `p` on `control` in `frame` does.
pub fn press(control: &Control, frame: &Rect, p: Point) -> Option<Press> {
    if let Control::Level { .. } = control {
        return level_hit(frame)
            .contains(p)
            .then(|| Press::Level(level_value(frame, p.x)));
    }
    if !frame.contains(p) {
        return None;
    }
    Some(match control {
        Control::Switch { on } => Press::Commit(Value::Bool(!on)),
        Control::Segmented { segments, .. } => Press::Commit(Value::Index(
            (((p.x - frame.x) / m::SEGMENT_WIDTH) as usize).min(segments.len() - 1),
        )),
        Control::Popup { .. } => Press::OpenMenu,
        Control::Slider { stops, .. } => Press::Drag(drag(*stops, frame, p.x)),
        Control::Button { .. } => Press::Commit(Value::Index(0)),
        Control::Buttons { titles } => {
            let i = ((p.x - frame.x) / (m::BUTTON.0 + m::BUTTON_GAP)) as usize;
            // the gap between two buttons hits neither
            if i >= titles.len()
                || p.x - frame.x - i as f64 * (m::BUTTON.0 + m::BUTTON_GAP) > m::BUTTON.0
            {
                return None;
            }
            Press::Commit(Value::Index(i))
        }
        Control::Level { .. } => unreachable!("handled above"),
        Control::Text { .. } => return None,
    })
}

/// The slider value shown while dragging at x: snapped to the stops when there are any.
pub fn drag(stops: Option<usize>, frame: &Rect, x: f64) -> f64 {
    let v = slider_value(frame, x);
    match stops {
        Some(n) => Control::stop_value(Control::nearest_stop(v, n), n),
        None => v,
    }
}

/// What the release of a slider drag at `shown` commits: the stop index for a slider with stops, else the fraction.
pub fn release(stops: Option<usize>, shown: f64) -> Value {
    match stops {
        Some(n) => Value::Index(Control::nearest_stop(shown, n)),
        None => Value::Fraction(shown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Rect = Rect::new(100.0, 10.0, 220.0, 33.0);

    #[test]
    fn switches_and_segments_commit_on_the_press() {
        assert_eq!(
            press(
                &Control::Switch { on: true },
                &Rect::new(0.0, 0.0, 38.0, 22.0),
                Point { x: 5.0, y: 5.0 }
            ),
            Some(Press::Commit(Value::Bool(false)))
        );
        let seg = Control::Segmented {
            segments: vec!["Bottom", "Left", "Right"],
            selected: 0,
        };
        let frame = Rect::new(0.0, 0.0, 216.0, 20.0);
        assert_eq!(
            press(&seg, &frame, Point { x: 150.0, y: 10.0 }),
            Some(Press::Commit(Value::Index(2)))
        );
        assert_eq!(
            press(&seg, &frame, Point { x: 300.0, y: 10.0 }),
            None,
            "outside the control"
        );
    }

    #[test]
    fn a_slider_with_stops_snaps_while_dragging_and_commits_the_stop_on_release() {
        let control = Control::Slider {
            value: 0.0,
            stops: Some(10),
            marks: &[],
            labels: ("Slow", "Fast"),
        };
        let (a, b) = slider_span(&FRAME);
        assert_eq!((a, b), (110.0, 310.0));
        let Some(Press::Drag(shown)) = press(
            &control,
            &FRAME,
            Point {
                x: a + (b - a) * 0.36,
                y: 20.0,
            },
        ) else {
            panic!("a slider drags")
        };
        assert_eq!(shown, 3.0 / 9.0, "0.36 snaps to stop 3 of 0…9");
        assert_eq!(release(Some(10), shown), Value::Index(3));
        assert_eq!(drag(Some(10), &FRAME, 0.0), 0.0, "clamped at the left end");
        assert_eq!(drag(None, &FRAME, 210.0), 0.5);
        assert_eq!(slider_knob(&FRAME, 1.0), Rect::new(300.0, 10.0, 20.0, 20.0));
    }

    #[test]
    fn the_level_slider_maps_x_like_the_control_panel_and_hits_nine_beyond() {
        let frame = Rect::new(100.0, 10.0, 222.0, 22.0);
        assert_eq!(level_value(&frame, 111.0), 0.0, "minX + 11 is 0");
        assert_eq!(level_value(&frame, 311.0), 1.0, "maxX − 11 is 1");
        assert_eq!(level_value(&frame, 211.0), 0.5);
        assert_eq!(level_value(&frame, 50.0), 0.0, "clamped");
        let control = Control::Level { value: 0.2 };
        assert_eq!(
            press(&control, &frame, Point { x: 95.0, y: 5.0 }),
            Some(Press::Level(0.0)),
            "the hit frame is 9 beyond the track"
        );
        assert_eq!(press(&control, &frame, Point { x: 90.0, y: 5.0 }), None);
    }

    #[test]
    fn a_level_drag_writes_at_most_every_16_7_ms_with_the_latest_value_and_the_release_last() {
        let mut w = LevelWriter::default();
        assert_eq!(w.moved(0.10, 0.000), Some(0.10), "the press writes at once");
        assert_eq!(w.moved(0.20, 0.005), None);
        assert_eq!(
            w.moved(0.30, 0.010),
            None,
            "inside the interval only the latest waits"
        );
        assert_eq!(w.due(), Some(1.0 / 60.0));
        assert_eq!(w.tick(0.012), None, "not due yet");
        assert_eq!(w.tick(0.017), Some(0.30), "the latest value, once due");
        assert_eq!(w.due(), None);
        assert_eq!(
            w.moved(0.40, 0.040),
            Some(0.40),
            "past the interval it writes at once"
        );
        assert_eq!(w.moved(0.45, 0.041), None);
        assert_eq!(w.released(0.50), 0.50, "the release writes the final value");
        assert_eq!(w.due(), None, "and nothing waits after it");
    }
}
