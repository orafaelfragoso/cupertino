//! Running an application on the session's Wayland display: the installed theme, the asset packs' fonts and the
//! built-in Inter, the renderer of the application's class (the GPU renderer once the process draws there) and the
//! monotonic clock.

use cupertino_assets::{Assets, FontRole};
use cupertino_defaults::appearance::{AppearanceSettings, Highlight};
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_keyboard::modifiers::Profile;
use cupertino_theme::ui::{Appearance, Color, ColorTokens, ThemePreferences, ThemeTokens};
use cupertino_theme::Theme;
use cupertino_ui_core::layout::LayoutDirection;
use cupertino_ui_core::platform::{PlatformError, SystemAppearance, WindowSystem};
use cupertino_ui_core::{
    Application, Command, EachTurn, Environment, ExitWhen, MonotonicClock, Runtime, RuntimeOptions,
    ThemeForAppearance, UiError,
};
use cupertino_ui_render::gpu::GpuConfig;
use cupertino_ui_render::{CpuConfig, CpuRenderer};
use cupertino_ui_text::GlyphRaster;
use cupertino_ui_wayland::atspi::AtspiAdapter;
use cupertino_ui_wayland::{GpuRenderers, WaylandWindowSystem};
use std::cell::Cell;
use std::fmt;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::shared::Shared;
use crate::{Entered, Instance};

/// The most nodes one application's tree holds.
const NODE_LIMIT: usize = 100_000;

#[derive(Debug)]
pub enum AppError {
    /// Not even the neutral theme could be made: the built-in control metrics lack a regular size.
    Theme(cupertino_theme::Error),
    /// The Wayland display could not be reached.
    Connect(PlatformError),
    Runtime(UiError),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Theme(e) => write!(f, "cannot read the theme: {e}"),
            AppError::Connect(e) => write!(f, "cannot connect to the Wayland display: {e:?}"),
            AppError::Runtime(e) => write!(f, "the application stopped: {e:?}"),
        }
    }
}

impl std::error::Error for AppError {}

/// Opens `A`'s windows on the session's Wayland display, publishes them to assistive technology over AT-SPI, follows
/// the session's appearance and display options, and runs until the application exits, its last window without a
/// close handler is closed, or a non-[resident](Application::RESIDENT) application closes its last window. Its
/// windows draw with the renderer of its class ([`Application::RENDERER`]). It and every other `run` here run for the
/// [`Instance`] the calling thread entered, else for the process's own.
pub fn run<A: Application>() -> Result<(), AppError> {
    let _instance = instance();
    let system = WaylandWindowSystem::connect_as(Profile::cupertino(), A::RENDERER)
        .map_err(AppError::Connect)?;
    run_on::<A>(system, A::init())
}

/// [`run`] on `system`, a connection the caller set up, from `start` instead of [`Application::init`]: a desktop
/// shell gives its surfaces their roles with [`WaylandWindowSystem::with_roles`] and starts from its options and its
/// channels to the shell.
pub fn run_on<A: Application>(
    system: WaylandWindowSystem,
    start: (A::Model, Vec<Command<A::Message>>),
) -> Result<(), AppError> {
    run_on_with::<A>(system, |_| start)
}

/// [`run_on`], with the session's appearance passed through `appearance` before the theme follows it: a shell surface
/// that keeps the dark appearance in a light session maps `dark` to true and follows the display options as they are.
pub fn run_on_with_appearance<A: Application>(
    system: WaylandWindowSystem,
    mut start: (A::Model, Vec<Command<A::Message>>),
    appearance: fn(SystemAppearance) -> SystemAppearance,
) -> Result<(), AppError> {
    let _instance = instance();
    let first = appearance(system.appearance().unwrap_or_default());
    let (mut options, theme) = options_and_theme(preferences(&first))?;
    renderers_on_gpu(&mut options, system.gpu_renderers());
    keep_caret(Arc::make_mut(&mut options.theme), &first);
    let mut personal = Personal::follow(system.waker(), theme, &options.theme, first);
    options.theme = personal.theme();
    let follow = mapped(personal.theme_for(), appearance);
    options.accessibility = Some(Box::new(AtspiAdapter::muted(system.waker())));
    start.1.extend(personal.watching());
    let renderers = system.gpu_renderers();
    Runtime::<A, _>::start(system, options, start)
        .with_theme_for(follow)
        .with_personal_refresh(personal.refresh())
        .with_each_turn(each_turn())
        .timed(renderers)
        .run()
        .map_err(AppError::Runtime)
}

