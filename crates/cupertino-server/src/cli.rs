//! Command line interface and startup of the compositor.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use clap::{ArgGroup, Parser};
use cupertino_assets::Assets;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

use crate::config::{BuiltIn, Config, PORTAL_BUS_NAME};
use crate::input::{translate_action, Action};
use crate::state::Cupertino;

#[derive(Parser, Debug, Clone, Default)]
#[command(
    name = "cupertino-server",
    about = "Smithay-based compositor of the Cupertino prototypes",
    version
)]
#[command(group(ArgGroup::new("backend").args(["winit", "udev", "headless"])))]
pub struct Cli {
    /// Run nested inside a window of the current session (development).
    #[arg(long)]
    pub winit: bool,
    /// Run on a TTY with DRM/KMS and a logind session.
    #[arg(long)]
    pub udev: bool,
    /// Run without a window system (offscreen EGL device).
    #[arg(long)]
    pub headless: bool,
    /// Print what the TTY backend needs from this machine (GPU driver and its settings) and exit; opens no device.
    #[arg(long)]
    pub check_hardware: bool,
    /// Development and nested runs: let any configuration file name the shell processes and the screenshot process, from
    /// any path. Without it only the system file, programs under /usr, and the person's file removing or reordering them
    /// apply.
    #[arg(long)]
    pub dev_shell: bool,
    /// Development-only native controller, supervised on its private Wayland connection.
    #[arg(long, requires = "dev_shell", value_name = "PATH")]
    pub control_program: Option<PathBuf>,
    /// Output size in pixels, e.g. 1280x800 (nested window / headless output).
    #[arg(long, value_parser = parse_size, default_value = "1280x800")]
    pub size: (u32, u32),
    /// Output scale (default: the host scale for the nested window, 1 otherwise; with `--physical-size` the
    /// headless output's default scale).
    #[arg(long)]
    pub scale: Option<f64>,
    /// Physical size of the headless output in millimetres, e.g. 302x196 (default: unknown). With it the output has a
    /// density, and so "looks like" choices and a default scale of its own.
    #[arg(long, value_parser = parse_size)]
    pub physical_size: Option<(u32, u32)>,
    /// Connector name of the headless output (default `headless-1`); `eDP-1` stands for the machine's own panel.
    #[arg(long)]
    pub connector: Option<String>,
    /// Headless: plug in one more offscreen display right of the others, `NAME:WIDTHxHEIGHT[@SCALE]` (pixels; scale 1
    /// without one); repeatable. With more than one display each screenshot is written for every display: the first
    /// one's at the path given, every other one's beside it with its name added (`shot-headless-2.png`).
    #[arg(long = "extra-output", value_name = "NAME:WxH[@SCALE]", value_parser = parse_extra_output, requires = "headless")]
    pub extra_outputs: Vec<(String, (u32, u32), f64)>,
    /// Headless: unplug the display with this connector name at a time, `SECONDS:NAME`; repeatable.
    #[arg(long = "unplug", value_name = "SECONDS:NAME", value_parser = parse_timed_text, requires = "headless")]
    pub unplugs: Vec<(f64, String)>,
    /// Appearance: light or dark.
    #[arg(long)]
    pub appearance: Option<String>,
    /// Write a PNG of the output after `--screenshot-after` seconds.
    #[arg(long, value_name = "PATH")]
    pub screenshot: Option<PathBuf>,
    /// Seconds after start at which the screenshot is taken.
    #[arg(long, default_value_t = 3.0)]
    pub screenshot_after: f64,
    /// Exit once the screenshot was written.
    #[arg(long)]
    pub exit_after_screenshot: bool,
    /// Exit after this many seconds.
    #[arg(long, value_name = "SECONDS")]
    pub run_for: Option<f64>,
    /// Headless: write the output as raw RGBA frames, 60 per second of wall time, to this file (a FIFO a video encoder
    /// reads, say) until the server exits.
    #[arg(long, value_name = "PATH", requires = "headless")]
    pub record: Option<PathBuf>,
    /// Seconds after start at which `--record` takes its first frame.
    #[arg(
        long,
        value_name = "SECONDS",
        default_value_t = 0.0,
        requires = "record"
    )]
    pub record_after: f64,
    /// Start a client (command line) after startup; repeatable.
    #[arg(long = "spawn", value_name = "COMMAND")]
    pub spawn: Vec<String>,
    /// Seconds to wait before the first --spawn command.
    #[arg(long, default_value_t = 0.5)]
    pub spawn_delay: f64,
    /// Run a named action at a time, `SECONDS:ACTION` (for example `2.5:overview`); repeatable.
    #[arg(long = "action", value_name = "SECONDS:ACTION", value_parser = parse_timed_action)]
    pub actions: Vec<(f64, Action)>,
    /// Move the pointer at a time, `SECONDS:X,Y` (logical coordinates); repeatable.
    #[arg(long = "pointer", value_name = "SECONDS:X,Y", value_parser = parse_timed_point)]
    pub pointers: Vec<(f64, (f64, f64))>,
    /// Click (press and release) the left button at a time and position, `SECONDS:X,Y`; repeatable.
    #[arg(long = "click", value_name = "SECONDS:X,Y", value_parser = parse_timed_point)]
    pub clicks: Vec<(f64, (f64, f64))>,
    /// Click (press and release) the right button at a time and position, `SECONDS:X,Y`; repeatable.
    #[arg(long = "right-click", value_name = "SECONDS:X,Y", value_parser = parse_timed_point)]
    pub right_clicks: Vec<(f64, (f64, f64))>,
    /// Press the left button at a time and position, `SECONDS:X,Y`; repeatable. With `--pointer` and `--release` it scripts a drag.
    #[arg(long = "press", value_name = "SECONDS:X,Y", value_parser = parse_timed_point)]
    pub presses: Vec<(f64, (f64, f64))>,
    /// Scroll as two fingers on a touchpad would at a time, position and distance, `SECONDS:X,Y:DY` (logical points,
    /// positive down); repeatable. A distance of 0 ends the gesture.
    #[arg(long = "scroll", value_name = "SECONDS:X,Y:DY", value_parser = parse_timed_scroll)]
    pub scrolls: Vec<(f64, (f64, f64), f64)>,
    /// Headless: the refresh rate of the display, in Hz (default 60).
    #[arg(long, value_name = "HZ", requires = "headless")]
    pub refresh: Option<f64>,
    /// Headless: write one line of timings per frame shown to this file (`scripts/dev/bench` reads it). The GPU is
    /// waited for after each frame and around each glass pass, so frames take as long as their work.
    #[arg(long, value_name = "PATH", requires = "headless")]
    pub frame_stats: Option<PathBuf>,
    /// Release the left button at a time and position, `SECONDS:X,Y`; repeatable.
    #[arg(long = "release", value_name = "SECONDS:X,Y", value_parser = parse_timed_point)]
    pub releases: Vec<(f64, (f64, f64))>,
    /// Press a modifier key (`shift`, `ctrl`, `alt`, `super`) and keep it down at a time, `SECONDS:MODIFIER`; repeatable.
    #[arg(long = "modifier-down", value_name = "SECONDS:MODIFIER", value_parser = parse_timed_text)]
    pub modifier_downs: Vec<(f64, String)>,
    /// Release a modifier key at a time, `SECONDS:MODIFIER`; repeatable.
    #[arg(long = "modifier-up", value_name = "SECONDS:MODIFIER", value_parser = parse_timed_text)]
    pub modifier_ups: Vec<(f64, String)>,
    /// Take a screenshot at a time, `SECONDS:PATH`; repeatable.
    #[arg(long = "shot", value_name = "SECONDS:PATH", value_parser = parse_timed_shot)]
    pub shots: Vec<(f64, PathBuf)>,
    /// Headless: write the windows at a time, `SECONDS:PATH`, as JSON, one object a line: the application, the title
    /// and the frame (title bar included) in logical points (`scripts/dev/bench` places its pointer paths by them);
    /// repeatable.
    #[arg(long = "window-list", value_name = "SECONDS:PATH", value_parser = parse_timed_shot, requires = "headless")]
    pub window_lists: Vec<(f64, PathBuf)>,
    /// Do not draw the menu bar.
    #[arg(long)]
    pub no_menu_bar: bool,
    /// Run model time (animations) at this multiple of the wall clock, e.g. 0.25 for slow motion.
    #[arg(long, value_name = "FACTOR")]
    pub time_scale: Option<f64>,
    /// Start the session bus worker even for the headless backend (menu registrar, status items).
    #[arg(long)]
    pub dbus: bool,
    /// End the server as a crash would at a time, `SECONDS:panic`, `SECONDS:abort` or `SECONDS:overflow` (of the stack),
    /// to test what a crash leaves behind.
    #[arg(long, value_name = "SECONDS:HOW", value_parser = parse_timed_crash)]
    pub crash: Option<(f64, Crash)>,
    /// Press a key chord at a time (`SECONDS:CHORD`, for example `1:Super+space`, `2:Return`); repeatable.
    #[arg(long = "key", value_name = "SECONDS:CHORD", value_parser = parse_timed_text)]
    pub keys: Vec<(f64, String)>,
    /// Type text at a time (`SECONDS:TEXT`); repeatable.
    #[arg(long = "type", value_name = "SECONDS:TEXT", value_parser = parse_timed_text)]
    pub typed: Vec<(f64, String)>,
    /// Open the accent picker for a character at a time (`SECONDS:CHAR`), as a held key would after its repeat delay.
    #[arg(long = "accent", value_name = "SECONDS:CHAR", value_parser = parse_timed_text)]
    pub accents: Vec<(f64, String)>,
    /// Scripted shortcuts behave as if their modifiers were still held (the input-source HUD stays open).
    #[arg(long)]
    pub hold_modifiers: bool,
    /// Once the Wayland socket is bound, write `WAYLAND_DISPLAY=<name>` and a line end to this open file descriptor and
    /// close it (`cupertino desktop start` waits on it).
    #[arg(long, value_name = "FD")]
    pub ready_fd: Option<i32>,
}

