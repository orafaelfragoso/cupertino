//! The slice's decisions without a display: which modifiers a buffer may use, what a frame draws, when a buffer may be
//! drawn into again, and how many refreshes a window missed.

use crate::gles::{MODIFIER_INVALID, MODIFIER_LINEAR};

/// The modifiers a buffer may use: those the compositor offers for the format that the driver can render to, in the
/// compositor's order. Linear is kept only when both sides list it; nothing is added, so a hardware driver never gets
/// linear unless it renders to it and the compositor asked for it. An implicit layout (`MODIFIER_INVALID`) is kept
/// when the compositor offers it, since no driver lists it.
pub fn usable_modifiers(offered: &[u64], renderable: &[u64], software: bool) -> Vec<u64> {
    offered
        .iter()
        .copied()
        .filter(|m| renderable.contains(m) || *m == MODIFIER_INVALID)
        .filter(|&m| software || m != MODIFIER_LINEAR || renderable.contains(&MODIFIER_LINEAR))
        .collect()
}

/// How many layers of drawing each frame of the overdraw test lays down before its last, flat one.
pub const OVERDRAW_LAYERS: usize = 24;

/// The two colours frames end in, alternately (premultiplied, opaque).
pub const FINAL_COLOURS: [[f32; 4]; 2] = [[0.20, 0.45, 0.85, 1.0], [0.85, 0.35, 0.20, 1.0]];

/// The colour of shared-memory frames, which the overdraw check also expects uniform.
pub const SHM_COLOUR: [f32; 4] = [0.25, 0.70, 0.35, 1.0];

/// What frame `n` of a `size` window draws: many overlapping rectangles in colours never shown at rest, then one
/// rectangle covering the window in one of [`FINAL_COLOURS`]. A capture of a finished frame is that colour alone;
/// one taken of a buffer still being drawn, or of a torn mix of two frames, is not.
pub fn frame_rects(n: u64, size: (i32, i32)) -> Vec<([i32; 4], [f32; 4])> {
    let (w, h) = size;
    let mut rects = Vec::with_capacity(OVERDRAW_LAYERS + 1);
    let mut seed = n.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for layer in 0..OVERDRAW_LAYERS {
        let x = (next() % w.max(1) as u64) as i32 / 2;
        let y = (next() % h.max(1) as u64) as i32 / 2;
        let rw = w - x - (next() % (w as u64 / 4 + 1)) as i32;
        let rh = h - y - (next() % (h as u64 / 4 + 1)) as i32;
        let shade = layer as f32 / OVERDRAW_LAYERS as f32;
        rects.push(([x, y, rw.max(1), rh.max(1)], [shade, 1.0 - shade, 0.5, 1.0]));
    }
    rects.push(([0, 0, w, h], final_colour(n)));
    rects
}

pub fn final_colour(n: u64) -> [f32; 4] {
    FINAL_COLOURS[(n % 2) as usize]
}

/// When a buffer may be drawn into again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reuse {
    /// Never attached, or ready again.
    Free,
    /// Waiting for its release point (explicit sync) to signal.
    ReleasePoint,
    /// Without explicit sync: waiting for the compositor's release.
    Release,
    /// Without explicit sync: released after commit number `.0`; free once a later commit has gone out.
    ReleasedAt(u64),
}

impl Reuse {
    /// Whether the buffer is free when `commits` frames have been committed in all.
    pub fn free(self, commits: u64) -> bool {
        match self {
            Reuse::Free => true,
            Reuse::ReleasePoint | Reuse::Release => false,
            Reuse::ReleasedAt(at) => commits > at,
        }
    }
}

/// Missed refreshes counted from presentation feedback: the refreshes between consecutive presented frames of a
/// window that draws one frame each refresh. A compositor that numbers its refreshes gives the count by its sequence
/// numbers; one that does not (sequence 0) by the time between presentations over the refresh period.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Gaps {
    last: Option<(u64, u64)>,
    pub presented: u64,
    pub discarded: u64,
    pub missed: u64,
    /// Each frame the compositor numbered that was not shown at the refresh after the last one's, as the numbers of
    /// the last one's refresh and of its own (at most [`Gaps::MAX_BREAKS`]): `b - a - 1` refreshes missed, or one
    /// more frame than refreshes when both are shown at one (-1). A compositor that numbers its refreshes by its
    /// clock as it draws, not by a display's vblank, shows a frame a little late as a refresh missed and the next as
    /// one shown twice; over a run of frames the breaks sum to the refreshes it really missed, and a reader can leave
    /// out those it caused itself.
    pub breaks: Vec<(u64, u64)>,
}

impl Gaps {
    /// The most breaks kept; later ones still count in `missed`.
    pub const MAX_BREAKS: usize = 4096;

    /// A frame shown at refresh `seq`, at `time_ns`, on an output refreshing every `refresh_ns` (0 when unknown).
    pub fn presented(&mut self, seq: u64, time_ns: u64, refresh_ns: u64) {
        self.presented += 1;
        if let Some((last_seq, last_time)) = self.last {
            let skipped = if seq != 0 && last_seq != 0 {
                seq.saturating_sub(last_seq).saturating_sub(1)
            } else {
                (time_ns.saturating_sub(last_time) + refresh_ns / 2)
                    .checked_div(refresh_ns)
                    .map_or(0, |periods| periods.saturating_sub(1))
            };
            self.missed += skipped;
            let numbered = seq != 0 && last_seq != 0;
            if numbered && seq != last_seq + 1 && self.breaks.len() < Self::MAX_BREAKS {
                self.breaks.push((last_seq, seq));
            }
        }
        self.last = Some((seq, time_ns));
    }

    /// A frame the compositor never showed; the next presented one counts its refreshes from the last shown.
    pub fn discarded(&mut self) {
        self.discarded += 1;
    }
}