/// [`run_on`], starting from what `start` makes of the runtime's options: a model that measures text itself does so
/// with the text system the windows draw with.
pub fn run_on_with<A: Application>(
    system: WaylandWindowSystem,
    start: impl FnOnce(&RuntimeOptions) -> (A::Model, Vec<Command<A::Message>>),
) -> Result<(), AppError> {
    run_shell::<A>(system, start, false, None)
}

/// [`run_on_with`] that also ends once `until` says so, asked on each turn of the loop after the window system was
/// read ([`Runtime::with_exit_when`]): a runner that stops the application from another thread sets what `until`
/// reads and wakes the window system.
pub fn run_on_with_until<A: Application>(
    system: WaylandWindowSystem,
    start: impl FnOnce(&RuntimeOptions) -> (A::Model, Vec<Command<A::Message>>),
    until: ExitWhen,
) -> Result<(), AppError> {
    run_shell::<A>(system, start, false, Some(until))
}

/// [`run_on_with`] for the application that draws the lock screen: its accessibility adapter is not muted while the
/// session is locked ([`AtspiAdapter::lock_screen`]), since what it shows then is what a person needs to hear.
pub fn run_lock_screen_on_with<A: Application>(
    system: WaylandWindowSystem,
    start: impl FnOnce(&RuntimeOptions) -> (A::Model, Vec<Command<A::Message>>),
) -> Result<(), AppError> {
    run_shell::<A>(system, start, true, None)
}

/// [`run_lock_screen_on_with`] that also ends once `until` says so ([`run_on_with_until`]).
pub fn run_lock_screen_on_with_until<A: Application>(
    system: WaylandWindowSystem,
    start: impl FnOnce(&RuntimeOptions) -> (A::Model, Vec<Command<A::Message>>),
    until: ExitWhen,
) -> Result<(), AppError> {
    run_shell::<A>(system, start, true, Some(until))
}

/// The runs of [`run_on_with`] and [`run_lock_screen_on_with`]: the lock screen's draws on the CPU and its adapter is
/// not muted while the session is locked.
fn run_shell<A: Application>(
    system: WaylandWindowSystem,
    start: impl FnOnce(&RuntimeOptions) -> (A::Model, Vec<Command<A::Message>>),
    lock_screen: bool,
    until: Option<ExitWhen>,
) -> Result<(), AppError> {
    let _instance = instance();
    // the first frame already has the session's appearance
    let (mut options, theme) = options_and_theme(start_preferences(system.appearance()))?;
    if !lock_screen {
        renderers_on_gpu(&mut options, system.gpu_renderers());
    }
    let first = system.appearance().unwrap_or_default();
    keep_caret(Arc::make_mut(&mut options.theme), &first);
    let mut personal = Personal::follow(system.waker(), theme, &options.theme, first);
    options.theme = personal.theme();
    let follow = personal.theme_for();
    let adapter = if lock_screen {
        AtspiAdapter::lock_screen(system.waker())
    } else {
        AtspiAdapter::muted(system.waker())
    };
    options.accessibility = Some(Box::new(adapter));
    let mut start = start(&options);
    start.1.extend(personal.watching());
    let renderers = system.gpu_renderers();
    let mut runtime = Runtime::<A, _>::start(system, options, start)
        .with_theme_for(follow)
        .with_personal_refresh(personal.refresh())
        .with_each_turn(each_turn())
        .timed(renderers);
    if let Some(until) = until {
        runtime = runtime.with_exit_when(until);
    }
    runtime.run().map_err(AppError::Runtime)
}