pub fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s.split_once('x').ok_or("expected WIDTHxHEIGHT")?;
    let (w, h) = (
        w.parse::<u32>().map_err(|e| e.to_string())?,
        h.parse::<u32>().map_err(|e| e.to_string())?,
    );
    if w == 0 || h == 0 {
        return Err("size must be positive".into());
    }
    Ok((w, h))
}

/// `NAME:WIDTHxHEIGHT[@SCALE]` of `--extra-output`.
pub fn parse_extra_output(s: &str) -> Result<(String, (u32, u32), f64), String> {
    let (name, rest) = s
        .split_once(':')
        .ok_or("expected NAME:WIDTHxHEIGHT[@SCALE]")?;
    if name.is_empty() {
        return Err("the display needs a name".into());
    }
    let (size, scale) = match rest.split_once('@') {
        Some((size, scale)) => (size, scale.parse::<f64>().map_err(|e| e.to_string())?),
        None => (rest, 1.0),
    };
    if !(scale.is_finite() && scale > 0.0) {
        return Err("the scale must be above zero".into());
    }
    Ok((name.to_owned(), parse_size(size)?, scale))
}

pub fn parse_timed_shot(s: &str) -> Result<(f64, PathBuf), String> {
    let (t, p) = s.split_once(':').ok_or("expected SECONDS:PATH")?;
    Ok((
        t.parse::<f64>().map_err(|e| e.to_string())?,
        PathBuf::from(p),
    ))
}

pub fn parse_timed_point(s: &str) -> Result<(f64, (f64, f64)), String> {
    let (t, p) = s.split_once(':').ok_or("expected SECONDS:X,Y")?;
    let (x, y) = p.split_once(',').ok_or("expected SECONDS:X,Y")?;
    let n = |v: &str| v.trim().parse::<f64>().map_err(|e| e.to_string());
    Ok((n(t)?, (n(x)?, n(y)?)))
}

pub fn parse_timed_scroll(s: &str) -> Result<(f64, (f64, f64), f64), String> {
    let (point, dy) = s.rsplit_once(':').ok_or("expected SECONDS:X,Y:DY")?;
    let (t, p) = parse_timed_point(point)?;
    Ok((t, p, dy.trim().parse::<f64>().map_err(|e| e.to_string())?))
}

pub fn parse_timed_text(s: &str) -> Result<(f64, String), String> {
    let (t, rest) = s.split_once(':').ok_or("expected SECONDS:TEXT")?;
    Ok((
        t.parse::<f64>().map_err(|e| e.to_string())?,
        rest.to_string(),
    ))
}

/// How `--crash` ends the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Crash {
    /// A panic on the event loop's thread.
    Panic,
    /// `abort`, which raises SIGABRT.
    Abort,
    /// A stack overflow, which faults with SIGSEGV.
    Overflow,
}

pub fn parse_timed_crash(s: &str) -> Result<(f64, Crash), String> {
    let (t, how) = s
        .split_once(':')
        .ok_or("expected SECONDS:panic, SECONDS:abort or SECONDS:overflow")?;
    let crash = match how {
        "panic" => Crash::Panic,
        "abort" => Crash::Abort,
        "overflow" => Crash::Overflow,
        _ => {
            return Err(format!(
                "unknown crash {how:?}: expected panic, abort or overflow"
            ))
        }
    };
    Ok((t.parse::<f64>().map_err(|e| e.to_string())?, crash))
}

