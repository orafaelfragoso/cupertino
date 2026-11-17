//! The progress spinner shown while an attempt takes long: 12 spokes, 24 frames of 1/24 s, a head
//! spoke that advances every second frame and a tail of spokes that fades behind it. Drawn from numbers, no artwork.
//! Pure: a frame is a number and the spoke opacities follow from it.

/// Spokes around the circle.
pub const SPOKES: usize = 12;
/// Frames in one turn (`SpinnerRegular0..23`).
pub const FRAMES: u32 = 24;
/// One turn takes this long: 1/24 s per frame.
pub const PERIOD: f64 = 1.0;
/// The spinner shows once authentication has taken this long (the design login process's ).
pub const SHOW_AFTER: f64 = 1.0;

/// Edge of the artwork canvas the measurements below are in: 64 px at 2x, 32 pt.
pub const CANVAS: f64 = 64.0;
/// A spoke runs from this radius to [`OUTER_RADIUS`] and is [`SPOKE_WIDTH`] wide, in canvas units.
pub const INNER_RADIUS: f64 = 11.77;
pub const OUTER_RADIUS: f64 = 26.57;
pub const SPOKE_WIDTH: f64 = 5.26;

/// Spoke opacities from the oldest to the head on even frames.
const RAMP_EVEN: [f32; SPOKES] = [
    0.161, 0.212, 0.259, 0.322, 0.420, 0.502, 0.600, 0.702, 0.800, 0.878, 0.949, 1.000,
];
/// The same on odd frames.
const RAMP_ODD: [f32; SPOKES] = [
    0.200, 0.251, 0.302, 0.349, 0.451, 0.529, 0.620, 0.722, 0.800, 0.878, 0.949, 1.000,
];

/// The frame `elapsed` seconds after the spinner appeared: `round(frac(t) * 24) mod 24`.
pub fn frame_at(elapsed: f64) -> u32 {
    ((elapsed / PERIOD).rem_euclid(1.0) * f64::from(FRAMES)).round() as u32 % FRAMES
}

/// Seconds from `elapsed` until the frame changes.
pub fn next_frame_in(elapsed: f64) -> f64 {
    let frame_length = PERIOD / f64::from(FRAMES);
    // frames change halfway between multiples of the frame length, where `round` turns over
    let turns_at = ((elapsed / frame_length).round() + 0.5) * frame_length;
    turns_at - elapsed
}

/// The head spoke of `frame`, counted clockwise from 12 o'clock: `floor((f - 1) / 2) mod 12`.
pub fn head(frame: u32) -> usize {
    (i64::from(frame) - 1)
        .div_euclid(2)
        .rem_euclid(SPOKES as i64) as usize
}

/// Opacity of `spoke` on `frame`: the spoke `k` steps behind the head (counter-clockwise) has `ramp[11 - k], the ramp
/// being the even or the odd one by the frame's parity.
pub fn opacity(frame: u32, spoke: usize) -> f32 {
    let behind = (head(frame) + SPOKES - spoke % SPOKES) % SPOKES;
    let ramp = if frame.is_multiple_of(2) {
        &RAMP_EVEN
    } else {
        &RAMP_ODD
    };
    ramp[SPOKES - 1 - behind]
}
