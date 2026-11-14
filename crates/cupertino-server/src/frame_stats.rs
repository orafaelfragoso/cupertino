//! Per-frame timings of an offscreen output, for the frame-rate benchmark (`--frame-stats PATH`, `scripts/dev/bench`).
//!
//! Each frame drawn for presentation writes one line: when it was shown, how long composing the scene and rendering it
//! took (the GPU waited for, so the time covers the GPU's work too), how much of that the glass passes and their
//! backdrop blurs took on the GPU, how many glass backdrops were blurred again or reused, how late it came after the
//! output owed it and how many refreshes passed meanwhile, and the input-to-present latency of the frame when it is the
//! first shown after an input; then how many render targets the material and glass passes created and how many passes
//! they ran; then the GPU time of the whole frame (by GPU timestamps, `-` where the device has none), the title bars and
//! rims and the keyboard HUDs drawn on the CPU, their uploads and the bytes those sent, and the passes of backdrop blurs.
//!
//! An output owes a frame from the time it is due ([`crate::state::Cupertino::frame_deadline`]): a change is due at the
//! next refresh, an animation frame at its grid point. A frame shown more than a refresh after it was owed missed the
//! refreshes in between ([`missed_refreshes`]); an output at rest owes nothing, so its rests count as no miss.
//!
//! Latency is counted two ways. `input` ends at the first frame shown after the input: what the server itself moves (a
//! dragged window, the pointer) shows there. `app` ends at the first frame shown after a client committed in answer
//! to it: a scrolled list or a magnified dock shows there. A commit later than [`APP_ANSWER_WITHIN`] is not taken as an
//! answer.

use std::io::Write;
use std::time::{Duration, Instant};

/// The longest a client commit may follow an input and still count as its answer.
pub const APP_ANSWER_WITHIN: Duration = Duration::from_millis(250);

/// The first frame shown after an input, and the first after a client answered it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Latency {
    /// The earliest input no frame has shown yet.
    input: Option<Instant>,
    /// The earliest input no client answer has shown yet.
    app: Option<Instant>,
    /// A client committed after [`Latency::app`].
    answered: bool,
}

impl Latency {
    /// An input arrived at `at`.
    pub fn input(&mut self, at: Instant) {
        self.input.get_or_insert(at);
        if self.app.is_none() {
            self.app = Some(at);
            self.answered = false;
        }
    }

    /// A client committed at `at`.
    pub fn commit(&mut self, at: Instant) {
        match self.app {
            Some(from) if at.saturating_duration_since(from) > APP_ANSWER_WITHIN => {
                self.app = None;
                self.answered = false;
            }
            Some(_) => self.answered = true,
            None => {}
        }
    }

    /// A frame was shown at `at`: the input latency and the client-answer latency it ends, if any.
    pub fn presented(&mut self, at: Instant) -> (Option<Duration>, Option<Duration>) {
        let input = self.input.take().map(|t| at.saturating_duration_since(t));
        let app = match self.app {
            Some(t) if self.answered => {
                self.app = None;
                self.answered = false;
                Some(at.saturating_duration_since(t))
            }
            _ => None,
        };
        (input, app)
    }
}

/// The refreshes of `refresh` that passed with a frame owed and none shown: a frame shown `late` after it was owed.
pub fn missed_refreshes(late: Duration, refresh: Duration) -> u64 {
    if refresh.is_zero() {
        return 0;
    }
    (late.as_nanos() / refresh.as_nanos()) as u64
}

/// The time the glass passes took within one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlassTime {
    /// Capturing and blurring backdrops, GPU included.
    pub blur: Duration,
    /// The glass draws, blurs included.
    pub total: Duration,
}

/// One frame shown.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameRecord {
    /// When it was shown, from the server's start.
    pub shown: Duration,
    pub compose: Duration,
    /// From the start of the frame's drawing until the GPU finished it.
    pub render: Duration,
    /// The CPU's part of `render`: until the frame's commands were issued, before waiting for the GPU.
    pub submit: Duration,
    pub glass: GlassTime,
    /// Glass backdrops blurred again and reused.
    pub blurred: u64,
    pub reused: u64,
    /// How long after the output owed it the frame was shown, and the refreshes missed meanwhile.
    pub late: Duration,
    pub missed: u64,
    pub latency: (Option<Duration>, Option<Duration>),
    /// Render targets the material and glass passes created, and the passes they ran.
    pub allocated: u64,
    pub passes: u64,
    /// The GPU's time for the whole frame, when the device has GPU timestamps.
    pub gpu: Option<Duration>,
    pub counts: DrawCounts,
}