pub fn parse_timed_action(s: &str) -> Result<(f64, Action), String> {
    let (t, a) = s.split_once(':').ok_or("expected SECONDS:ACTION")?;
    let t = t.parse::<f64>().map_err(|e| e.to_string())?;
    let action = Action::from_name(a).ok_or_else(|| format!("unknown action {a:?}"))?;
    Ok((t, action))
}

/// Backend selected by the flags (nested window when none is given and a session exists).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Winit,
    Udev,
    Headless,
}

impl Cli {
    pub fn mode(&self) -> Result<Mode> {
        match (self.winit, self.udev, self.headless) {
            (true, false, false) => Ok(Mode::Winit),
            (false, true, false) => Ok(Mode::Udev),
            (false, false, true) => Ok(Mode::Headless),
            (false, false, false) => {
                if std::env::var_os("WAYLAND_DISPLAY").is_some()
                    || std::env::var_os("DISPLAY").is_some()
                {
                    Ok(Mode::Winit)
                } else {
                    Ok(Mode::Udev)
                }
            }
            _ => Err(anyhow!("choose one of --winit, --udev, --headless")),
        }
    }

    /// The configuration of the settings files and the command line, and what the files held.
    pub fn config(&self) -> Result<(Config, crate::config::SettingsFiles)> {
        let (mut cfg, files) = Config::load_with_files(self.dev_shell);
        if let Some(a) = &self.appearance {
            cfg.appearance = a.parse().map_err(|e: String| anyhow!(e))?;
        }
        if self.no_menu_bar {
            cfg.shell.show_menu_bar = false;
        }
        Ok((cfg, files))
    }
}

fn schedule(
    event_loop: &EventLoop<'static, Cupertino>,
    after: f64,
    f: impl FnOnce(&mut Cupertino) + 'static,
) -> Result<()> {
    let mut f = Some(f);
    let timer = Timer::from_duration(Duration::from_secs_f64(after.max(0.0)));
    event_loop
        .handle()
        .insert_source(timer, move |_, _, state| {
            if let Some(f) = f.take() {
                f(state);
            }
            TimeoutAction::Drop
        })
        .map_err(|e| anyhow::anyhow!("inserting a scripted step timer: {e}"))?;
    Ok(())
}

#[allow(unconditional_recursion)]
fn overflow(depth: u64) -> u64 {
    let frame = [depth; 512];
    std::hint::black_box(&frame);
    overflow(depth + 1) + std::hint::black_box(frame[0])
}

fn schedule_script(cli: &Cli, event_loop: &EventLoop<'static, Cupertino>) -> Result<()> {
    for (i, cmd) in cli.spawn.iter().enumerate() {
        let cmd = cmd.clone();
        schedule(event_loop, cli.spawn_delay + 0.3 * i as f64, move |state| {
            let parts: Vec<String> = cmd.split_whitespace().map(String::from).collect();
            if let Some((prog, args)) = parts.split_first() {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                state.spawn(prog, &args);
            }
        })?;
    }
    for (t, action) in cli.actions.iter().copied() {
        schedule(event_loop, t, move |state| {
            crate::frame_stats::note_input(state);
            let ctx = state.action_context();
            let plan = translate_action(action, &ctx);
            state.execute_plan(plan);
        })?;
    }
    if let Some((t, crash)) = cli.crash {
        schedule(event_loop, t, move |_| match crash {
            Crash::Panic => panic!("the crash asked for with --crash"),
            Crash::Abort => std::process::abort(),
            Crash::Overflow => {
                std::hint::black_box(overflow(0));
            }
        })?;
    }
    for (t, chord) in cli.keys.iter().cloned() {
        schedule(event_loop, t, move |state| {
            if let Err(e) = state.scripted_key(&chord) {
                tracing::warn!(chord, error = %e, "scripted key ignored");
            }
        })?;
    }
    for (t, text) in cli.typed.iter().cloned() {
        schedule(event_loop, t, move |state| state.scripted_type(&text))?;
    }
    for (t, text) in cli.accents.iter().cloned() {
        schedule(event_loop, t, move |state| {
            if let Some(c) = text.chars().next() {
                if !state.accent_open(c) {
                    tracing::warn!(base = %c, "no accent candidates (set accent_table in keyboard.conf)");
                }
            }
        })?;
    }
    for (t, (x, y)) in cli.pointers.iter().copied() {
        schedule(event_loop, t, move |state| {
            state.pointer_moved_to((x, y).into(), input_millis(state), None);
        })?;
    }
    for (t, (x, y)) in cli.clicks.iter().copied() {
        schedule(event_loop, t, move |state| {
            state.scripted_click((x, y), crate::events::BTN_LEFT);
        })?;
    }
    for (t, (x, y)) in cli.right_clicks.iter().copied() {
        schedule(event_loop, t, move |state| {
            state.scripted_click((x, y), crate::events::BTN_RIGHT);
        })?;
    }
    for (t, name) in cli.modifier_downs.iter().cloned() {
        schedule(event_loop, t, move |state| {
            if let Err(e) = state.scripted_modifier(&name, true) {
                tracing::warn!(modifier = name, error = %e, "scripted modifier ignored");
            }
        })?;
    }
    for (t, name) in cli.modifier_ups.iter().cloned() {
        schedule(event_loop, t, move |state| {
            if let Err(e) = state.scripted_modifier(&name, false) {
                tracing::warn!(modifier = name, error = %e, "scripted modifier ignored");
            }
        })?;
    }
    for (t, (x, y)) in cli.presses.iter().copied() {
        schedule(event_loop, t, move |state| {
            state.pointer_moved_to((x, y).into(), input_millis(state), None);
            state.pointer_button(
                crate::events::BTN_LEFT,
                smithay::backend::input::ButtonState::Pressed,
                input_millis(state),
            );
        })?;
    }
    for (t, (x, y)) in cli.releases.iter().copied() {
        schedule(event_loop, t, move |state| {
            state.pointer_moved_to((x, y).into(), input_millis(state), None);
            state.pointer_button(
                crate::events::BTN_LEFT,
                smithay::backend::input::ButtonState::Released,
                input_millis(state),
            );
        })?;
    }
    for (t, (x, y), dy) in cli.scrolls.iter().copied() {
        schedule(event_loop, t, move |state| {
            state.pointer_moved_to((x, y).into(), input_millis(state), None);
            let usec = Duration::from(state.clock.now()).as_micros() as u64;
            state.pointer_axis(&scripted_scroll(dy, usec));
        })?;
    }
    for (t, name) in cli.unplugs.iter().cloned() {
        schedule(event_loop, t, move |state| {
            let head = state
                .displays
                .heads
                .iter()
                .find(|h| h.info.connector == name)
                .map(|h| h.id);
            match head {
                Some(head) => {
                    if let Err(e) = crate::backend::headless::remove_display(state, head) {
                        tracing::warn!(error = %format!("{e:#}"), %name, "unplugging the display failed");
                    }
                }
                None => tracing::warn!(%name, "no display to unplug"),
            }
        })?;
    }
    for (t, path) in cli.window_lists.iter().cloned() {
        schedule(event_loop, t, move |state| {
            if let Err(e) = std::fs::write(&path, window_list(state)) {
                tracing::warn!(error = %e, path = %path.display(), "writing the window list failed");
            }
        })?;
    }
    for (t, path) in cli.shots.iter().cloned() {
        schedule(event_loop, t, move |state| {
            state.screenshot_requests.push(path);
            state.request_redraw();
        })?;
    }
    if let Some(path) = cli.screenshot.clone() {
        let exit = cli.exit_after_screenshot;
        schedule(event_loop, cli.screenshot_after, move |state| {
            state.screenshot_requests.push(path);
            state.exit_after_screenshot = exit;
            state.request_redraw();
        })?;
    }
    Ok(())
}

