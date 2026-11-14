//! Running an application on the session's Wayland display: the installed theme, the asset packs' fonts and the
//! built-in Inter, the CPU renderer and the monotonic clock.

use cupertino_assets::{Assets, FontRole, FontSource, BUILTIN_FONT};
use cupertino_defaults::appearance::{AppearanceSettings, Highlight};
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_modifiers::Profile;
use cupertino_theme::Theme;
use cupertino_ui_atspi::AtspiAdapter;
use cupertino_ui_core::{
    Application, Command, MonotonicClock, Runtime, RuntimeOptions, ThemeForAppearance, UiError,
};
use cupertino_ui_layout::LayoutDirection;
use cupertino_ui_platform::{PlatformError, SystemAppearance, WindowSystem};
use cupertino_ui_render::{CpuConfig, CpuRenderer};
use cupertino_ui_text::{
    FontIndex, FontStyle, FontWeight, GlyphRaster, InstalledFonts, TextSystem,
};
use cupertino_ui_theme::{Appearance, ColorTokens, ThemePreferences, ThemeTokens};
use cupertino_ui_wayland::WaylandWindowSystem;
use std::cell::Cell;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
/// close handler is closed, or a non-[resident](Application::RESIDENT) application closes its last window.
pub fn run<A: Application>() -> Result<(), AppError> {
    let system = WaylandWindowSystem::connect(Profile::cupertino()).map_err(AppError::Connect)?;
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
    start: (A::Model, Vec<Command<A::Message>>),
    appearance: fn(SystemAppearance) -> SystemAppearance,
) -> Result<(), AppError> {
    let first = appearance(system.appearance().unwrap_or_default());
    let (mut options, theme) = options_and_theme(preferences(&first))?;
    keep_caret(Arc::make_mut(&mut options.theme), &first);
    let personal = Personal::follow(system.waker(), theme, &options.theme, first);
    options.theme = personal.theme();
    let follow = mapped(personal.theme_for(), appearance);
    options.accessibility = Some(Box::new(AtspiAdapter::muted(system.waker())));
    Runtime::<A, _>::start(system, options, start)
        .with_theme_for(follow)
        .with_personal_refresh(personal.refresh())
        .timed()
        .run()
        .map_err(AppError::Runtime)
}

/// [`run_on`], starting from what `start` makes of the runtime's options: a model that measures text itself does so
/// with the text system the windows draw with.
pub fn run_on_with<A: Application>(
    system: WaylandWindowSystem,
    start: impl FnOnce(&RuntimeOptions) -> (A::Model, Vec<Command<A::Message>>),
) -> Result<(), AppError> {
    // the first frame already has the session's appearance
    let (mut options, theme) = options_and_theme(start_preferences(system.appearance()))?;
    let first = system.appearance().unwrap_or_default();
    keep_caret(Arc::make_mut(&mut options.theme), &first);
    let personal = Personal::follow(system.waker(), theme, &options.theme, first);
    options.theme = personal.theme();
    let follow = personal.theme_for();
    options.accessibility = Some(Box::new(AtspiAdapter::muted(system.waker())));
    let start = start(&options);
    Runtime::<A, _>::start(system, options, start)
        .with_theme_for(follow)
        .with_personal_refresh(personal.refresh())
        .timed()
        .run()
        .map_err(AppError::Runtime)
}

/// [`run_on_with`] for the application that draws the lock screen: its accessibility adapter is not muted while the
/// session is locked ([`AtspiAdapter::lock_screen`]), since what it shows then is what a person needs to hear.
pub fn run_lock_screen_on_with<A: Application>(
    system: WaylandWindowSystem,
    start: impl FnOnce(&RuntimeOptions) -> (A::Model, Vec<Command<A::Message>>),
) -> Result<(), AppError> {
    let (mut options, theme) = options_and_theme(start_preferences(system.appearance()))?;
    let first = system.appearance().unwrap_or_default();
    keep_caret(Arc::make_mut(&mut options.theme), &first);
    let personal = Personal::follow(system.waker(), theme, &options.theme, first);
    options.theme = personal.theme();
    let follow = personal.theme_for();
    options.accessibility = Some(Box::new(AtspiAdapter::lock_screen(system.waker())));
    let start = start(&options);
    Runtime::<A, _>::start(system, options, start)
        .with_theme_for(follow)
        .with_personal_refresh(personal.refresh())
        .timed()
        .run()
        .map_err(AppError::Runtime)
}