/// What the compositor drew on the CPU and uploaded, and the blur passes it ran, within a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawCounts {
    pub decoration_rasters: u64,
    pub hud_rasters: u64,
    pub decoration_uploads: u64,
    pub decoration_upload_bytes: u64,
    pub hud_uploads: u64,
    pub hud_upload_bytes: u64,
    pub blur_passes: u64,
    /// Texels the blur passes wrote.
    pub blur_texels: u64,
}

/// The running totals [`DrawCounts`] are taken from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub material: crate::render::material::MaterialStats,
    pub decoration_rasters: u64,
    pub hud_rasters: u64,
    pub uploads: crate::render::uploads::Uploads,
}

impl Totals {
    /// The totals of `state` now.
    pub fn of(state: &crate::state::Cupertino) -> Totals {
        Totals {
            material: state
                .render
                .as_ref()
                .and_then(|r| r.material.as_ref())
                .map(|m| m.borrow().stats)
                .unwrap_or_default(),
            decoration_rasters: state.decor.renders,
            hud_rasters: state.overlays.images.rasters,
            uploads: crate::render::uploads::total(),
        }
    }

    /// What happened from `before` to these totals.
    pub fn since(&self, before: &Totals) -> DrawCounts {
        DrawCounts {
            decoration_rasters: self.decoration_rasters - before.decoration_rasters,
            hud_rasters: self.hud_rasters - before.hud_rasters,
            decoration_uploads: self.uploads.decoration - before.uploads.decoration,
            decoration_upload_bytes: self.uploads.decoration_bytes
                - before.uploads.decoration_bytes,
            hud_uploads: self.uploads.hud - before.uploads.hud,
            hud_upload_bytes: self.uploads.hud_bytes - before.uploads.hud_bytes,
            blur_passes: self.material.blur_passes - before.material.blur_passes,
            blur_texels: self.material.blur_texels - before.material.blur_texels,
        }
    }
}

impl FrameRecord {
    /// The record as one line: times in microseconds, `-` for a latency the frame does not end.
    pub fn line(&self) -> String {
        let us = |d: Duration| d.as_micros().to_string();
        let opt = |d: Option<Duration>| d.map_or_else(|| "-".into(), us);
        let c = &self.counts;
        format!(
            "{} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
            us(self.shown),
            us(self.compose),
            us(self.render),
            us(self.glass.total),
            us(self.glass.blur),
            self.blurred,
            self.reused,
            us(self.late),
            self.missed,
            opt(self.latency.0),
            opt(self.latency.1),
            self.allocated,
            self.passes,
            opt(self.gpu),
            c.decoration_rasters,
            c.hud_rasters,
            c.decoration_uploads,
            c.decoration_upload_bytes,
            c.hud_uploads,
            c.hud_upload_bytes,
            c.blur_passes,
            c.blur_texels,
            us(self.submit),
        )
    }
}

/// `glQueryCounterEXT` and `glGetQueryObjectui64vEXT` of the timer query extension.
type QueryCounter = unsafe extern "system" fn(u32, u32);
type QueryResult64 = unsafe extern "system" fn(u32, u32, *mut u64);

/// `GL_TIMESTAMP_EXT` and `GL_QUERY_RESULT_EXT`.
const TIMESTAMP: u32 = 0x8E28;
const QUERY_RESULT: u32 = 0x8866;

/// The GPU's time for a whole frame, from a timestamp before its first command and one after its last. Timestamps,
/// unlike elapsed-time queries, may be taken while the glass passes time themselves.
pub struct FrameTimer {
    counter: QueryCounter,
    result: QueryResult64,
    queries: [u32; 2],
}

impl FrameTimer {
    /// The timer of a context with GPU timer queries; `None` when the driver has no entry points for them.
    ///
    /// # Safety
    /// `gl` must be current.
    pub unsafe fn new(gl: &smithay::backend::renderer::gles::ffi::Gles2) -> Option<FrameTimer> {
        let counter = smithay::backend::egl::get_proc_address("glQueryCounterEXT");
        let result = smithay::backend::egl::get_proc_address("glGetQueryObjectui64vEXT");
        if counter.is_null() || result.is_null() {
            return None;
        }
        let mut queries = [0; 2];
        gl.GenQueries(2, queries.as_mut_ptr());
        Some(FrameTimer {
            counter: std::mem::transmute::<*const std::ffi::c_void, QueryCounter>(counter),
            result: std::mem::transmute::<*const std::ffi::c_void, QueryResult64>(result),
            queries,
        })
    }