/// The milliseconds a scripted pointer event is stamped with: the model clock's, as a device's would be, so clients
/// see the time between scripted clicks and do not take two at one spot seconds apart for a double click.
fn input_millis(state: &Cupertino) -> u32 {
    (state.now() * 1000.0) as u32
}

impl Cupertino {
    /// A scripted click of `button` (an evdev code) at `at`, in global logical points: the pointer goes there, and
    /// the button is pressed and released, stamped as [`input_millis`] says.
    pub fn scripted_click(&mut self, at: (f64, f64), button: u32) {
        use smithay::backend::input::ButtonState;
        self.pointer_moved_to(at.into(), input_millis(self), None);
        self.pointer_button(button, ButtonState::Pressed, input_millis(self));
        self.pointer_button(button, ButtonState::Released, input_millis(self));
    }
}

/// A scripted two-finger scroll of `dy` points at `usec`: the horizontal axis carries nothing (a zero there would end
/// the gesture on it at every step), and a `dy` of zero ends the scroll.
fn scripted_scroll(dy: f64, usec: u64) -> crate::events::AxisInput {
    use smithay::backend::input::{AxisRelativeDirection, AxisSource};
    crate::events::AxisInput {
        device: String::new(),
        source: AxisSource::Finger,
        usec,
        amount: [None, Some(dy)],
        v120: [None, None],
        relative_direction: [AxisRelativeDirection::Identical; 2],
    }
}

/// The windows of `state`, back to front, one JSON object a line ([`window_line`]).
fn window_list(state: &Cupertino) -> String {
    state
        .wm
        .windows()
        .map(|w| {
            let app = state
                .wm
                .app(w.app)
                .map(|a| a.identity.as_str())
                .unwrap_or_default();
            let frame = w.geometry();
            let entry = state.entry(w.id).filter(|e| e.server_side);
            // the middle of the close and zoom buttons the server draws, from the frame's top left corner
            let buttons = entry.map(|e| {
                let size = e.window.geometry().size;
                let content = cupertino_geometry::Size::new(f64::from(size.w), f64::from(size.h));
                let layout = state.decor.for_style(e.titlebar).layout(content);
                let centre = |kind| {
                    let c = layout.button(kind).center;
                    (frame.x + c.x, frame.y + c.y)
                };
                [
                    centre(cupertino_decorations::ButtonKind::Close),
                    centre(cupertino_decorations::ButtonKind::Zoom),
                ]
            });
            window_line(app, &w.title, frame, buttons)
        })
        .collect()
}

/// One window of the window list: its application, title, frame in logical points and, when the server draws its
/// title bar, the middle of its close and zoom buttons in the same points (`null` otherwise).
pub fn window_line(
    app: &str,
    title: &str,
    frame: cupertino_wm::Rect,
    buttons: Option<[(f64, f64); 2]>,
) -> String {
    let mut line = serde_json::json!({
        "app": app,
        "title": title,
        "x": frame.x,
        "y": frame.y,
        "width": frame.w,
        "height": frame.h,
        "server_side": buttons.is_some(),
        "buttons": buttons.map(|b| b.map(|(x, y)| [x, y])),
    })
    .to_string();
    line.push('\n');
    line
}

/// Start writing per-frame timings to `path`, naming the device that renders.
fn start_frame_stats(
    state: &mut Cupertino,
    path: &std::path::Path,
    refresh_mhz: i32,
    start: Instant,
) -> Result<()> {
    let Some(crate::backend::Backend::Headless(h)) = state.backend.as_mut() else {
        return Err(anyhow!("--frame-stats needs the headless backend"));
    };
    use smithay::backend::renderer::gles::ffi;
    let text = |gl: &ffi::Gles2, name| unsafe {
        let s = gl.GetString(name);
        if s.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(s.cast())
                .to_string_lossy()
                .into_owned()
        }
    };
    let (renderer, extensions) = h
        .renderer
        .with_context(|gl| (text(gl, ffi::RENDERER), text(gl, ffi::EXTENSIONS)))
        .unwrap_or_default();
    // the GPU times each glass pass when it can; otherwise the passes are timed by waiting for the GPU around each
    let gpu_timers = extensions
        .split_whitespace()
        .any(|e| e == "GL_EXT_disjoint_timer_query");
    // and each whole frame, by timestamps
    let timer = if gpu_timers {
        h.renderer
            .with_context(|gl| unsafe { crate::frame_stats::FrameTimer::new(gl) })
            .ok()
            .flatten()
    } else {
        None
    };
    if let Some(material) = state.render.as_ref().and_then(|r| r.material.as_ref()) {
        material.borrow_mut().gpu_timers = gpu_timers;
    }
    let renderer = if renderer.is_empty() {
        "unknown".into()
    } else {
        renderer
    };
    let renderer = format!(
        "{renderer} ({})",
        if gpu_timers {
            "glass timed by GPU timer queries"
        } else {
            "glass timed by waiting for the GPU"
        }
    );
    let refresh = Duration::from_secs_f64(1000.0 / f64::from(refresh_mhz.max(1000)));
    let mut stats = crate::frame_stats::FrameStats::create(path, &renderer, refresh, start)
        .with_context(|| format!("creating {}", path.display()))?;
    stats.timer = timer;
    state.frame_stats = Some(stats);
    Ok(())
}

