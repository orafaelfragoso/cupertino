//! How late after a vblank a display's next frame starts. A frame drawn right after the vblank sits finished for most
//! of the period before its flip, and every input that arrives meanwhile waits a whole period more. Where the display's
//! flips report real presentation timestamps (the driver's flip timestamp, not the server's own clock), the vblank's
//! phase is known, so the frame can start later and still make the next vblank: it starts the recent maximum render
//! time plus [`MARGIN`] before that vblank, and never later than half a period after the last one, because a missed
//! refresh costs more than the latency it saves.
//!
//! The render times are the last [`WINDOW`] frames' times from the start of drawing to the GPU finishing the frame,
//! the moment its render fence signalled ([`GpuDone::At`]), as the flip is queued before the GPU is done. Where the
//! fence cannot be read, a frame's render time is its time to the flip being queued plus a kept estimate of the GPU's
//! part, which a held frame that still missed its vblank raises for good ([`LateLatch::gpu_estimate`]). A frame that
//! missed its vblank also turns the delay off: for [`WINDOW`] frames, and twice as long as the time before for each
//! miss that follows within a window of the delay coming back, up to [`MAX_OFF`]. Without render times yet, or with the
//! switch off (`CUPERTINO_LATE_LATCH=0`), frames start at once, as on a display whose timestamps come from the server's
//! own clock and on the headless and nested backends.

use std::collections::VecDeque;
use std::time::Duration;

/// Turns the late start off when set to `0` or `off`; on otherwise.
pub const ENV: &str = "CUPERTINO_LATE_LATCH";

/// How many recent frames' render times the delay is taken from, and how many frames a first miss turns it off for:
/// one second of frames at 60 Hz.
pub const WINDOW: usize = 60;

/// The longest the delay stays off after misses that follow one another: 16 windows, 16 seconds at 60 Hz.
pub const MAX_OFF: u32 = WINDOW as u32 * 16;

/// Time kept between the recent maximum render time and the vblank: the commit, the flip's own latency and the loop's
/// wake-up.
pub const MARGIN: Duration = Duration::from_micros(2_000);

/// Whether the late start is on, from the variable's value.
pub fn requested(value: Option<&str>) -> bool {
    !matches!(value.map(str::trim), Some("0" | "off"))
}

/// When the GPU finished a frame, as far as the backend can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuDone {
    /// Within the CPU time: the renderer waited for the GPU, or the frame drew nothing of its own on it (a client
    /// buffer scanned out).
    WithinCpu,
    /// Its render fence signalled this long after the frame started.
    At(Duration),
    /// Not known: the fence could not be read.
    Unknown,
}

/// The times of a frame whose flip came.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameTimes {
    /// When it started, on the clock of the flips' timestamps.
    pub started: Duration,
    /// From its start to its flip being queued.
    pub cpu: Duration,
    /// When the GPU finished it.
    pub gpu: GpuDone,
    /// It started late after a vblank, held by the delay.
    pub held: bool,
}

/// The recent render times of one display and the delay they allow.
#[derive(Clone, Debug, Default)]
pub struct LateLatch {
    times: VecDeque<Duration>,
    /// The GPU's part of a frame whose fence cannot be read, kept from the misses of such frames.
    gpu_estimate: Duration,
    /// Frames left with the delay off.
    off: u32,
    /// How many frames the last miss turned the delay off for; 0 before any miss.
    off_len: u32,
    /// Frames since the last miss; `None` before any.
    since_miss: Option<u32>,
}

impl LateLatch {
    /// A frame took `took` from the start of drawing to the GPU finishing it.
    pub fn rendered(&mut self, took: Duration) {
        self.push(took);
        self.off = self.off.saturating_sub(1);
        self.since_miss = self.since_miss.map(|n| n.saturating_add(1));
    }

    fn push(&mut self, took: Duration) {
        if self.times.len() == WINDOW {
            self.times.pop_front();
        }
        self.times.push_back(took);
    }

    /// A frame's render time from its CPU time `cpu` and when its GPU work was done: the later of the two, or the CPU
    /// time and the kept estimate where the fence could not be read.
    pub fn render_time(&self, cpu: Duration, gpu: GpuDone) -> Duration {
        match gpu {
            GpuDone::WithinCpu => cpu,
            GpuDone::At(done) => cpu.max(done),
            GpuDone::Unknown => cpu + self.gpu_estimate,
        }
    }

