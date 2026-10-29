//! Presentation feedback (`wp_presentation`): when each presented frame reached the screen, on the compositor's
//! presentation clock, or that it never did.

use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::presentation_time::client::wp_presentation::{self, WpPresentation};
use wayland_protocols::wp::presentation_time::client::wp_presentation_feedback::{
    self, Kind, WpPresentationFeedback,
};

use crate::connection::State;
use crate::{Event, SurfaceId};

/// A presented frame reached the screen ([`Event::Presented`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Presented {
    pub surface: SurfaceId,
    /// When the frame turned visible, in nanoseconds on the clock of [`crate::Connection::presentation_clock`].
    pub time_ns: u64,
    /// The output's refresh interval in nanoseconds; 0 when it has none, such as a variable refresh rate.
    pub refresh_ns: u32,
    /// The output's vertical retrace counter at the frame; 0 when the output has none.
    pub sequence: u64,
    pub kind: Kind,
}

impl Dispatch<WpPresentation, ()> for State {
    fn event(
        state: &mut Self,
        _: &WpPresentation,
        event: wp_presentation::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.presentation_clock = Some(clk_id);
        }
    }
}

impl Dispatch<WpPresentationFeedback, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        id: &SurfaceId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if !state.has_surface(*id) {
            return;
        }
        match event {
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                seq_hi,
                seq_lo,
                flags,
            } => {
                let seconds = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                state.events.push(Event::Presented(Presented {
                    surface: *id,
                    time_ns: seconds
                        .saturating_mul(1_000_000_000)
                        .saturating_add(u64::from(tv_nsec)),
                    refresh_ns: refresh,
                    sequence: (u64::from(seq_hi) << 32) | u64::from(seq_lo),
                    kind: flags.into_result().unwrap_or(Kind::empty()),
                }));
            }
            wp_presentation_feedback::Event::Discarded => {
                state.events.push(Event::Discarded(*id));
            }
            _ => {}
        }
    }
}
