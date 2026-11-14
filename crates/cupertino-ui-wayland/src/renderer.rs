//! Which renderer a process draws with, and why: the fallback rule as a model without a display, the size and repaint
//! rule of a process that declares no class, and the record of the choice each process leaves for `cupertino-diagnose`
//! and the frame statistics.
//!
//! A process draws on the CPU when `CUPERTINO_RENDERER=cpu` is set, when its class is the CPU's
//! ([`RendererClass::Cpu`]) or the size and repaint rule has not yet asked for the GPU ([`RendererClass::ByUse`],
//! [`large`], [`Busy`]), when the compositor offers no dmabuf feedback or its device has no render node the process
//! can open, when EGL or GBM cannot be loaded or started on that device, when the device is a software rasterizer, or
//! after its context was lost three times within a minute. Video-memory purges are never counted.
//! `CUPERTINO_RENDERER=gpu` overrides the class, the software and missing-feedback conditions (it takes the driver's
//! default device then), which tests use; nothing overrides missing libraries or the losses. A process that wanted the
//! GPU and draws on the CPU instead says so on standard error and in a diagnostic event ([`fallback`]).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub use cupertino_ui_platform::{RendererClass, RendererKind};

/// The variable that asks for a renderer: `cpu` or `gpu`.
pub const RENDERER_ENV: &str = "CUPERTINO_RENDERER";

/// Losses within [`LOSS_WINDOW`] after which a process stays on the CPU.
pub const LOSS_LIMIT: usize = 3;
/// The time [`LOSS_LIMIT`] losses are counted over.
pub const LOSS_WINDOW: Duration = Duration::from_secs(60);

/// A window covering more than one part in this many of its output's pixels is large: the size rule's quarter.
pub const LARGE_WINDOW_PARTS: u64 = 4;
/// Frames a second a window must present more than to be busy.
pub const BUSY_FRAMES_PER_SECOND: u32 = 30;
/// Whole seconds in a row a window must be busy before its process moves to the GPU.
pub const BUSY_SECONDS: u32 = 2;

/// Whether the class puts the process on the GPU from its start (the rule's class waits for a large or busy window).
pub fn on_at_start(class: RendererClass) -> bool {
    class == RendererClass::Gpu
}

/// Whether a window of `window` pixels is large on an output of `output` pixels: more than a quarter of it.
pub fn large(window: (u32, u32), output: (u32, u32)) -> bool {
    let area = |(w, h): (u32, u32)| u64::from(w) * u64::from(h);
    let output = area(output);
    output > 0 && area(window) * LARGE_WINDOW_PARTS > output
}

/// The repaint half of the rule for one window: its presents counted by whole seconds of the clock, from its first.
#[derive(Clone, Debug, Default)]
pub struct Busy {
    /// When the second being counted began, in nanoseconds of the clock.
    second: Option<u64>,
    /// Frames presented in it.
    frames: u32,
    /// Busy seconds in a row before it.
    seconds: u32,
}

impl Busy {
    /// Count a frame presented at `at_ns` (monotonic nanoseconds); whether the window has now presented more than
    /// [`BUSY_FRAMES_PER_SECOND`] in each of [`BUSY_SECONDS`] whole seconds in a row.
    pub fn presented(&mut self, at_ns: u64) -> bool {
        const SECOND: u64 = 1_000_000_000;
        let start = *self.second.get_or_insert(at_ns);
        if at_ns >= start.saturating_add(SECOND) {
            let passed = (at_ns - start) / SECOND;
            let busy = self.frames > BUSY_FRAMES_PER_SECOND;
            // a second with no frame at all between ends the run
            self.seconds = match (busy, passed) {
                (true, 1) => self.seconds + 1,
                _ => 0,
            };
            self.second = Some(start + passed * SECOND);
            self.frames = 0;
        }
        self.frames += 1;
        self.seconds >= BUSY_SECONDS
    }
}

/// What the environment asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requested {
    /// Nothing: the rule decides.
    Default,
    Cpu,
    /// The GPU even on a software device or without feedback.
    Gpu,
}

impl Requested {
    /// `CUPERTINO_RENDERER`'s value: `cpu`, `gpu`, anything else (or nothing) the default.
    pub fn parse(value: Option<&str>) -> Requested {
        match value.map(str::trim) {
            Some("cpu") => Requested::Cpu,
            Some("gpu") => Requested::Gpu,
            _ => Requested::Default,
        }
    }

