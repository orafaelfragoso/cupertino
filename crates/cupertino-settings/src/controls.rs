//! The controls of the panes (§6.2): what each holds and the values it commits; the Kit shell draws them
//! (`shell::form`). Widths not in the design record are approximate.

/// Control metrics.
pub mod metrics {
    /// The slider's width (approximate).
    pub const SLIDER_WIDTH: f64 = 220.0;
    /// The level slider of Sound and Displays; its width approximate.
    pub const LEVEL_WIDTH: f64 = 220.0;
    /// The level slider writes at most this often while dragging (16.7 ms).
    pub const LEVEL_WRITE_INTERVAL: f64 = 1.0 / 60.0;
    /// The title under a picture of a grid, and the title and the photographer's name. approximate.
    pub const TILE_TITLE_HEIGHT: f64 = 20.0;
    pub const TILE_CREDITED_HEIGHT: f64 = 34.0;
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
    /// A push button, then a switch: a press of the button commits `Value::Index(0)`, the switch its new state.
    ButtonAndSwitch {
        title: String,
        on: bool,
    },
    /// A value that is only shown.
    Text {
        value: String,
    },
    /// The picture the desktop shows: large, with its name and a note (a pair of light and dark pictures).
    Preview {
        image: Option<std::path::PathBuf>,
        title: String,
        note: Option<String>,
    },
    /// A grid of pictures; a press commits the index of the one hit.
    Tiles {
        tiles: Vec<Tile>,
        selected: Option<usize>,
    },
}

/// A picture of a [`Control::Tiles`] grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    pub title: String,
    /// The image file the thumbnail is made from.
    pub image: std::path::PathBuf,
    /// The tile is a light and a dark picture.
    pub pair: bool,
    /// Who made the picture, shown under its title.
    pub credit: Option<String>,
}

/// The height under a picture of `tiles` for its title (and the photographer, when one is named).
pub fn tile_caption_height(tiles: &[Tile]) -> f64 {
    if tiles.iter().any(|t| t.credit.is_some()) {
        m::TILE_CREDITED_HEIGHT
    } else {
        m::TILE_TITLE_HEIGHT
    }
}

/// A value a control commits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Index(usize),
    Fraction(f64),
}

impl Control {
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

#[cfg(test)]
mod tests {
    use super::*;

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