/// Poll the shell processes of the configuration and the built-in ones (starting them on the first tick) and the X
/// display's satellite from the event loop.
pub fn start_shell_supervision(
    state: &Cupertino,
    event_loop: &EventLoop<'static, Cupertino>,
) -> Result<()> {
    if state.supervisor.is_empty() && state.xwayland.is_none() {
        return Ok(());
    }
    event_loop
        .handle()
        .insert_source(Timer::immediate(), |_, _, state| {
            supervise(state);
            TimeoutAction::Drop
        })
        .map_err(|e| anyhow::anyhow!("inserting the shell supervision timer: {e}"))?;
    Ok(())
}

/// When a child whose exit cannot be watched (no pidfd) is looked at again.
const UNWATCHED_POLL: Duration = Duration::from_millis(100);

/// Start due processes, reap exited ones, and arm what wakes the next pass: a pidfd per running child, readable when it
/// exits, and a timer at the earliest restart. While every child runs, nothing wakes the loop for them.
pub(crate) fn supervise(state: &mut Cupertino) {
    let socket = state
        .socket_name
        .clone()
        .expect("the server listens on a socket when it supervises shell processes");
    let display = state.display_handle.clone();
    if let Some(x) = &mut state.xwayland {
        x.poll(&state.loop_handle, Instant::now());
    }
    let x11 = state.xwayland.as_ref().map(|x| x.display());
    state
        .supervisor
        .poll(Instant::now(), &socket, x11, &mut |spec| {
            crate::shell_connection::connect_bound_client(
                &display,
                crate::shell_connection::shell_process_state(&spec.roles),
            )
        });
    let trust = state.login_trust();
    if let Some(dbus) = &state.dbus {
        dbus.set_login(trust);
    }
    state.refresh_session_agent();

    let mut next = [
        state.supervisor.next_deadline(),
        state.xwayland.as_ref().and_then(|x| x.next_deadline()),
    ]
    .into_iter()
    .flatten()
    .min();
    let running: Vec<u32> = state
        .supervisor
        .pids()
        .into_iter()
        .chain(state.xwayland.as_ref().and_then(|x| x.pid()))
        .collect();
    for pid in running {
        if state.supervision_wakeups.watched.contains_key(&pid) {
            continue;
        }
        match watch_exit(state, pid) {
            Ok(token) => {
                state.supervision_wakeups.watched.insert(pid, token);
            }
            Err(e) => {
                tracing::warn!(pid, error = %e, "cannot watch the exit of a supervised process: it is polled");
                let poll = Instant::now() + UNWATCHED_POLL;
                next = Some(next.map_or(poll, |n| n.min(poll)));
            }
        }
    }
    if let Some(at) = next {
        arm_supervision_timer(state, at);
    }
}

/// A source that runs [`supervise`] once `pid` (a child the server started and has not reaped) exits.
fn watch_exit(
    state: &Cupertino,
    pid: u32,
) -> std::io::Result<smithay::reexports::calloop::RegistrationToken> {
    use smithay::reexports::calloop::generic::Generic;
    use smithay::reexports::calloop::{Interest, Mode, PostAction};
    let raw =
        rustix::process::Pid::from_raw(pid as i32).ok_or_else(|| std::io::Error::other("pid 0"))?;
    let pidfd = rustix::process::pidfd_open(raw, rustix::process::PidfdFlags::empty())?;
    state
        .loop_handle
        .insert_source(
            Generic::new(pidfd, Interest::READ, Mode::Level),
            move |_, _, state| {
                state.supervision_wakeups.watched.remove(&pid);
                supervise(state);
                Ok(PostAction::Remove)
            },
        )
        .map_err(|e| std::io::Error::other(e.to_string()))
}

/// Make sure a supervision pass runs at `at`: an armed timer that fires sooner is kept, a later one is replaced.
fn arm_supervision_timer(state: &mut Cupertino, at: Instant) {
    if let Some((token, armed)) = state.supervision_wakeups.timer {
        if armed <= at {
            return;
        }
        state.loop_handle.remove(token);
    }
    let inserted = state
        .loop_handle
        .insert_source(Timer::from_deadline(at), |_, _, state| {
            state.supervision_wakeups.timer = None;
            supervise(state);
            TimeoutAction::Drop
        });
    match inserted {
        Ok(token) => state.supervision_wakeups.timer = Some((token, at)),
        Err(e) => {
            tracing::warn!(error = %e, "cannot arm the supervision timer: a restart waits for the next exit")
        }
    }
}

/// Takes an X display for X11 programs when `xwayland-satellite` is installed; without it, or when no display can be
/// taken, the session serves Wayland clients only.
fn start_xwayland(state: &mut Cupertino) {
    let Some(program) = crate::state::which(crate::xwayland::SATELLITE) else {
        tracing::info!("xwayland-satellite is not installed: X11 programs cannot run");
        return;
    };
    let (Some(socket), Some(runtime_dir)) = (
        state.socket_name.clone(),
        std::env::var_os("XDG_RUNTIME_DIR"),
    ) else {
        return;
    };
    let started = crate::xwayland::XDisplay::open(
        std::path::Path::new(crate::xwayland::X_TMP),
        std::path::Path::new(&runtime_dir),
    )
    .and_then(|display| {
        crate::xwayland::XWayland::start(
            display,
            program,
            socket,
            &state.loop_handle,
            |s| s.xwayland.as_mut(),
            supervise,
        )
    });
    match started {
        Ok(x) => state.xwayland = Some(x),
        Err(e) => tracing::warn!(error = %e, "no X display: X11 programs cannot run"),
    }
}

/// Sessions in a row that ended with a lost rendering context, kept in `$XDG_STATE_HOME/cupertino`, so a loss that recurs
/// at every login shows in the journal: counted up when this one did (`lost`), cleared when it did not. `None` when the
/// count cannot be kept.
fn lost_context_streak(lost: bool) -> Option<u32> {
    // a relative XDG_STATE_HOME is ignored, as the base directory specification says
    let dir = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?
        .join("cupertino");
    count_streak(&dir.join(LOST_CONTEXT_STREAK), lost)
}

const LOST_CONTEXT_STREAK: &str = "lost-context-streak";

fn count_streak(path: &std::path::Path, lost: bool) -> Option<u32> {
    if !lost {
        let _ = std::fs::remove_file(path);
        return None;
    }
    // a count is a few digits: the read is bounded, so a huge file or a device in its place costs nothing
    let mut text = String::new();
    let before = std::fs::File::open(path)
        .and_then(|f| std::io::Read::read_to_string(&mut std::io::Read::take(f, 16), &mut text))
        .ok()
        .and_then(|_| text.trim().parse::<u32>().ok())
        .unwrap_or(0);
    let count = before.saturating_add(1);
    std::fs::create_dir_all(path.parent()?).ok()?;
    std::fs::write(path, format!("{count}\n")).ok()?;
    Some(count)
}