    /// Before the frame's first command (`end` false) or after its last (`end` true).
    ///
    /// # Safety
    /// The context the timer was made in must be current.
    pub unsafe fn stamp(&self, end: bool) {
        (self.counter)(self.queries[usize::from(end)], TIMESTAMP);
    }

    /// The time between the two stamps; call once the GPU finished the frame.
    ///
    /// # Safety
    /// The context the timer was made in must be current.
    pub unsafe fn elapsed(&self) -> Duration {
        let (mut start, mut end) = (0u64, 0u64);
        (self.result)(self.queries[0], QUERY_RESULT, &mut start);
        (self.result)(self.queries[1], QUERY_RESULT, &mut end);
        Duration::from_nanos(end.saturating_sub(start))
    }
}

/// `at` on the monotonic clock (`CLOCK_MONOTONIC`), in nanoseconds.
pub fn monotonic_ns(at: Instant) -> u128 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid timespec to write to.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    let now = Instant::now();
    let mono = ts.tv_sec as u128 * 1_000_000_000 + ts.tv_nsec as u128;
    match at.checked_duration_since(now) {
        Some(ahead) => mono + ahead.as_nanos(),
        None => mono.saturating_sub(now.duration_since(at).as_nanos()),
    }
}

/// The recorder: the latency bookkeeping and the file the lines go to.
pub struct FrameStats {
    pub start: Instant,
    pub latency: Latency,
    /// From when each output (by index) owes the frame it has not shown yet, on the frame clock.
    owed: Vec<Option<Duration>>,
    refresh: Duration,
    /// The counters when the frame being drawn began.
    before: Totals,
    /// Times whole frames on the GPU, where the device can.
    pub timer: Option<FrameTimer>,
    out: std::io::BufWriter<std::fs::File>,
}

/// The first line of the file: the column names.
pub const HEADER: &str = "# shown_us compose_us render_us glass_us blur_us blurred reused late_us missed input_lat_us \
app_lat_us allocated passes gpu_us deco_rasters hud_rasters deco_uploads deco_upload_bytes hud_uploads hud_upload_bytes \
blur_passes blur_texels submit_us";

impl FrameStats {
    /// Write to `path`, naming the device that renders (`renderer`) on the first line; times count from `start`.
    pub fn create(
        path: &std::path::Path,
        renderer: &str,
        refresh: Duration,
        start: Instant,
    ) -> std::io::Result<Self> {
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        writeln!(out, "# renderer {renderer}")?;
        writeln!(out, "# refresh_us {}", refresh.as_micros())?;
        // where `start` lies on the monotonic clock, so the times of other processes line up with these
        writeln!(out, "# start_monotonic_ns {}", monotonic_ns(start))?;
        writeln!(out, "{HEADER}")?;
        // the header at once: a reader lines its own clock up by it while the session runs
        out.flush()?;
        Ok(Self {
            start,
            latency: Latency::default(),
            owed: Vec::new(),
            refresh,
            before: Default::default(),
            timer: None,
            out,
        })
    }

    /// Output `idx` owes a frame from `due` (the frame clock) on, unless it owed one from earlier.
    pub fn owe(&mut self, idx: usize, due: Duration) {
        if self.owed.len() <= idx {
            self.owed.resize(idx + 1, None);
        }
        let owed = &mut self.owed[idx];
        *owed = Some(owed.map_or(due, |o| o.min(due)));
    }

    /// Output `idx` showed a frame at `now` (the frame clock): how late it came after it was owed.
    pub fn shown(&mut self, idx: usize, now: Duration) -> Duration {
        self.owed
            .get_mut(idx)
            .and_then(Option::take)
            .map_or(Duration::ZERO, |o| now.saturating_sub(o))
    }

    pub fn write(&mut self, record: &FrameRecord) {
        let _ = writeln!(self.out, "{}", record.line());
    }

    pub fn flush(&mut self) {
        let _ = self.out.flush();
    }
}

/// Before a frame is composed: the glass passes time themselves from here on, and their counters are noted.
pub fn start_glass_timing(state: &mut crate::state::Cupertino) {
    if state.frame_stats.is_none() {
        return;
    }
    let totals = Totals::of(state);
    if let Some(stats) = state.frame_stats.as_mut() {
        stats.before = totals;
    }
    if let Some(material) = state.render.as_ref().and_then(|r| r.material.as_ref()) {
        let mut m = material.borrow_mut();
        m.timing = true;
        m.glass_time = GlassTime::default();
    }
}