    /// A frame's flip came after the vblank it was drawn for: it counts as a whole `refresh` in the window, and the
    /// delay is off for [`WINDOW`] frames, or for twice as long as the last time when the delay had been back for at
    /// most a window, up to [`MAX_OFF`].
    pub fn missed(&mut self, refresh: Duration) {
        self.push(refresh);
        let window = WINDOW as u32;
        let soon = self
            .since_miss
            .is_some_and(|n| n <= self.off_len.saturating_add(window));
        self.off_len = if soon {
            self.off_len.saturating_mul(2).clamp(window, MAX_OFF)
        } else {
            window
        };
        self.off = self.off_len;
        self.since_miss = Some(0);
    }

    /// The longest render time in the window.
    pub fn max_render(&self) -> Option<Duration> {
        self.times.iter().max().copied()
    }

    /// The kept estimate of the GPU's part of a frame whose fence cannot be read.
    pub fn gpu_estimate(&self) -> Duration {
        self.gpu_estimate
    }

    /// How many more frames the delay stays off for.
    pub fn off_for(&self) -> u32 {
        self.off
    }

    /// How long after a vblank the next frame starts on a display of period `refresh`: the period less the recent
    /// maximum render time and [`MARGIN`], at most half the period; nothing without render times or while a miss keeps
    /// the delay off.
    pub fn delay(&self, refresh: Duration) -> Duration {
        let Some(max) = self.max_render().filter(|_| self.off == 0) else {
            return Duration::ZERO;
        };
        refresh
            .saturating_sub(max)
            .saturating_sub(MARGIN)
            .min(refresh / 2)
    }

    /// When the frame after the vblank at `vblank` starts: `None` (at once) when the delay is nothing.
    pub fn start_at(&self, vblank: Duration, refresh: Duration) -> Option<Duration> {
        let delay = self.delay(refresh);
        (!delay.is_zero()).then_some(vblank + delay)
    }

    /// The flip of a frame drawn for the vblank at `target` (`None`: drawn while the phase was unknown) came, with the
    /// driver's timestamp `stamp` (`None`: the server's own clock stood in for it); `frame` holds the frame's times,
    /// where the backend kept them, and its render time joins the window. How many vblanks late the flip was, which
    /// counts as a miss here, and when the next frame starts: `None`, at once, without a real timestamp or with the
    /// late start off (`on`). A held frame whose GPU time is not known and which still missed needed more than the
    /// time from its start to that vblank, so the kept estimate of the GPU's part rises to at least that less its CPU
    /// time.
    pub fn flipped(
        &mut self,
        frame: Option<FrameTimes>,
        stamp: Option<Duration>,
        target: Option<Duration>,
        refresh: Duration,
        on: bool,
    ) -> Flipped {
        if let Some(f) = frame {
            self.rendered(self.render_time(f.cpu, f.gpu));
        }
        let late = match (stamp, target) {
            (Some(shown), Some(target)) => periods_late(target, shown, refresh),
            _ => 0,
        };
        if late > 0 {
            if let (Some(f), Some(target)) = (frame, target) {
                if f.held && f.gpu == GpuDone::Unknown {
                    let needed = target.saturating_sub(f.started).saturating_sub(f.cpu);
                    self.gpu_estimate = self.gpu_estimate.max(needed).min(refresh);
                }
            }
            self.missed(refresh);
        }
        let start_at = stamp
            .filter(|_| on)
            .and_then(|vblank| self.start_at(vblank, refresh));
        Flipped { late, start_at }
    }
}

/// What a flip means for the next frame ([`LateLatch::flipped`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flipped {
    /// Vblanks after the one the frame was drawn for.
    pub late: u32,
    /// When the next frame starts; `None`: at once.
    pub start_at: Option<Duration>,
}

/// How many vblanks after `target` (the vblank a frame was drawn for) a flip shown at `shown` came, on a display of
/// period `refresh`: 0 for one shown on time, a flip half a period off still counting as on time (timestamps jitter).
pub fn periods_late(target: Duration, shown: Duration, refresh: Duration) -> u32 {
    if refresh.is_zero() {
        return 0;
    }
    let late = shown.saturating_sub(target);
    let periods = (late + refresh / 2).as_nanos() / refresh.as_nanos();
    u32::try_from(periods).unwrap_or(u32::MAX)
}