/// Start the compositor with the given options and run until it exits.
#[cfg(feature = "udev")]
fn check_hardware() -> Result<()> {
    let report = crate::backend::udev::preflight()?;
    print!("{report}");
    if report.is_ok() {
        println!("ok: nothing blocks the TTY backend");
        Ok(())
    } else {
        Err(anyhow!("this machine is not set up for the TTY backend"))
    }
}

pub fn run(cli: Cli) -> Result<()> {
    // the login screen's process is not dumpable, and its screen reader socket is taken before any thread or child exists:
    // a dumpable login screen does not take it at all
    let login_screen = crate::config::login_screen(
        std::env::var_os(crate::config::CONFIG_ENV).as_deref(),
        std::env::var_os("XDG_SESSION_CLASS").as_deref(),
    );
    let screen_reader = crate::screen_reader::take(login_screen && crate::screen_reader::harden());
    // the ready descriptor is for the program that started the server alone, never for one the server starts
    if let Some(fd) = cli.ready_fd {
        crate::runtime_info::keep_from_children(fd);
    }
    if cli.check_hardware {
        #[cfg(feature = "udev")]
        return check_hardware();
        #[cfg(not(feature = "udev"))]
        return Err(anyhow!("built without the udev feature"));
    }
    let assets = Assets::discover();
    let mode = cli.mode()?;
    let (cfg, files) = cli.config()?;
    let mut event_loop: EventLoop<'static, Cupertino> =
        EventLoop::try_new().context("creating the event loop")?;
    let display: Display<Cupertino> = Display::new().context("creating the wayland display")?;
    let mut state = Cupertino::new(
        display,
        event_loop.handle(),
        event_loop.get_signal(),
        cfg,
        true,
        &assets,
    )?;
    state.login_screen = login_screen;
    let _runtime_file = announce_start(&cli, mode, &state)?;
    if let Some(f) = cli.time_scale.filter(|f| *f > 0.0) {
        state.time_scale = f;
    }
    let deadline = cli
        .run_for
        .map(|s| Instant::now() + Duration::from_secs_f64(s));
    // the user's session keeps its accessibility settings over the machine's (the login screen's account has none, so it
    // has the machine's); offscreen runs keep them in memory unless the environment names their file
    use cupertino_defaults::accessibility::{ACCESSIBILITY_ENV, ACCESSIBILITY_FILE};
    let mut paths = crate::live::SettingsPaths::from_env();
    if mode == Mode::Headless {
        paths.system_accessibility = None;
        if let Some(path) = std::env::var_os(ACCESSIBILITY_ENV).filter(|p| !p.is_empty()) {
            state.use_accessibility_store(path.into())?;
        }
    } else {
        if let Some(system) = paths
            .system_accessibility
            .as_deref()
            .and_then(crate::accessibility::read_system_accessibility)
        {
            state.set_system_accessibility(system);
        }
        match cupertino_defaults::config_file(ACCESSIBILITY_ENV, ACCESSIBILITY_FILE) {
            Some(path) => state.use_accessibility_store(path)?,
            None => tracing::warn!(
                "neither XDG_CONFIG_HOME nor HOME is set: accessibility settings are not stored"
            ),
        }
    }
    state.start_settings_watch(files, paths);
    // the scripted steps and the recording count their seconds from the same instant
    let script_start = Instant::now();
    schedule_script(&cli, &event_loop)?;
    if mode != Mode::Headless {
        start_xwayland(&mut state);
    }
    // the built-in processes belong to a user's session on the hardware: not to the login screen, nor to nested and
    // offscreen runs
    if mode == Mode::Udev && !login_screen {
        let programs = BuiltIn::ALL.map(|b| (b, b.installed()));
        state.supervisor.add_built_ins(programs, Instant::now());
    }
    if let Some(program) = &cli.control_program {
        state.supervisor.add_built_ins(
            [(BuiltIn::DesktopControl, Some(program.clone()))],
            Instant::now(),
        );
    }
    start_shell_supervision(&state, &event_loop)?;
    state.overlays.hold_modifiers = cli.hold_modifiers;
    state.connect_input_method();
    if let Some(socket) = screen_reader {
        state.start_screen_reader(socket);
    }
    // a person's session on the hardware runs its own reader; nested and offscreen runs never start one
    if mode == Mode::Udev && !login_screen {
        state.session_reader.program = cupertino_defaults::accessibility::installed_screen_reader();
        state.sync_screen_reader();
        // the window buttons the server draws are read on the session's accessibility bus, as the clients' content is
        state.start_button_access();
    }
    if mode != Mode::Headless || cli.dbus {
        start_dbus(&mut state);
    }
    tracing::info!(?mode, socket = ?state.socket_name, "starting");
    match mode {
        #[cfg(feature = "winit")]
        Mode::Winit => {
            let events = crate::backend::winit::init(&mut state, cli.size, cli.scale)?;
            export_socket(&state);
            crate::backend::winit::run(&mut state, &mut event_loop, events, deadline);
        }
        #[cfg(not(feature = "winit"))]
        Mode::Winit => return Err(anyhow!("built without the winit feature")),
        Mode::Headless => {
            let physical_mm = cli
                .physical_size
                .map_or((0, 0), |(w, h)| (w as i32, h as i32));
            let display = crate::backend::headless::HeadlessDisplay {
                name: cli.connector.clone().unwrap_or_else(|| "headless-1".into()),
                size: (cli.size.0 as i32, cli.size.1 as i32),
                physical_mm,
            };
            // with a physical size the output starts at its own default scale unless one is given
            let scale = match cli.physical_size {
                Some(_) => cli.scale,
                None => Some(cli.scale.unwrap_or(1.0)),
            };
            crate::backend::headless::init_displays(&mut state, &[], scale)?;
            let refresh_mhz = cli
                .refresh
                .map_or(crate::backend::headless::DEFAULT_REFRESH_MHZ, |hz| {
                    (hz * 1000.0).round() as i32
                });
            crate::backend::headless::add_display_at(
                &mut state,
                &display,
                refresh_mhz,
                &[],
                scale,
            )?;
            if let Some(path) = &cli.frame_stats {
                start_frame_stats(&mut state, path, refresh_mhz, script_start)?;
            }
            for (name, (w, h), scale) in &cli.extra_outputs {
                let extra = crate::backend::headless::HeadlessDisplay {
                    name: name.clone(),
                    size: (*w as i32, *h as i32),
                    physical_mm: (0, 0),
                };
                crate::backend::headless::add_display(&mut state, &extra, Some(*scale))?;
            }
            state.screenshot_each_output = !cli.extra_outputs.is_empty();
            export_socket(&state);
            let recorder = cli
                .record
                .as_deref()
                .map(|path| {
                    let after = Duration::from_secs_f64(cli.record_after.max(0.0));
                    crate::backend::record::start(&event_loop, path, script_start + after)
                })
                .transpose()?;
            crate::backend::headless::run(&mut state, &mut event_loop, deadline)?;
            if let Some(stats) = state.frame_stats.as_mut() {
                stats.flush();
            }
            if let Some(recorder) = recorder {
                let written = recorder.finish()?;
                tracing::info!(
                    frames = written.frames,
                    drawn = written.drawn,
                    due = written.due,
                    "recording written"
                );
            }
        }
        #[cfg(feature = "udev")]
        Mode::Udev => {
            // the real displays keep their arrangement; nested and offscreen runs keep it in memory
            use cupertino_defaults::displays::{DISPLAYS_ENV, DISPLAYS_FILE};
            match cupertino_defaults::config_file(DISPLAYS_ENV, DISPLAYS_FILE) {
                Some(path) => state.displays.use_store(path)?,
                None => tracing::warn!(
                    "neither XDG_CONFIG_HOME nor HOME is set: display arrangements are not stored"
                ),
            }
            publish_session_when_ready(&state);
            crate::backend::udev::run(&mut state, &mut event_loop, deadline, cli.scale)?;
        }
        #[cfg(not(feature = "udev"))]
        Mode::Udev => return Err(anyhow!("built without the udev feature")),
    }
    tracing::info!("shutting down");
    // only a session on the hardware counts: nested and offscreen runs (tests among them) leave the count alone
    let streak = (mode == Mode::Udev)
        .then(|| lost_context_streak(state.rendering_lost.is_some()))
        .flatten();
    if let Some(reason) = &state.rendering_lost {
        return Err(anyhow!(
            "the rendering context was lost ({reason}); sessions in a row that ended this way: {}",
            streak.unwrap_or(1)
        ));
    }
    Ok(())
}