/// The outputs that owe a frame at `now` (the frame clock), noted for [`frame_rendered`].
pub fn note_owed(state: &mut crate::state::Cupertino, now: Duration) {
    if state.frame_stats.is_none() {
        return;
    }
    let due: Vec<(usize, Duration)> = (0..state.outputs.len())
        .filter(|&idx| state.displays.powered(state.outputs[idx].id))
        .filter_map(|idx| state.frame_deadline(idx, now).map(|at| (idx, at)))
        .filter(|(_, at)| *at <= now)
        .collect();
    if let Some(stats) = state.frame_stats.as_mut() {
        for (idx, at) in due {
            stats.owe(idx, at);
        }
    }
}

/// Once the frame's GPU work finished: the GPU times of its timed passes. Call with the renderer's context current.
///
/// # Safety
/// `gl` must be the current context of the renderer the material pipeline was made on.
pub unsafe fn collect_pass_times(
    state: &crate::state::Cupertino,
    gl: &smithay::backend::renderer::gles::ffi::Gles2,
) {
    if let Some(material) = state.render.as_ref().and_then(|r| r.material.as_ref()) {
        material.borrow_mut().collect_pass_times(gl);
    }
}

/// A frame of output `idx` was rendered and its GPU work finished: write its line.
pub fn frame_rendered(
    state: &mut crate::state::Cupertino,
    idx: usize,
    compose: Duration,
    render: Duration,
    submit: Duration,
    gpu: Option<Duration>,
) {
    let clock = state.frame_now();
    let glass = state
        .render
        .as_ref()
        .and_then(|r| r.material.as_ref())
        .map(|m| m.borrow().glass_time)
        .unwrap_or_default();
    let totals = Totals::of(state);
    let Some(stats) = state.frame_stats.as_mut() else {
        return;
    };
    let now = Instant::now();
    let late = stats.shown(idx, clock);
    let counts = totals.material;
    let before = stats.before.material;
    let record = FrameRecord {
        shown: now.saturating_duration_since(stats.start),
        compose,
        render,
        submit,
        glass,
        blurred: counts.backdrop_misses - before.backdrop_misses,
        reused: counts.backdrop_hits - before.backdrop_hits,
        late,
        missed: missed_refreshes(late, stats.refresh),
        latency: stats.latency.presented(now),
        allocated: counts.allocations - before.allocations,
        passes: counts.passes - before.passes,
        gpu,
        counts: totals.since(&stats.before),
    };
    stats.write(&record);
}

/// An input arrived (benchmark runs and the DRM trace only).
pub fn note_input(state: &mut crate::state::Cupertino) {
    if let Some(stats) = state.frame_stats.as_mut() {
        stats.latency.input(Instant::now());
    }
    if let Some(trace) = state.drm_trace.as_mut() {
        trace.input(Instant::now());
    }
}