/// Names the file that [`Timed::timed`] appends the timing of each scroll to.
pub const SCROLL_TIMING_ENV: &str = "CUPERTINO_SCROLL_TIMING";

/// Names the file that [`Timed::timed`] appends the phases of each frame to; every process of a session may share it.
pub const FRAME_TIMING_ENV: &str = "CUPERTINO_FRAME_TIMING";

/// A runtime that reports how long scrolls take to reach the screen when [`SCROLL_TIMING_ENV`] names an absolute path:
/// one JSON object a line per scroll event, its stages in nanoseconds (`input`, `handled`, `updated`, `submitted`,
/// `presented`; `null` for a stage that did not happen). When [`FRAME_TIMING_ENV`] names an absolute path it appends
/// the phases of every frame there ([`frame_timing_json`]), one line per frame written whole, so the processes of a
/// session can share the file. Without the variables the runtime keeps no timing.
pub trait Timed {
    fn timed(self) -> Self;
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
    fn timed(self) -> Self {
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
                let renderer = cupertino_ui_wayland::renderer::active_name();
                let line = format!("{}\n", frame_timing_json(t, pid, &program, renderer));
                let _ = file.write_all(line.as_bytes());
            }));
        }
        runtime
    }
}

/// This program's name and the modes it was started in (`cupertino-files --desktop`), as the frame timing names it
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
pub fn frame_timing_json(
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
pub fn scroll_timing_json(t: &cupertino_ui_core::ScrollTiming) -> String {
    let ns = |t: Option<cupertino_ui_platform::Timestamp>| {
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

/// The theme preferences an application starts with: the session's when the window system knows them, else light with
/// motion on.
pub fn start_preferences(appearance: Option<SystemAppearance>) -> ThemePreferences {
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
    /// Keeps the watch on the file while the application runs.
    _watch: Option<ConfigWatcher>,
}

struct PersonalInner {
    theme: Option<Theme>,
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
    /// Read the file, tell the toolkit what it holds and watch it: a change wakes the loop through `waker`. `theme` is
    /// the installed theme data, `start` the theme the application starts with and `first` the session's appearance.
    fn follow(
        waker: std::task::Waker,
        theme: Option<Theme>,
        start: &Arc<ThemeTokens>,
        first: SystemAppearance,
    ) -> Self {
        let settings = AppearanceSettings::current();
        apply_personal(&settings);
        let changed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&changed);
        let watch = AppearanceSettings::path().and_then(|path| {
            ConfigWatcher::new(vec![path], move |event| {
                if matches!(event, WatchEvent::Changed(_)) {
                    flag.store(true, Ordering::Release);
                    waker.wake_by_ref();
                }
            })
            .ok()
        });
        Personal {
            inner: Rc::new(PersonalInner {
                theme,
                start: Arc::clone(start),
                settings: Cell::new(settings),
                last: Cell::new(first),
                changed,
            }),
            _watch: watch,
        }
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

/// The Cupertino theme from the installed theme data, else the neutral one: an application starts without the
/// desktop's data, and says why on stderr.
fn start_theme(preferences: ThemePreferences) -> Result<(ThemeTokens, Option<Theme>), AppError> {
    let cupertino = Theme::load().and_then(|data| {
        ThemeTokens::cupertino(&data, preferences).map(|theme| (theme, Some(data)))
    });
    cupertino.or_else(|e| {
        eprintln!("cupertino-kit: no Cupertino theme ({e}); using the neutral theme");
        ThemeTokens::neutral(preferences)
            .map(|theme| (theme, None))
            .map_err(AppError::Theme)
    })
}

fn options_and_theme(
    preferences: ThemePreferences,
) -> Result<(RuntimeOptions, Option<Theme>), AppError> {
    let (theme, data) = start_theme(preferences)?;
    let assets = Assets::discover();
    let (fonts, builtin) = fonts(&assets);
    let preferred = [
        assets.font_family(FontRole::Ui),
        Some(&*theme.type_scale.family),
    ];
    let family = ui_family(&fonts, preferred.into_iter().flatten(), builtin.as_deref());
    let text = Arc::new(TextSystem::new(fonts, &family));
    let glyphs = Arc::clone(&text);
    let options = RuntimeOptions {
        theme: Arc::new(theme),
        text,
        icons: Arc::new(crate::icons::PackIcons::new(&assets)),
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

/// The largest font file read; the biggest real collections are a few tens of megabytes.
const MAX_FONT_BYTES: u64 = 64 << 20;

/// A regular file of at most [`MAX_FONT_BYTES`], so a pack entry that names a device, a pipe or a huge file never
/// stalls or exhausts an application at start.
fn read_font(path: &Path) -> io::Result<Vec<u8>> {
    let too_big = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "not a regular file of at most 64 MiB",
        )
    };
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() || meta.len() > MAX_FONT_BYTES {
        return Err(too_big());
    }
    let file = File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > MAX_FONT_BYTES {
        return Err(too_big());
    }
    let mut data = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_FONT_BYTES + 1).read_to_end(&mut data)?;
    if data.len() as u64 > MAX_FONT_BYTES {
        return Err(too_big());
    }
    Ok(data)
}

/// The first of `preferred` that names a loaded face, else `builtin`: UI text never falls to whichever face happened
/// to load first.
fn ui_family<'a>(
    fonts: &FontIndex,
    mut preferred: impl Iterator<Item = &'a str>,
    builtin: Option<&'a str>,
) -> String {
    preferred
        .find(|family| {
            fonts
                .match_face(family, FontWeight::REGULAR, FontStyle::Normal)
                .is_some()
        })
        .or(builtin)
        .unwrap_or_default()
        .to_owned()
}

/// The asset packs' fonts, then the built-in Inter, with the family Inter registered under, and the installed fonts
/// for the characters and families none of them has. A file that cannot be read or parsed is skipped with a message.
fn fonts(assets: &Assets) -> (FontIndex, Option<String>) {
    let mut index = FontIndex::new();
    let mut builtin = None;
    for source in assets.fonts() {
        let data = match &source {
            FontSource::File(path) => match read_font(path) {
                Ok(data) => data,
                Err(e) => {
                    eprintln!("cupertino-kit: skipping the font {}: {e}", path.display());
                    continue;
                }
            },
            FontSource::Builtin => BUILTIN_FONT.to_vec(),
        };
        match index.add_font(Arc::new(data)) {
            Ok(faces) if matches!(source, FontSource::Builtin) => {
                builtin = faces
                    .first()
                    .and_then(|&face| index.family(face))
                    .map(str::to_owned);
            }
            Ok(_) => {}
            Err(e) => eprintln!("cupertino-kit: skipping the font {}: {e:?}", source.label()),
        }
    }
    if let Some(installed) = InstalledFonts::new() {
        index.set_system_fonts(Box::new(installed));
    }
    (index, builtin)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What an application's window follows: the theme for each appearance the session reports, from `theme` (the
    /// installed data) or else `start`, with the default personal settings.
    fn theme_for(theme: Option<Theme>, start: Arc<ThemeTokens>) -> ThemeForAppearance {
        let inner = Rc::new(PersonalInner {
            theme,
            start,
            settings: Cell::new(AppearanceSettings::UNSET),
            last: Cell::new(SystemAppearance::default()),
            changed: Arc::new(AtomicBool::new(false)),
        });
        Box::new(move |appearance| inner.make(appearance))
    }

    #[test]
    fn a_frame_timing_line_names_the_process_and_every_phase_in_nanoseconds() {
        use std::time::Duration;
        let t = cupertino_ui_core::FrameTiming {
            start: cupertino_ui_platform::Timestamp(1_000),
            update: Duration::from_nanos(1),
            view: Duration::from_nanos(2),
            layout: Duration::from_nanos(3),
            paint: Duration::from_nanos(4),
            raster: Duration::from_nanos(5),
            copy: Duration::from_nanos(6),
            commit: Duration::from_nanos(7),
            total: Duration::from_nanos(30),
            frames: 2,
        };
        assert_eq!(
            frame_timing_json(&t, 42, "cupertino-dock", "cpu"),
            r#"{"pid":42,"program":"cupertino-dock","renderer":"cpu","start":1000,"update":1,"view":2,"layout":3,"paint":4,"raster":5,"copy":6,"commit":7,"total":30,"frames":2}"#
        );
    }

    #[test]
    fn a_program_name_with_quotes_or_control_characters_stays_one_json_line() {
        let t = cupertino_ui_core::FrameTiming {
            start: cupertino_ui_platform::Timestamp(1),
            update: std::time::Duration::ZERO,
            view: std::time::Duration::ZERO,
            layout: std::time::Duration::ZERO,
            paint: std::time::Duration::ZERO,
            raster: std::time::Duration::ZERO,
            copy: std::time::Duration::ZERO,
            commit: std::time::Duration::ZERO,
            total: std::time::Duration::ZERO,
            frames: 1,
        };
        let name = "odd \"name\"\\\n\r\t\u{1}\u{1f}\u{7f} é";
        let line = frame_timing_json(&t, 7, name, "gpu");
        assert!(
            !line.contains(['\n', '\r', '\t', '\u{1}', '\u{1f}', '\u{7f}']),
            "{line}"
        );
        assert!(
            line.contains(r#""program":"odd \"name\"\\\n\r\t\u0001\u001f\u007f é","#),
            "{line}"
        );
    }

    #[test]
    fn the_program_label_keeps_the_file_name_and_bare_mode_flags_alone() {
        let label = |args: &[&str]| program_label(args.iter().map(std::ffi::OsString::from));
        assert_eq!(
            label(&["/usr/bin/cupertino-files", "--desktop"]),
            "cupertino-files --desktop"
        );
        assert_eq!(
            label(&[
                "./target/release/cupertino-files",
                "--open=/home/someone/Private/taxes.pdf",
                "--token",
                "s3cret",
                "/home/someone",
                "--Name=X",
                "--two-panes",
                "--",
            ]),
            "cupertino-files --token --two-panes",
            "no value, path or argument that is not a bare flag"
        );
        assert_eq!(label(&[]), "unknown");
        assert_eq!(label(&["/"]), "unknown");
    }

    #[test]
    fn a_scroll_timing_line_is_one_json_object_with_null_for_a_missing_stage() {
        use cupertino_ui_platform::{Timestamp, WindowId};
        let t = cupertino_ui_core::ScrollTiming {
            window: WindowId(1),
            input: Timestamp(100),
            handled: Timestamp(150),
            updated: None,
            submitted: Timestamp(200),
            presented: Some(Timestamp(400)),
        };
        let line = scroll_timing_json(&t);
        assert_eq!(
            line,
            r#"{"input":100,"handled":150,"updated":null,"submitted":200,"presented":400}"#
        );
    }

    #[test]
    fn the_session_appearance_picks_the_theme_appearance_and_its_preferences() {
        let session =
            |dark, increase_contrast, reduce_transparency, reduce_motion| SystemAppearance {
                dark,
                increase_contrast,
                reduce_transparency,
                reduce_motion,
                steady_caret: false,
            };
        assert_eq!(
            preferences(&session(false, false, false, false)).appearance,
            Appearance::Light
        );
        assert_eq!(
            preferences(&session(true, false, false, false)).appearance,
            Appearance::Dark
        );
        assert_eq!(
            preferences(&session(false, true, false, false)).appearance,
            Appearance::LightHighContrast
        );
        let all = preferences(&session(true, true, true, true));
        assert_eq!(all.appearance, Appearance::DarkHighContrast);
        assert!(all.reduce_transparency && all.reduce_motion);
    }

    #[test]
    fn an_application_starts_in_the_session_appearance_so_its_first_frame_has_it() {
        assert_eq!(
            start_preferences(None),
            preferences(&SystemAppearance::default())
        );
        assert_eq!(start_preferences(None).appearance, Appearance::Light);
        let dark = SystemAppearance {
            dark: true,
            reduce_transparency: true,
            ..SystemAppearance::default()
        };
        let (options, theme) = options_and_theme(start_preferences(Some(dark))).unwrap();
        assert_eq!(
            *options.theme,
            ThemeTokens::cupertino(&theme.unwrap(), preferences(&dark)).unwrap(),
            "every window opens with this theme"
        );
        assert_eq!(options.theme.preferences.appearance, Appearance::Dark);
    }

    #[test]
    fn following_the_session_makes_its_cupertino_theme() {
        let theme = Theme::load().unwrap();
        let start = Arc::new(
            ThemeTokens::cupertino(&theme, preferences(&SystemAppearance::default())).unwrap(),
        );
        let mut follow = theme_for(Some(theme), start);
        let dark = SystemAppearance {
            dark: true,
            reduce_motion: true,
            ..SystemAppearance::default()
        };
        let tokens = follow(&dark);
        // the theme for the session's appearance, with the person's accent (the default one, nothing being set) over it
        let mut expected =
            ThemeTokens::cupertino(&Theme::load().unwrap(), preferences(&dark)).unwrap();
        let accent = AppearanceSettings::UNSET.accent.rgb(true);
        expected.colors = expected.colors.with_accent(accent, accent, true);
        assert_eq!(*tokens, expected);
        assert!(tokens.motion.reduce);
    }

    #[test]
    fn a_surface_can_keep_the_dark_appearance_and_follow_the_display_options() {
        let dark = |session: SystemAppearance| SystemAppearance {
            dark: true,
            ..session
        };
        let start =
            Arc::new(ThemeTokens::neutral(preferences(&SystemAppearance::default())).unwrap());
        let mut follow = mapped(theme_for(None, start), dark);
        let light = follow(&SystemAppearance {
            reduce_transparency: true,
            ..SystemAppearance::default()
        });
        assert_eq!(light.preferences.appearance, Appearance::Dark);
        assert!(light.preferences.reduce_transparency);
    }

    #[test]
    fn without_theme_data_following_the_session_makes_its_neutral_theme() {
        let start =
            Arc::new(ThemeTokens::neutral(preferences(&SystemAppearance::default())).unwrap());
        let mut follow = theme_for(None, start);
        let dark = SystemAppearance {
            dark: true,
            increase_contrast: true,
            reduce_motion: true,
            ..SystemAppearance::default()
        };
        assert_eq!(
            *follow(&dark),
            ThemeTokens::neutral(preferences(&dark)).unwrap()
        );
    }

    #[test]
    fn a_session_that_keeps_the_caret_steady_gets_a_caret_that_never_blinks() {
        let start =
            Arc::new(ThemeTokens::neutral(preferences(&SystemAppearance::default())).unwrap());
        let mut follow = theme_for(None, Arc::clone(&start));
        assert_eq!(
            follow(&SystemAppearance::default()).motion.caret_phase_ms,
            start.motion.caret_phase_ms
        );
        assert!(start.motion.caret_phase_ms > 0);
        let steady = SystemAppearance {
            steady_caret: true,
            ..SystemAppearance::default()
        };
        assert_eq!(follow(&steady).motion.caret_phase_ms, 0);
        let mut follow = theme_for(Some(Theme::load().unwrap()), start);
        let tokens = follow(&steady);
        assert_eq!(tokens.motion.caret_phase_ms, 0);
        assert!(!tokens.motion.reduce, "the rest of motion stays as it is");
    }

    #[test]
    fn only_a_regular_font_file_of_bounded_size_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let font = dir.path().join("a.ttf");
        std::fs::write(&font, b"font").unwrap();
        assert_eq!(read_font(&font).unwrap(), b"font");

        let huge = dir.path().join("huge.ttf");
        File::create(&huge)
            .unwrap()
            .set_len(MAX_FONT_BYTES + 1)
            .unwrap();
        assert!(read_font(&huge).is_err());

        let device = dir.path().join("zero.ttf");
        std::os::unix::fs::symlink("/dev/zero", &device).unwrap();
        assert!(read_font(&device).is_err());
        assert!(read_font(dir.path()).is_err());
    }

    #[test]
    fn ui_text_takes_a_preferred_family_that_is_loaded_else_the_built_in_one() {
        // a pack's face registered first, as Assets::fonts orders them, then the built-in Inter
        let mut fonts = FontIndex::new();
        let data = Arc::new(BUILTIN_FONT.to_vec());
        let pack = fonts
            .add_face(
                "Pack Sans",
                FontWeight::REGULAR,
                FontStyle::Normal,
                data.clone(),
                0,
            )
            .unwrap();
        let inter = fonts.add_font(data).unwrap()[0];
        let builtin = fonts.family(inter).unwrap().to_owned();
        let face = |family: &str| fonts.match_face(family, FontWeight::REGULAR, FontStyle::Normal);

        let family = ui_family(
            &fonts,
            ["Inter", "Missing Sans"].into_iter(),
            Some(&builtin),
        );
        assert_eq!(
            face(&family),
            Some(inter),
            "never the first registered face"
        );
        assert_ne!(face(&family), Some(pack));

        let family = ui_family(
            &fonts,
            ["Missing Sans", "pack sans"].into_iter(),
            Some(&builtin),
        );
        assert_eq!(
            face(&family),
            Some(pack),
            "a preferred family that is loaded wins"
        );
    }
}