/// Start the session bus worker unless `CUPERTINO_NO_DBUS` is set; failures only disable the
/// notification centre shortcut.
#[cfg(feature = "dbus")]
fn start_dbus(state: &mut Cupertino) {
    if std::env::var_os("CUPERTINO_NO_DBUS").is_some() {
        return;
    }
    // the worker cannot touch the state: it tells the loop, which ends the server when a log out finds no login process
    let (login_missing, rx) = smithay::reexports::calloop::channel::channel::<()>();
    let inserted = state.loop_handle.insert_source(rx, |event, _, state| {
        if let smithay::reexports::calloop::channel::Event::Msg(()) = event {
            state.quit_compositor();
        }
    });
    state.dbus = match inserted {
        Ok(_) => crate::dbus::DbusHandle::start_with(move || {
            // the loop is gone only while the server shuts down
            let _ = login_missing.send(());
        }),
        Err(e) => {
            tracing::warn!(error = %e, "cannot listen for the D-Bus worker");
            crate::dbus::DbusHandle::start()
        }
    };
    if state.config.dev_shell {
        if let Some(dbus) = &state.dbus {
            dbus.set_login(crate::dbus::LoginTrust::Any);
        }
    }
    state.watch_menu_reader();
    crate::power::start_power_bus(state);
    crate::localed::start(state);
    crate::quit::start_session_service(state);
}

#[cfg(not(feature = "dbus"))]
fn start_dbus(_state: &mut Cupertino) {}

/// How long the session's publication waits for the portal to own [`PORTAL_BUS_NAME`].
pub const PORTAL_READY_TIMEOUT: Duration = Duration::from_secs(5);

/// [`publish_session`] once the supervised portal owns its bus name, so the portal frontend that the publication starts
/// finds its backend; after [`PORTAL_READY_TIMEOUT`] without the name it is published all the same, and at once when
/// the portal is not supervised. The event loop goes on meanwhile: the portal needs it to connect.
pub fn publish_session_when_ready(state: &Cupertino) {
    use smithay::reexports::calloop::channel::{channel, Event};
    let Some(dbus) = state
        .dbus
        .as_ref()
        .filter(|_| state.supervisor.supervises(BuiltIn::Portal.name()))
    else {
        publish_session(state);
        return;
    };
    let published = std::rc::Rc::new(std::cell::Cell::new(false));
    let publish = move |state: &mut Cupertino, owned: bool| {
        if published.replace(true) {
            return;
        }
        if !owned {
            tracing::warn!(
                name = PORTAL_BUS_NAME,
                timeout = ?PORTAL_READY_TIMEOUT,
                "the portal did not take its bus name in time: the session is published without it"
            );
        }
        publish_session(state);
    };
    let (tx, rx) = channel::<()>();
    let on_owned = publish.clone();
    let owner_source = state.loop_handle.insert_source(rx, move |event, _, state| {
        if let Event::Msg(()) = event {
            on_owned(state, true);
        }
    });
    let timer = owner_source.as_ref().ok().map(|&token| {
        state.loop_handle.insert_source(
            Timer::from_duration(PORTAL_READY_TIMEOUT),
            move |_, _, state| {
                publish(state, false);
                state.loop_handle.remove(token);
                TimeoutAction::Drop
            },
        )
    });
    let waiting = matches!(timer, Some(Ok(_)))
        && dbus.await_owner(
            PORTAL_BUS_NAME,
            Box::new(move || {
                // the loop is gone only while the server shuts down
                let _ = tx.send(());
            }),
        );
    if !waiting {
        tracing::warn!("cannot wait for the portal's bus name: the session is published at once");
        if let Ok(token) = owner_source {
            state.loop_handle.remove(token);
        }
        if let Some(Ok(token)) = timer {
            state.loop_handle.remove(token);
        }
        publish_session(state);
    }
}

/// The TTY session's display and desktop name, for the services of a shared user bus ([`crate::activation_env`]).
pub fn publish_session(state: &Cupertino) {
    let (Some(socket), Some(dbus)) = (&state.socket_name, &state.dbus) else {
        return;
    };
    let mut variables = vec![
        ("WAYLAND_DISPLAY".into(), socket.clone()),
        (
            "XDG_CURRENT_DESKTOP".into(),
            crate::state::CURRENT_DESKTOP.into(),
        ),
        ("XDG_SESSION_TYPE".into(), "wayland".into()),
    ];
    let mut defaults = Vec::new();
    if let Some(x) = &state.xwayland {
        variables.push(("DISPLAY".into(), x.display().name()));
        variables.push((
            "XAUTHORITY".into(),
            x.display().authority().to_string_lossy().into_owned(),
        ));
        // a value the session gave the server is the user's choice, as for the programs it starts
        defaults.extend(
            crate::xwayland::WAYLAND_FIRST
                .iter()
                .filter(|(name, _)| std::env::var_os(name).is_none())
                .map(|(name, value)| ((*name).into(), (*value).into())),
        );
    }
    dbus.publish_session(variables, defaults);
}

