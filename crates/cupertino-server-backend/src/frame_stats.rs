//! Per-frame timing, latency, and GPU timestamp recording for `--frame-stats PATH`.

use std::io::Write;
use std::time::{Duration, Instant};

/// The longest a client commit may follow an input and still count as its answer.
pub const APP_ANSWER_WITHIN: Duration = Duration::from_millis(250);

/// The first frame shown after an input, and the first after a client answered it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Latency {
    input: Option<Instant>,
    app: Option<Instant>,
    answered: bool,
    pointer: Option<Instant>,
}

impl Latency {
    pub fn input(&mut self, at: Instant) {
        self.input.get_or_insert(at);
        if self.app.is_none() {
            self.app = Some(at);
            self.answered = false;
        }
    }

    pub fn pointer(&mut self, at: Instant) {
        self.input(at);
        self.pointer.get_or_insert(at);
    }

    pub fn pointer_presented(&mut self, at: Instant) -> Option<Duration> {
        self.pointer.take().map(|t| at.saturating_duration_since(t))
    }

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

/// Material/glass pipeline counters decoupled from the render crate's internal stats struct.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialCounts {
    pub backdrop_misses: u64,
    pub backdrop_hits: u64,
    pub allocations: u64,
    pub passes: u64,
    pub blur_passes: u64,
    pub blur_texels: u64,
}

/// CPU upload counters decoupled from the render crate's internal upload tracker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UploadCounts {
    pub decoration: u64,
    pub decoration_bytes: u64,
    pub hud: u64,
    pub hud_bytes: u64,
}

/// One frame shown.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameRecord {
    pub shown: Duration,
    pub compose: Duration,
    pub render: Duration,
    pub submit: Duration,
    pub glass: GlassTime,
    pub blurred: u64,
    pub reused: u64,
    pub late: Duration,
    pub missed: u64,
    pub latency: (Option<Duration>, Option<Duration>),
    pub allocated: u64,
    pub passes: u64,
    pub gpu: Option<Duration>,
    pub counts: DrawCounts,
    pub pointer_latency: Option<Duration>,
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
    pub blur_texels: u64,
    pub decoration_draws: u64,
    pub hud_draws: u64,
}

/// Running totals [`DrawCounts`] are diffed from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub material: MaterialCounts,
    pub decoration_rasters: u64,
    pub hud_rasters: u64,
    pub uploads: UploadCounts,
    pub decoration_draws: u64,
    pub hud_draws: u64,
}

impl Totals {
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
            decoration_draws: self.decoration_draws - before.decoration_draws,
            hud_draws: self.hud_draws - before.hud_draws,
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
            "{} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
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
            opt(self.pointer_latency),
            us(self.submit),
            c.decoration_draws,
            c.hud_draws,
        )
    }
}

type QueryCounter = unsafe extern "system" fn(u32, u32);
type QueryResult64 = unsafe extern "system" fn(u32, u32, *mut u64);

const TIMESTAMP: u32 = 0x8E28;
const QUERY_RESULT: u32 = 0x8866;

/// GPU timestamp query pair for timing a whole frame on the GPU.
pub struct FrameTimer {
    counter: QueryCounter,
    result: QueryResult64,
    queries: [u32; 2],
}

impl FrameTimer {
    /// Create a timer on a context with `EXT_disjoint_timer_query`; `None` if unavailable.
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

    /// Elapsed GPU duration between the start and end stamps.
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

/// The first line of the file: the column names.
pub const HEADER: &str = "# shown_us compose_us render_us glass_us blur_us blurred reused late_us missed input_lat_us \
app_lat_us allocated passes gpu_us deco_rasters hud_rasters deco_uploads deco_upload_bytes hud_uploads hud_upload_bytes \
blur_passes blur_texels pointer_lat_us submit_us deco_draws hud_draws";

/// Frame-statistics writer and latency tracker.
pub struct FrameStats {
    pub start: Instant,
    pub latency: Latency,
    owed: Vec<Option<Duration>>,
    pub refresh: Duration,
    pub before: Totals,
    pub timer: Option<FrameTimer>,
    out: std::io::BufWriter<std::fs::File>,
}

impl FrameStats {
    pub fn create(
        path: &std::path::Path,
        renderer: &str,
        refresh: Duration,
        start: Instant,
    ) -> std::io::Result<Self> {
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        writeln!(out, "# renderer {renderer}")?;
        writeln!(out, "# refresh_us {}", refresh.as_micros())?;
        writeln!(out, "# start_monotonic_ns {}", monotonic_ns(start))?;
        writeln!(out, "{HEADER}")?;
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

    pub fn owe(&mut self, idx: usize, due: Duration) {
        if self.owed.len() <= idx {
            self.owed.resize(idx + 1, None);
        }
        let owed = &mut self.owed[idx];
        *owed = Some(owed.map_or(due, |o| o.min(due)));
    }

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
