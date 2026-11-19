//! Compositor wiring for [`cupertino_server_render::cursor`].

use std::time::Duration;

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::utils::{Logical, Point};

use crate::state::Cupertino;

pub use cupertino_server_render::cursor::*;

impl Cupertino {
    /// Feed a pointer motion to `pos` to the shake detector. A recognised shake grows the cursor
    /// and starts the evaluations that shrink it again.
    pub fn shake_motion(&mut self, pos: Point<f64, Logical>) {
        if !self.config.pointer.shake_to_locate {
            return;
        }
        let now = self.now();
        if !self
            .cursor_shake
            .detector
            .motion(cupertino_input::Point::new(pos.x, pos.y), now)
        {
            return;
        }
        self.request_redraw();
        if self.cursor_shake.timer.is_some() {
            return;
        }
        let interval = Duration::from_secs_f64(cupertino_input::SHAKE_EVALUATION_INTERVAL_S);
        let timer =
            self.loop_handle
                .insert_source(Timer::from_duration(interval), move |_, _, st| {
                    if st.evaluate_cursor_shake() {
                        TimeoutAction::ToDuration(interval)
                    } else {
                        st.cursor_shake.timer = None;
                        TimeoutAction::Drop
                    }
                });
        match timer {
            Ok(token) => self.cursor_shake.timer = Some(token),
            Err(e) => {
                // without the evaluations the cursor would stay grown: give the shake up
                tracing::error!(error = %e, "shake-to-locate timer not inserted; the cursor keeps its size");
                self.cursor_shake.detector.release();
            }
        }
    }

    /// One evaluation of the shake multiplier at the current time; true while the cursor is
    /// still grown.
    pub fn evaluate_cursor_shake(&mut self) -> bool {
        let before = self.cursor_shake.detector.multiplier();
        let now = self.now();
        let after = self.cursor_shake.detector.evaluate(now);
        if after != before {
            self.request_redraw();
        }
        after > 1.0
    }

    /// The shake multiplier, 1 while the cursor has its normal size.
    pub fn shake_multiplier(&self) -> f64 {
        self.cursor_shake.detector.multiplier()
    }

    /// Scale of the cursors the server draws: the user cursor scale times the shake multiplier
    /// (product), kept below the display server's limit.
    pub fn cursor_scale(&self) -> f64 {
        (self.config.pointer.cursor_scale * self.shake_multiplier())
            .min(cupertino_input::CURSOR_SCALE_LIMIT)
    }
}