/// The runtime file of this server, when `XDG_RUNTIME_DIR` is set (removed when the returned file drops, at the end of
/// [`run`], or when a signal ends the server), then the readiness line on `--ready-fd`. A runtime file that cannot be
/// written is logged; a ready descriptor that is not open ends the start.
fn announce_start(
    cli: &Cli,
    mode: Mode,
    state: &Cupertino,
) -> Result<Option<crate::runtime_info::RuntimeFile>> {
    use crate::runtime_info;
    let Some(display) = &state.socket_name else {
        return Ok(None);
    };
    let backend = match mode {
        Mode::Udev => "udev",
        Mode::Winit => "winit",
        Mode::Headless => "headless",
    };
    let file = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|dir| !dir.is_empty())
        .and_then(|dir| {
            let info = runtime_info::RuntimeInfo::now(display, backend);
            runtime_info::write(std::path::Path::new(&dir), &info)
                .inspect_err(|e| tracing::warn!(error = %e, "cannot write the runtime file"))
                .ok()
        });
    if let Some(file) = &file {
        file.remove_on_ending_signals();
    }
    if let Some(fd) = cli.ready_fd {
        runtime_info::announce(fd, display).context("announcing the socket on --ready-fd")?;
    }
    Ok(file)
}

fn export_socket(state: &Cupertino) {
    if let Some(s) = &state.socket_name {
        // children started by the compositor inherit nothing from the host session's display
        println!("WAYLAND_DISPLAY={s}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scripted_scroll_moves_the_vertical_axis_alone_and_its_zero_ends_it() {
        // the steps of a scroll stop neither axis, so the client sees one gesture, not one per step
        for dy in [20.0, -3.5] {
            let step = scripted_scroll(dy, 1_000);
            assert_eq!(step.stopped_axes(), [false, false], "{dy}");
            assert!(!step.ends_gesture(), "{dy}");
        }
        // the zero at the end stops the vertical axis alone and ends the gesture
        let end = scripted_scroll(0.0, 2_000);
        assert_eq!(end.stopped_axes(), [false, true]);
        assert!(end.ends_gesture());
        // a zero on the horizontal axis would have stopped it at every step
        let both = crate::events::AxisInput {
            amount: [Some(0.0), Some(20.0)],
            ..scripted_scroll(20.0, 1_000)
        };
        assert_eq!(both.stopped_axes(), [true, false]);
    }

    #[test]
    fn a_window_list_line_is_one_json_object_with_the_frame_in_points() {
        let line = window_line(
            "org.example.Files",
            "Home \"docs\"",
            cupertino_wm::Rect::new(10.0, 32.5, 800.0, 500.0),
            Some([(36.0, 58.5), (76.0, 58.5)]),
        );
        assert!(line.ends_with('\n'));
        let v: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["app"], "org.example.Files");
        assert_eq!(v["title"], "Home \"docs\"");
        assert_eq!((v["x"].as_f64(), v["y"].as_f64()), (Some(10.0), Some(32.5)));
        assert_eq!(
            (v["width"].as_f64(), v["height"].as_f64()),
            (Some(800.0), Some(500.0))
        );
        assert_eq!(v["server_side"], true);
        assert_eq!(
            v["buttons"],
            serde_json::json!([[36.0, 58.5], [76.0, 58.5]])
        );
        let bare = window_line("a", "b", cupertino_wm::Rect::new(0.0, 0.0, 1.0, 1.0), None);
        let v: serde_json::Value = serde_json::from_str(bare.trim_end()).unwrap();
        assert_eq!(
            (v["server_side"].clone(), v["buttons"].clone()),
            (false.into(), serde_json::Value::Null)
        );
    }

    #[test]
    fn lost_contexts_are_counted_in_a_row_and_cleared_by_a_normal_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cupertino").join(LOST_CONTEXT_STREAK);
        assert_eq!(count_streak(&path, true), Some(1));
        assert_eq!(count_streak(&path, true), Some(2));
        assert_eq!(count_streak(&path, false), None);
        assert!(!path.exists());
        assert_eq!(count_streak(&path, true), Some(1));

        std::fs::write(&path, "x".repeat(1 << 20)).unwrap();
        assert_eq!(
            count_streak(&path, true),
            Some(1),
            "a malformed count starts over"
        );
    }

    #[test]
    fn size_parses() {
        assert_eq!(parse_size("1280x800").unwrap(), (1280, 800));
        assert!(parse_size("0x10").is_err());
        assert!(parse_size("abc").is_err());
    }

    #[test]
    fn timed_points_parse() {
        assert_eq!(
            parse_timed_point("2.5:100,40.5").unwrap(),
            (2.5, (100.0, 40.5))
        );
        assert!(parse_timed_point("2:100").is_err());
        assert!(parse_timed_point("x:1,2").is_err());
    }

    #[test]
    fn timed_actions_parse() {
        let (t, a) = parse_timed_action("2.5:overview").unwrap();
        assert_eq!(t, 2.5);
        assert_eq!(a, Action::Overview);
        assert!(parse_timed_action("x:overview").is_err());
        assert!(parse_timed_action("1:nope").is_err());
    }

    #[test]
    fn timed_text_keeps_colons_in_the_text() {
        assert_eq!(
            parse_timed_text("1.5:Super+space").unwrap(),
            (1.5, "Super+space".to_string())
        );
        assert_eq!(parse_timed_text("2:a:b").unwrap(), (2.0, "a:b".to_string()));
        assert!(parse_timed_text("x:y").is_err());
        assert!(parse_timed_text("nocolon").is_err());
        assert_eq!(parse_timed_crash("1.5:abort").unwrap(), (1.5, Crash::Abort));
        assert!(parse_timed_crash("1:segfault").is_err());
    }

    #[test]
    fn mode_requires_one_backend() {
        let cli = Cli {
            winit: true,
            udev: true,
            ..Default::default()
        };
        assert!(cli.mode().is_err());
        let cli = Cli {
            headless: true,
            ..Default::default()
        };
        assert_eq!(cli.mode().unwrap(), Mode::Headless);
    }
}