/// A client committed (benchmark runs only).
pub fn note_commit(state: &mut crate::state::Cupertino) {
    if let Some(stats) = state.frame_stats.as_mut() {
        stats.latency.commit(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn an_input_ends_at_the_next_frame_and_its_answer_at_the_frame_after_the_commit() {
        let t0 = Instant::now();
        let mut l = Latency::default();
        l.input(t0);
        l.input(t0 + ms(2));
        assert_eq!(l.presented(t0 + ms(5)), (Some(ms(5)), None));
        l.commit(t0 + ms(9));
        assert_eq!(l.presented(t0 + ms(13)), (None, Some(ms(13))));
        assert_eq!(l.presented(t0 + ms(20)), (None, None));
    }

    #[test]
    fn a_commit_long_after_an_input_is_not_its_answer() {
        let t0 = Instant::now();
        let mut l = Latency::default();
        l.input(t0);
        l.presented(t0 + ms(1));
        l.commit(t0 + APP_ANSWER_WITHIN + ms(1));
        assert_eq!(l.presented(t0 + ms(400)), (None, None));
        l.input(t0 + ms(500));
        l.commit(t0 + ms(504));
        assert_eq!(l.presented(t0 + ms(508)), (Some(ms(8)), Some(ms(8))));
    }

    #[test]
    fn a_frame_shown_within_a_refresh_of_being_owed_misses_none_and_a_rest_owes_nothing() {
        let refresh = Duration::from_micros(8333);
        assert_eq!(missed_refreshes(ms(3), refresh), 0);
        assert_eq!(missed_refreshes(ms(8), refresh), 0);
        assert_eq!(missed_refreshes(ms(9), refresh), 1);
        assert_eq!(missed_refreshes(ms(30), refresh), 3);
        let mut owed = Vec::<Option<Duration>>::new();
        let mut owe = |idx: usize, due: Duration| {
            if owed.len() <= idx {
                owed.resize(idx + 1, None);
            }
            owed[idx] = Some(owed[idx].map_or(due, |o: Duration| o.min(due)));
            owed[idx]
        };
        // owed from the first time it was due, however often the loop looks again
        assert_eq!(owe(0, ms(100)), Some(ms(100)));
        assert_eq!(owe(0, ms(104)), Some(ms(100)));
    }

    #[test]
    fn the_recorder_counts_lateness_from_when_the_frame_was_first_owed() {
        let path = std::env::temp_dir().join(format!("frame-stats-{}.txt", std::process::id()));
        let mut stats =
            FrameStats::create(&path, "test", Duration::from_micros(8333), Instant::now()).unwrap();
        stats.owe(0, ms(100));
        stats.owe(0, ms(104));
        assert_eq!(stats.shown(0, ms(120)), ms(20));
        // nothing owed since: a frame drawn after a rest is not late
        assert_eq!(stats.shown(0, ms(900)), Duration::ZERO);
        // another output keeps its own debt
        stats.owe(1, ms(50));
        assert_eq!(stats.shown(0, ms(60)), Duration::ZERO);
        assert_eq!(stats.shown(1, ms(60)), ms(10));
        drop(stats);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_instant_lies_on_the_monotonic_clock_where_the_clock_read_it() {
        let a = Instant::now();
        let b = a + Duration::from_millis(5);
        let (ma, mb) = (monotonic_ns(a), monotonic_ns(b));
        // the two readings of the clock differ by at most what passed between them
        let d = mb as i128 - ma as i128 - 5_000_000;
        assert!(d.abs() < 2_000_000, "{d}");
    }

    #[test]
    fn the_header_is_on_disk_before_the_first_frame() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frames.txt");
        let _stats = FrameStats::create(&path, "test", ms(16), Instant::now()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# start_monotonic_ns "), "{text}");
        assert!(text.ends_with(&format!("{HEADER}\n")), "{text}");
    }

    #[test]
    fn a_line_names_every_column() {
        let r = FrameRecord {
            shown: ms(1),
            latency: (Some(ms(3)), None),
            allocated: 4,
            passes: 9,
            ..Default::default()
        };
        assert_eq!(
            r.line().split(' ').count(),
            HEADER.trim_start_matches("# ").split(' ').count()
        );
        assert!(r.line().ends_with("3000 - 4 9 - 0 0 0 0 0 0 0 0 0"));
    }

    #[test]
    fn a_line_ends_with_the_frames_gpu_time_its_cpu_drawing_uploads_and_blur_passes_and_its_submit_time(
    ) {
        let r = FrameRecord {
            gpu: Some(Duration::from_micros(1500)),
            counts: DrawCounts {
                decoration_rasters: 1,
                hud_rasters: 2,
                decoration_uploads: 3,
                decoration_upload_bytes: 4000,
                hud_uploads: 5,
                hud_upload_bytes: 6000,
                blur_passes: 7,
                blur_texels: 8000,
            },
            submit: Duration::from_micros(900),
            ..Default::default()
        };
        assert!(
            r.line().ends_with(" 1500 1 2 3 4000 5 6000 7 8000 900"),
            "{}",
            r.line()
        );
    }

    #[test]
    fn the_counts_of_a_frame_are_the_change_of_the_totals() {
        let mut before = Totals {
            decoration_rasters: 10,
            hud_rasters: 3,
            ..Default::default()
        };
        before.uploads.decoration = 4;
        before.uploads.decoration_bytes = 100;
        before.material.blur_passes = 20;
        before.material.blur_texels = 5000;
        let mut after = before;
        after.decoration_rasters = 12;
        after.uploads.decoration = 6;
        after.uploads.decoration_bytes = 900;
        after.uploads.hud = 1;
        after.uploads.hud_bytes = 64;
        after.material.blur_passes = 26;
        after.material.blur_texels = 7500;
        assert_eq!(
            after.since(&before),
            DrawCounts {
                decoration_rasters: 2,
                hud_rasters: 0,
                decoration_uploads: 2,
                decoration_upload_bytes: 800,
                hud_uploads: 1,
                hud_upload_bytes: 64,
                blur_passes: 6,
                blur_texels: 2500,
            }
        );
    }
}