    pub fn from_env() -> Requested {
        Requested::parse(std::env::var(RENDERER_ENV).ok().as_deref())
    }
}

/// Why a process draws with its renderer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// `CUPERTINO_RENDERER=cpu`.
    AskedForCpu,
    /// The process's class draws on the CPU ([`RendererClass::Cpu`]).
    CpuClass,
    /// The process takes the size and repaint rule and no window of it has been large or busy yet.
    NotYetLargeOrBusy,
    /// The compositor offers no dmabuf feedback.
    NoFeedback,
    /// The feedback names a device whose render node the process cannot find (a sandbox without `/dev/dri`).
    NoRenderNode,
    /// EGL or GBM is missing or cannot start on the device.
    Unavailable(String),
    /// The device is a software rasterizer.
    Software,
    /// The device allocates no dmabufs the compositor takes.
    NoAllocator,
    /// The context was lost [`LOSS_LIMIT`] times within [`LOSS_WINDOW`].
    Losses,
    /// The process draws on the GPU while its context and programs are made; the first frames are the CPU's.
    WarmingUp,
    /// The GPU device works.
    Gpu,
    /// The GPU device works, taken because `CUPERTINO_RENDERER=gpu` overrode the rule.
    Forced,
}

impl Reason {
    /// A short name for logs, the diagnostics and the frame statistics.
    pub fn label(&self) -> String {
        match self {
            Reason::AskedForCpu => "asked for the CPU".into(),
            Reason::CpuClass => "its class draws on the CPU".into(),
            Reason::NotYetLargeOrBusy => "no window large or busy enough yet".into(),
            Reason::NoFeedback => "no dmabuf feedback".into(),
            Reason::NoRenderNode => "no render node for the dmabuf device".into(),
            Reason::Unavailable(why) => format!("no GPU device: {why}"),
            Reason::Software => "a software rasterizer".into(),
            Reason::NoAllocator => "no dmabuf allocator".into(),
            Reason::Losses => format!(
                "the context was lost {LOSS_LIMIT} times within {} s",
                LOSS_WINDOW.as_secs()
            ),
            Reason::WarmingUp => "warming up".into(),
            Reason::Gpu => "the GPU".into(),
            Reason::Forced => "asked for the GPU".into(),
        }
    }

    /// Whether a process with this reason wanted the GPU and could not have it: a fallback, logged and recorded as a
    /// diagnostic event ([`fallback`]).
    pub fn is_fallback(&self) -> bool {
        matches!(
            self,
            Reason::NoFeedback
                | Reason::NoRenderNode
                | Reason::Unavailable(_)
                | Reason::Software
                | Reason::NoAllocator
                | Reason::Losses
        )
    }
}

/// What the rule says before a device exists: draw on the CPU for `reason`, or make a device (on `device`, a render
/// node, or the driver's default device when `None`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    Cpu(Reason),
    Device { node: Option<PathBuf> },
}

/// What the compositor's dmabuf feedback said about the device to draw on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Feedback {
    /// No dmabuf protocol, or it was not asked (a process on the CPU never binds it).
    None,
    /// The compositor offers dmabufs and names no device.
    NoDevice,
    /// It names a device whose render node the process cannot find.
    NoNode,
    /// The render node of the device it names.
    Node(PathBuf),
}

/// Why a process in the rule's class still waits: `CpuClass` for the CPU's class, `NotYetLargeOrBusy` for the rule's.
fn waiting(class: RendererClass) -> Reason {
    match class {
        RendererClass::ByUse => Reason::NotYetLargeOrBusy,
        _ => Reason::CpuClass,
    }
}

/// The first half of the rule: the request, the process's class, whether the class wants the GPU now (always for
/// the GPU's class, once a window was large or busy for the rule's), and the feedback's device.
pub fn start(
    requested: Requested,
    class: RendererClass,
    wanted: bool,
    feedback: Feedback,
) -> Start {
    match requested {
        Requested::Cpu => return Start::Cpu(Reason::AskedForCpu),
        Requested::Default if class == RendererClass::Cpu || !wanted => {
            return Start::Cpu(waiting(class))
        }
        _ => {}
    }
    match (feedback, requested) {
        (Feedback::Node(node), _) => Start::Device { node: Some(node) },
        (_, Requested::Gpu) => Start::Device { node: None },
        (Feedback::NoNode, _) => Start::Cpu(Reason::NoRenderNode),
        _ => Start::Cpu(Reason::NoFeedback),
    }
}