/// Names the file that [`Timed::timed`] appends the timing of each scroll to.
const SCROLL_TIMING_ENV: &str = "CUPERTINO_SCROLL_TIMING";

/// Names the file that [`Timed::timed`] appends the phases of each frame to; every process of a session may share it.
const FRAME_TIMING_ENV: &str = "CUPERTINO_FRAME_TIMING";

/// A runtime that reports how long scrolls take to reach the screen when [`SCROLL_TIMING_ENV`] names an absolute path:
/// one JSON object a line per scroll event, its stages in nanoseconds (`input`, `handled`, `updated`, `submitted`,
/// `presented`; `null` for a stage that did not happen). When [`FRAME_TIMING_ENV`] names an absolute path it appends
/// the phases of every frame there ([`frame_timing_json`]), one line per frame written whole, so the processes of a
/// session can share the file, each with the renderer `renderers` drew it with. Without the variables the runtime keeps
/// no timing.
trait Timed {
    fn timed(self, renderers: GpuRenderers) -> Self;
}

/// The file an absolute path in `var` names, opened to append; `None` (said on standard error) otherwise.
fn timing_file(var: &str) -> Option<std::fs::File> {
    let path = std::env::var_os(var).map(std::path::PathBuf::from)?;
    if !path.is_absolute() {
        eprintln!("{var}: {} is not an absolute path", path.display());
        return None;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .inspect_err(|error| eprintln!("{var}: {}: {error}", path.display()))
        .ok()
}

impl<A: Application, W: WindowSystem> Timed for Runtime<A, W> {
    fn timed(self, renderers: GpuRenderers) -> Self {
        let mut runtime = self;
        if let Some(file) = timing_file(SCROLL_TIMING_ENV) {
            let mut out = std::io::LineWriter::new(file);
            runtime = runtime.with_scroll_timing(Box::new(move |t| {
                use std::io::Write;
                let _ = writeln!(out, "{}", scroll_timing_json(&t));
            }));
        }
        if let Some(mut file) = timing_file(FRAME_TIMING_ENV) {
            let program = program_name();
            let pid = std::process::id();
            runtime = runtime.with_frame_timing(Box::new(move |t| {
                use std::io::Write;
                // one write per line: lines of other processes appending to the same file never land inside it
                let renderer = if renderers.on() { "gpu" } else { "cpu" };
                let line = format!("{}\n", frame_timing_json(t, pid, &program, renderer));
                let _ = file.write_all(line.as_bytes());
            }));
        }
        runtime
    }
}

/// This program's name and the modes it was started in (`cupertino-screenshot --serve`), as the frame timing names it
/// ([`program_label`]).
fn program_name() -> String {
    program_label(std::env::args_os())
}

/// The file name of the first of `args` and the bare `--mode` flags among the rest. A flag with a value
/// (`--path=...`) and every other argument are left out: a path, a name or a token never reaches the report.
fn program_label(args: impl IntoIterator<Item = std::ffi::OsString>) -> String {
    let mut args = args.into_iter();
    let name = args
        .next()
        .and_then(|a| {
            std::path::Path::new(&a)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "unknown".into());
    let mode = |a: &str| {
        a.strip_prefix("--").is_some_and(|flag| {
            !flag.is_empty()
                && flag
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
    };
    std::iter::once(name)
        .chain(
            args.filter_map(|a| a.into_string().ok())
                .filter(|a| mode(a)),
        )
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` as a JSON string, quotes included: quotes, backslashes and control characters escaped.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One line of the frame timing report: the process, the renderer it drew the frame with (`cpu` or `gpu`), the turn's
/// start on the monotonic clock and each phase, in nanoseconds, then the frames the turn presented.
fn frame_timing_json(
    t: &cupertino_ui_core::FrameTiming,
    pid: u32,
    program: &str,
    renderer: &str,
) -> String {
    let ns = |d: std::time::Duration| d.as_nanos();
    format!(
        r#"{{"pid":{pid},"program":{},"renderer":{},"start":{},"update":{},"view":{},"layout":{},"paint":{},"raster":{},"copy":{},"commit":{},"total":{},"frames":{}}}"#,
        json_string(program),
        json_string(renderer),
        t.start.0,
        ns(t.update),
        ns(t.view),
        ns(t.layout),
        ns(t.paint),
        ns(t.raster),
        ns(t.copy),
        ns(t.commit),
        ns(t.total),
        t.frames
    )
}

/// One line of the scroll timing report.
fn scroll_timing_json(t: &cupertino_ui_core::ScrollTiming) -> String {
    let ns = |t: Option<cupertino_ui_core::platform::Timestamp>| {
        t.map_or("null".to_string(), |t| t.0.to_string())
    };
    format!(
        r#"{{"input":{},"handled":{},"updated":{},"submitted":{},"presented":{}}}"#,
        t.input.0,
        t.handled.0,
        ns(t.updated),
        t.submitted.0,
        ns(t.presented)
    )
}

/// The family of the asset packs' fixed-pitch face, which the runtime's text system loads with the interface face;
/// `None` when no pack names one. Text that lines up by character, such as a file's contents, is set in it.
pub fn mono_family() -> Option<String> {
    Assets::discover()
        .font_family(FontRole::Mono)
        .map(str::to_owned)
}

/// The runtime options of an application on this system, in the light appearance with motion on. [`run`] starts in the
/// session's appearance and display options instead and follows them.
pub fn options() -> Result<RuntimeOptions, AppError> {
    options_and_theme(start_preferences(None)).map(|(options, _)| options)
}

/// The theme, text system and symbols an application of this system lays out with, in the light appearance with motion
/// on, for a program that measures text before it has a window. [`run`] makes its own and follows the session.
pub fn environment() -> Result<Environment, AppError> {
    let options = options()?;
    Ok(Environment {
        theme: options.theme,
        text: options.text,
        direction: options.direction,
        scale: 1.0,
        icons: options.icons,
    })
}

/// The colour the theme data names `name` (`system-green`) in `appearance`, a high contrast appearance taking its base
/// appearance's colour where it names none. `None` for a name the data does not have, and until an application of the
/// process started on the installed theme data ([`run`] and the functions beside it, [`environment`]), which every
/// instance of the process shares.
pub fn system_color(name: &str, appearance: Appearance) -> Option<Color> {
    Shared::made()?
        .theme
        .as_ref()
        .ok()?
        .tokens
        .color(appearance, name)
}

/// The theme preferences an application starts with: the session's when the window system knows them, else light with
/// motion on.
fn start_preferences(appearance: Option<SystemAppearance>) -> ThemePreferences {
    preferences(&appearance.unwrap_or_default())
}

/// The theme preferences of the session's appearance and display options.
pub fn preferences(appearance: &SystemAppearance) -> ThemePreferences {
    ThemePreferences {
        appearance: match (appearance.dark, appearance.increase_contrast) {
            (false, false) => Appearance::Light,
            (true, false) => Appearance::Dark,
            (false, true) => Appearance::LightHighContrast,
            (true, true) => Appearance::DarkHighContrast,
        },
        reduce_transparency: appearance.reduce_transparency,
        reduce_motion: appearance.reduce_motion,
    }
}

/// Keep the text caret of `tokens` from blinking when the session asks for a steady one.
fn keep_caret(tokens: &mut ThemeTokens, appearance: &SystemAppearance) {
    if appearance.steady_caret {
        tokens.motion.caret_phase_ms = 0;
    }
}

/// `follow`, with the session's appearance passed through `appearance` first.
fn mapped(
    mut follow: ThemeForAppearance,
    appearance: fn(SystemAppearance) -> SystemAppearance,
) -> ThemeForAppearance {
    Box::new(move |session| follow(&appearance(*session)))
}

/// What the person chose in `appearance.conf`, followed while the application runs: the accent and highlight colours
/// go into the theme, and the other choices are told to the toolkit ([`cupertino_ui_core::set_personal`]).
struct Personal {
    inner: Rc<PersonalInner>,
    /// The watch on the file, until [`Personal::watching`] hands it to the loop.
    watch: Option<ConfigWatcher>,
    /// What wakes the loop when the file changed.
    waker: std::task::Waker,
}

struct PersonalInner {
    theme: Option<Arc<Theme>>,
    start: Arc<ThemeTokens>,
    settings: Cell<AppearanceSettings>,
    /// The session's appearance the theme was last made for.
    last: Cell<SystemAppearance>,
    /// The file changed since it was last read.
    changed: Arc<AtomicBool>,
}

impl PersonalInner {
    /// The Cupertino theme for `appearance` from the theme data, else the neutral colours on `start`'s metrics and
    /// type, with the accent and highlight laid over them.
    fn make(&self, appearance: &SystemAppearance) -> Arc<ThemeTokens> {
        self.last.set(*appearance);
        let preferences = preferences(appearance);
        let followed = self.theme.as_ref().and_then(|theme| {
            ThemeTokens::cupertino(theme, preferences)
                .map_err(|e| {
                    eprintln!("cupertino-kit: the theme cannot follow the session ({e}); using the neutral colours");
                })
                .ok()
        });
        let mut tokens = followed.unwrap_or_else(|| {
            let mut tokens = (*self.start).clone();
            tokens.preferences = preferences;
            tokens.colors = ColorTokens::neutral(preferences.appearance);
            tokens.motion.reduce = preferences.reduce_motion;
            tokens
        });
        let settings = self.settings.get();
        // a high contrast look keeps its own colours
        if !preferences.appearance.is_high_contrast() {
            let dark = preferences.appearance.is_dark();
            let accent = settings.accent.rgb(dark);
            let highlight = match settings.highlight {
                Highlight::Automatic => accent,
                Highlight::Color(color) => color.rgb(dark),
            };
            tokens.colors = tokens.colors.with_accent(accent, highlight, dark);
        }
        keep_caret(&mut tokens, appearance);
        Arc::new(tokens)
    }
}

fn apply_personal(settings: &AppearanceSettings) {
    cupertino_ui_core::set_personal(cupertino_ui_core::Personal {
        scroll_click: settings.scroll_click,
        sidebar_icons: settings.sidebar_icons,
        titlebar_double_click: settings.titlebar_double_click,
    });
}

impl Personal {
    /// Read the file, tell the toolkit what it holds and set a watch on it, which [`Personal::watching`] hands to the
    /// loop: a change wakes the loop through `waker`. `theme` is the installed theme data, `start` the theme the
    /// application starts with and `first` the session's appearance.
    fn follow(
        waker: std::task::Waker,
        theme: Option<Arc<Theme>>,
        start: &Arc<ThemeTokens>,
        first: SystemAppearance,
    ) -> Self {
        let settings = AppearanceSettings::current();
        apply_personal(&settings);
        let watch = AppearanceSettings::path().and_then(|path| ConfigWatcher::new(vec![path]).ok());
        Personal {
            inner: Rc::new(PersonalInner {
                theme,
                start: Arc::clone(start),
                settings: Cell::new(settings),
                last: Cell::new(first),
                changed: Arc::new(AtomicBool::new(false)),
            }),
            watch,
            waker,
        }
    }

    /// The task of the loop that follows the file while the application runs, once: it never ends, and a change
    /// marks the file to be read again and wakes the loop.
    fn watching<M: Send + 'static>(&mut self) -> Option<Command<M>> {
        let watch = self.watch.take()?;
        let changed = Arc::clone(&self.inner.changed);
        let waker = self.waker.clone();
        Some(Command::Perform(Box::pin(crate::task::follow_settings(
            watch,
            move |event| {
                if matches!(event, WatchEvent::Changed(_)) {
                    changed.store(true, Ordering::Release);
                    waker.wake_by_ref();
                }
            },
        ))))
    }

    /// The theme for the appearance the application starts in.
    fn theme(&self) -> Arc<ThemeTokens> {
        self.inner.make(&self.inner.last.get())
    }

    /// Makes the Cupertino theme for each appearance the session reports.
    fn theme_for(&self) -> ThemeForAppearance {
        let inner = Rc::clone(&self.inner);
        Box::new(move |appearance| inner.make(appearance))
    }

    /// The theme again when `appearance.conf` changed since the last time.
    fn refresh(&self) -> cupertino_ui_core::PersonalRefresh {
        let inner = Rc::clone(&self.inner);
        Box::new(move || {
            if !inner.changed.swap(false, Ordering::AcqRel) {
                return None;
            }
            let settings = AppearanceSettings::current();
            inner.settings.set(settings);
            apply_personal(&settings);
            Some(inner.make(&inner.last.get()))
        })
    }
}

/// The Cupertino theme from the process's theme data (`data`, [`Shared::theme`]), else the neutral one: an application
/// starts without the desktop's data, and says why on stderr.
fn start_theme(
    preferences: ThemePreferences,
    data: &Result<Arc<Theme>, String>,
) -> Result<(ThemeTokens, Option<Arc<Theme>>), AppError> {
    let cupertino = data.clone().and_then(|data| {
        ThemeTokens::cupertino(&data, preferences)
            .map(|theme| (theme, Some(data)))
            .map_err(|e| e.to_string())
    });
    cupertino.or_else(|e| {
        eprintln!("cupertino-kit: no Cupertino theme ({e}); using the neutral theme");
        ThemeTokens::neutral(preferences)
            .map(|theme| (theme, None))
            .map_err(AppError::Theme)
    })
}

/// The calling thread runs for the instance it entered, or for the process's own when it entered none, until the
/// guard is dropped.
fn instance() -> Entered {
    Instance::current()
        .unwrap_or_else(Instance::process)
        .enter()
}

/// What every `run` here does on each turn of its loop ([`Runtime::with_each_turn`]) for the instance the calling
/// thread runs for: it reaps the programs the instance started ([`crate::spawn`]) that have ended. A runtime driven
/// otherwise, such as a test's, does the same with it.
pub fn each_turn() -> EachTurn {
    let instance = Instance::current();
    Box::new(move || {
        if let Some(instance) = &instance {
            instance.reap();
        }
    })
}

/// The windows' renderers drawn on the process's GPU while it draws there (the window system's GPU path chose it and
/// its device is ready), with the CPU renderer otherwise: before the switch, after a loss, and for a renderer the
/// device cannot make.
fn renderers_on_gpu(options: &mut RuntimeOptions, gpu: GpuRenderers) {
    let text = Arc::clone(&options.text);
    let mut cpu = std::mem::replace(
        &mut options.renderers,
        Box::new(|| unreachable!("replaced below")),
    );
    options.renderers = Box::new(move || {
        match gpu.renderer(GpuConfig::default(), GlyphRaster::new(Arc::clone(&text))) {
            Some(renderer) => Box::new(renderer),
            None => cpu(),
        }
    });
}

/// The runtime options of an instance that starts in `preferences`, with the theme data: its own theme, on the
/// process's theme data, text system and icons ([`Shared`]).
fn options_and_theme(
    preferences: ThemePreferences,
) -> Result<(RuntimeOptions, Option<Arc<Theme>>), AppError> {
    let shared = Shared::get();
    let (theme, data) = start_theme(preferences, &shared.theme)?;
    let text = Arc::clone(&shared.text);
    let glyphs = Arc::clone(&text);
    let options = RuntimeOptions {
        theme: Arc::new(theme),
        text,
        icons: Arc::<crate::icons::PackIcons>::clone(&shared.icons),
        direction: LayoutDirection::Ltr,
        node_limit: NODE_LIMIT,
        renderers: Box::new(move || {
            Box::new(CpuRenderer::new(
                CpuConfig {
                    max_pixels: 1 << 26,
                    glyph_cache_bytes: 8 << 20,
                    image_cache_bytes: 32 << 20,
                    font_smoothing: true,
                },
                GlyphRaster::new(Arc::clone(&glyphs)),
            ))
        }),
        accessibility: None,
        clock: Arc::new(MonotonicClock),
        on_window_error: Some(Box::new(|window, error| {
            eprintln!("cupertino-kit: window {window:?}: {error:?}");
        })),
    };
    Ok((options, data))
}
