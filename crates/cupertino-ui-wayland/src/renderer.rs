//! Which renderer a process draws with, and why: the fallback rule as a model without a display, and the record of the
//! choice each process leaves for `cupertino-diagnose` and the frame statistics.
//!
//! A process draws on the CPU when `CUPERTINO_RENDERER=cpu` is set, when the GPU path is not turned on for it
//! ([`GPU_BY_DEFAULT`]), when the compositor offers no dmabuf feedback, when EGL or GBM cannot be loaded or started on
//! the device the feedback names, when that device is a software rasterizer, or after its context was lost three
//! times within a minute. Video-memory purges are never counted. `CUPERTINO_RENDERER=gpu` overrides the software and
//! missing-feedback conditions (it takes the driver's default device then), which tests use; nothing overrides missing
//! libraries or the losses.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub use cupertino_ui_platform::RendererKind;

/// Whether a process whose surfaces are of a GPU class draws on the GPU without being asked to: the one switch that
/// turns the GPU path on for every process. Off until the GPU renderer meets its budgets; `CUPERTINO_RENDERER=gpu`
/// turns it on for one process meanwhile.
pub const GPU_BY_DEFAULT: bool = false;

/// The variable that asks for a renderer: `cpu` or `gpu`.
pub const RENDERER_ENV: &str = "CUPERTINO_RENDERER";

/// Losses within [`LOSS_WINDOW`] after which a process stays on the CPU.
pub const LOSS_LIMIT: usize = 3;
/// The time [`LOSS_LIMIT`] losses are counted over.
pub const LOSS_WINDOW: Duration = Duration::from_secs(60);

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
    /// The GPU path is not turned on for the process ([`GPU_BY_DEFAULT`]).
    NotTurnedOn,
    /// The compositor offers no dmabuf feedback.
    NoFeedback,
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
            Reason::NotTurnedOn => "the GPU path is not turned on".into(),
            Reason::NoFeedback => "no dmabuf feedback".into(),
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
}

/// What the rule says before a device exists: draw on the CPU for `reason`, or make a device (on `device`, a render
/// node, or the driver's default device when `None`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    Cpu(Reason),
    Device { node: Option<PathBuf> },
}

/// The first half of the rule: the request, whether the GPU path is on, and the feedback's device (`Some(None)`: the
/// compositor offers dmabufs but names no device; `None`: no dmabuf protocol).
pub fn start(requested: Requested, turned_on: bool, feedback: Option<Option<PathBuf>>) -> Start {
    match requested {
        Requested::Cpu => return Start::Cpu(Reason::AskedForCpu),
        Requested::Default if !turned_on => return Start::Cpu(Reason::NotTurnedOn),
        _ => {}
    }
    match (feedback, requested) {
        (Some(Some(node)), _) => Start::Device { node: Some(node) },
        (_, Requested::Gpu) => Start::Device { node: None },
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
            reason: Reason::NotTurnedOn,
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
    if let Some(dir) = record_dir() {
        let _ = write_record(&dir, std::process::id(), &program(), &choice);
    }
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

    fn node() -> Option<Option<PathBuf>> {
        Some(Some(PathBuf::from("/dev/dri/renderD128")))
    }

    #[test]
    fn the_environment_asks_for_cpu_or_gpu_and_anything_else_is_the_default() {
        assert_eq!(Requested::parse(Some("cpu")), Requested::Cpu);
        assert_eq!(Requested::parse(Some(" gpu")), Requested::Gpu);
        assert_eq!(Requested::parse(Some("vulkan")), Requested::Default);
        assert_eq!(Requested::parse(None), Requested::Default);
    }

    #[test]
    fn every_process_draws_on_the_cpu_until_the_switch_is_turned_on() {
        const { assert!(!GPU_BY_DEFAULT) };
        assert_eq!(
            start(Requested::Default, GPU_BY_DEFAULT, node()),
            Start::Cpu(Reason::NotTurnedOn)
        );
        assert_eq!(
            start(Requested::Default, true, node()),
            Start::Device {
                node: Some(PathBuf::from("/dev/dri/renderD128"))
            }
        );
    }

    #[test]
    fn cpu_wins_over_everything_and_gpu_overrides_missing_feedback_only() {
        assert_eq!(
            start(Requested::Cpu, true, node()),
            Start::Cpu(Reason::AskedForCpu)
        );
        assert_eq!(
            start(Requested::Default, true, None),
            Start::Cpu(Reason::NoFeedback)
        );
        assert_eq!(
            start(Requested::Default, true, Some(None)),
            Start::Cpu(Reason::NoFeedback)
        );
        assert_eq!(
            start(Requested::Gpu, false, None),
            Start::Device { node: None }
        );
        assert_eq!(
            start(Requested::Gpu, false, Some(None)),
            Start::Device { node: None }
        );
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