/// The second half: the device made (or why not), whether it is software and whether it allocates dmabufs.
pub fn settle(
    requested: Requested,
    device: Result<(bool, bool), String>,
    history_exceeded: bool,
) -> (RendererKind, Reason) {
    if history_exceeded {
        return (RendererKind::Cpu, Reason::Losses);
    }
    let (software, allocates) = match device {
        Err(why) => return (RendererKind::Cpu, Reason::Unavailable(why)),
        Ok(facts) => facts,
    };
    if !allocates {
        return (RendererKind::Cpu, Reason::NoAllocator);
    }
    match (software, requested) {
        (true, Requested::Gpu) => (RendererKind::Gpu, Reason::Forced),
        (true, _) => (RendererKind::Cpu, Reason::Software),
        (false, Requested::Gpu) => (RendererKind::Gpu, Reason::Forced),
        (false, _) => (RendererKind::Gpu, Reason::Gpu),
    }
}

/// The context losses of a process: resets count toward the CPU fallback, purges never do.
#[derive(Clone, Debug, Default)]
pub struct LossHistory {
    resets: VecDeque<Instant>,
}

impl LossHistory {
    /// Count a loss at `now` (`counts`: a reset, not a purge); whether the process must stay on the CPU from now on.
    pub fn record(&mut self, counts: bool, now: Instant) -> bool {
        if counts {
            self.resets.push_back(now);
        }
        self.exceeded(now)
    }

    /// Whether [`LOSS_LIMIT`] resets happened within the last [`LOSS_WINDOW`].
    pub fn exceeded(&mut self, now: Instant) -> bool {
        while self
            .resets
            .front()
            .is_some_and(|&at| now.saturating_duration_since(at) >= LOSS_WINDOW)
        {
            self.resets.pop_front();
        }
        self.resets.len() >= LOSS_LIMIT
    }
}

/// The renderer a process draws with now, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub kind: RendererKind,
    pub reason: Reason,
}

static ACTIVE: Mutex<Option<Choice>> = Mutex::new(None);

/// The renderer this process draws with: the CPU until a window system decided otherwise.
pub fn active() -> Choice {
    ACTIVE
        .lock()
        .ok()
        .and_then(|c| c.clone())
        .unwrap_or(Choice {
            kind: RendererKind::Cpu,
            reason: Reason::CpuClass,
        })
}

/// The name the frame statistics give the renderer this process draws with now (`cpu` or `gpu`).
pub fn active_name() -> &'static str {
    active().kind.name()
}

/// Record the renderer this process draws with: logged once per change on standard error, and written to the
/// session's record of process renderers ([`record_dir`]) that `cupertino-diagnose` reads.
pub fn set_active(choice: Choice) {
    let changed = ACTIVE
        .lock()
        .map(|mut c| c.replace(choice.clone()).as_ref() != Some(&choice))
        .unwrap_or(false);
    if !changed {
        return;
    }
    eprintln!(
        "renderer: {} ({})",
        choice.kind.name(),
        choice.reason.label()
    );
    if choice.reason.is_fallback() {
        fallback(FallbackScope::Process, &choice.reason.label());
    }
    if let Some(dir) = record_dir() {
        let _ = write_record(&dir, std::process::id(), &program(), &choice);
    }
}

/// What a fallback from the GPU took to the CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FallbackScope {
    /// The whole process draws on the CPU.
    Process,
    /// One surface presents from shared memory; the process stays on the GPU.
    Surface,
    /// The context was lost: the process draws on the CPU until a new device is made, or for good after the third
    /// reset within a minute.
    Loss,
}

impl FallbackScope {
    pub fn name(self) -> &'static str {
        match self {
            FallbackScope::Process => "process",
            FallbackScope::Surface => "surface",
            FallbackScope::Loss => "loss",
        }
    }
}

/// The line a fallback logs on standard error.
pub fn fallback_line(scope: FallbackScope, why: &str) -> String {
    format!("renderer: fell back to the CPU ({}): {why}", scope.name())
}

/// A process that wanted the GPU draws on the CPU instead, or one of its surfaces presents from shared memory: logged
/// on standard error and recorded as the diagnostic event `toolkit.renderer.fallback` (the program, the scope and
/// why), which lasts past the session in the user's event store, so days of use can be checked for any.
pub fn fallback(scope: FallbackScope, why: &str) {
    use cupertino_diagnostic_events::{self as diag, Severity, Value};
    eprintln!("{}", fallback_line(scope, why));
    let program = program();
    diag::event(
        Severity::Warn,
        "toolkit.renderer.fallback",
        "a process or surface that wanted the GPU draws on the CPU",
    )
    .field("program", Value::Public(&program))
    .field("scope", Value::Public(scope.name()))
    .field("reason", Value::Public(why))
    .emit();
}

/// `$XDG_RUNTIME_DIR/cupertino/renderers`: one file per process, named by its id.
pub fn record_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|p| p.join("cupertino").join("renderers"))
}

fn program() -> String {
    std::env::args_os()
        .next()
        .and_then(|a| {
            std::path::Path::new(&a)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "unknown".into())
}

/// The record of one process: `program`, `renderer` and `reason` lines, written whole (a rename) so a reader never
/// sees half of one.
pub fn record_text(program: &str, choice: &Choice) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect()
    };
    format!(
        "program={}\nrenderer={}\nreason={}\n",
        clean(program),
        choice.kind.name(),
        clean(&choice.reason.label())
    )
}

pub fn write_record(
    dir: &std::path::Path,
    pid: u32,
    program: &str,
    choice: &Choice,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{pid}.tmp"));
    std::fs::write(&tmp, record_text(program, choice))?;
    std::fs::rename(tmp, dir.join(pid.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> Feedback {
        Feedback::Node(PathBuf::from("/dev/dri/renderD128"))
    }

    const CPU: RendererClass = RendererClass::Cpu;
    const GPU: RendererClass = RendererClass::Gpu;
    const BY_USE: RendererClass = RendererClass::ByUse;

    #[test]
    fn the_environment_asks_for_cpu_or_gpu_and_anything_else_is_the_default() {
        assert_eq!(Requested::parse(Some("cpu")), Requested::Cpu);
        assert_eq!(Requested::parse(Some(" gpu")), Requested::Gpu);
        assert_eq!(Requested::parse(Some("vulkan")), Requested::Default);
        assert_eq!(Requested::parse(None), Requested::Default);
    }

    #[test]
    fn each_class_decides_its_process_s_renderer_and_the_rule_s_waits_for_a_large_or_busy_window() {
        let device = Start::Device {
            node: Some(PathBuf::from("/dev/dri/renderD128")),
        };
        assert!(on_at_start(GPU) && !on_at_start(CPU) && !on_at_start(BY_USE));
        assert_eq!(
            start(Requested::Default, CPU, on_at_start(CPU), node()),
            Start::Cpu(Reason::CpuClass)
        );
        assert_eq!(
            start(Requested::Default, CPU, true, node()),
            Start::Cpu(Reason::CpuClass),
            "nothing but the variable takes a CPU class to the GPU"
        );
        assert_eq!(
            start(Requested::Default, GPU, on_at_start(GPU), node()),
            device
        );
        assert_eq!(
            start(Requested::Default, BY_USE, on_at_start(BY_USE), node()),
            Start::Cpu(Reason::NotYetLargeOrBusy)
        );
        assert_eq!(start(Requested::Default, BY_USE, true, node()), device);
        assert_eq!(RendererClass::default(), BY_USE);
        assert!(!Reason::CpuClass.is_fallback() && !Reason::NotYetLargeOrBusy.is_fallback());
    }

    #[test]
    fn cpu_wins_over_everything_and_gpu_overrides_the_class_and_missing_feedback_only() {
        for class in [CPU, GPU, BY_USE] {
            assert_eq!(
                start(Requested::Cpu, class, true, node()),
                Start::Cpu(Reason::AskedForCpu)
            );
            for feedback in [Feedback::None, Feedback::NoDevice, Feedback::NoNode] {
                assert_eq!(
                    start(Requested::Gpu, class, false, feedback),
                    Start::Device { node: None }
                );
            }
        }
        assert_eq!(
            start(Requested::Default, GPU, true, Feedback::None),
            Start::Cpu(Reason::NoFeedback)
        );
        assert_eq!(
            start(Requested::Default, GPU, true, Feedback::NoDevice),
            Start::Cpu(Reason::NoFeedback)
        );
        assert_eq!(
            start(Requested::Default, BY_USE, true, Feedback::NoNode),
            Start::Cpu(Reason::NoRenderNode),
            "a sandbox without the render node draws on the CPU"
        );
        for reason in [Reason::NoFeedback, Reason::NoRenderNode, Reason::Software] {
            assert!(reason.is_fallback(), "{reason:?}");
        }
        assert_eq!(
            fallback_line(FallbackScope::Surface, "the import was refused"),
            "renderer: fell back to the CPU (surface): the import was refused"
        );
    }

    #[test]
    fn a_window_over_a_quarter_of_its_output_is_large() {
        let output = (3840, 2160);
        assert!(!large((1920, 1080), output), "exactly a quarter");
        assert!(large((1921, 1080), output));
        assert!(large((3840, 2160), output));
        assert!(!large((900, 600), output), "a dialog");
        assert!(!large((3840, 2160), (0, 0)), "an output without a mode");
    }

    #[test]
    fn a_window_is_busy_after_two_whole_seconds_of_more_than_thirty_frames_each() {
        const MS: u64 = 1_000_000;
        let mut at_60 = Busy::default();
        let busy: Vec<bool> = (0..200).map(|i| at_60.presented(i * 16_667_000)).collect();
        // two seconds of 60 frames close at the first frame of the third
        assert_eq!(busy.iter().position(|&b| b), Some(120));
        assert!(busy[120..].iter().all(|&b| b));

        let mut at_30 = Busy::default();
        assert!(
            (0..300).all(|i| !at_30.presented(i * 1000 * MS / 30)),
            "thirty frames a second are not more than thirty"
        );

        // a busy second, a second with no frame, another busy second: no two in a row
        let mut paused = Busy::default();
        for i in 0..63 {
            assert!(!paused.presented(i * 16 * MS));
        }
        let mut t = 2100 * MS;
        while t < 3500 * MS {
            assert!(!paused.presented(t), "at {} ms", t / MS);
            t += 16 * MS;
        }
    }

    #[test]
    fn a_software_device_draws_on_the_cpu_unless_the_gpu_is_asked_for_and_missing_libraries_always_do(
    ) {
        let gpu = RendererKind::Gpu;
        let cpu = RendererKind::Cpu;
        assert_eq!(
            settle(Requested::Default, Ok((false, true)), false),
            (gpu, Reason::Gpu)
        );
        assert_eq!(
            settle(Requested::Default, Ok((true, true)), false),
            (cpu, Reason::Software)
        );
        assert_eq!(
            settle(Requested::Gpu, Ok((true, true)), false),
            (gpu, Reason::Forced)
        );
        assert_eq!(
            settle(Requested::Gpu, Err("libEGL.so.1".into()), false),
            (cpu, Reason::Unavailable("libEGL.so.1".into()))
        );
        assert_eq!(
            settle(Requested::Gpu, Ok((false, false)), false),
            (cpu, Reason::NoAllocator)
        );
        assert_eq!(
            settle(Requested::Gpu, Ok((false, true)), true),
            (cpu, Reason::Losses)
        );
    }

    #[test]
    fn three_resets_within_a_minute_end_the_gpu_and_purges_never_count() {
        let t = Instant::now();
        let mut h = LossHistory::default();
        for s in 0..10 {
            assert!(!h.record(false, t + Duration::from_secs(s)), "a purge");
        }
        assert!(!h.record(true, t));
        assert!(!h.record(true, t + Duration::from_secs(30)));
        assert!(h.record(true, t + Duration::from_secs(59)));
        let mut spread = LossHistory::default();
        assert!(!spread.record(true, t));
        assert!(!spread.record(true, t + Duration::from_secs(40)));
        assert!(
            !spread.record(true, t + Duration::from_secs(61)),
            "the first fell out of the minute"
        );
    }

    #[test]
    fn a_record_names_the_program_the_renderer_and_why_on_lines_of_their_own() {
        let choice = Choice {
            kind: RendererKind::Cpu,
            reason: Reason::Unavailable("no\nlibrary".into()),
        };
        assert_eq!(
            record_text("cupertino-files", &choice),
            "program=cupertino-files\nrenderer=cpu\nreason=no GPU device: no library\n"
        );
        let dir = std::env::temp_dir().join(format!("cupertino-renderers-{}", std::process::id()));
        write_record(&dir, 42, "files", &choice).unwrap();
        assert!(std::fs::read_to_string(dir.join("42"))
            .unwrap()
            .contains("renderer=cpu"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
