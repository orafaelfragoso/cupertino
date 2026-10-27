//! Command line interface and startup of the compositor.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use clap::{ArgGroup, Parser};
use cupertino_assets::Assets;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

use crate::config::Config;
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
    /// Development and nested runs: let any configuration file name the shell processes and the screenshot process, from
    /// any path. Without it only the system file, programs under /usr, and the person's file removing or reordering them
    /// apply.
    #[arg(long)]
    pub dev_shell: bool,
    /// Output size in pixels, e.g. 1280x800 (nested window / headless output).
    #[arg(long, value_parser = parse_size, default_value = "1280x800")]
    pub size: (u32, u32),
    /// Output scale (default: the host scale for the nested window, 1 otherwise).
    #[arg(long)]
    pub scale: Option<f64>,
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
    /// Do not draw the menu bar.
    #[arg(long)]
    pub no_menu_bar: bool,
    /// Run model time (animations) at this multiple of the wall clock, e.g. 0.25 for slow motion.
    #[arg(long, value_name = "FACTOR")]
    pub time_scale: Option<f64>,
    /// Start the session bus worker even for the headless backend (menu registrar, status items).
    #[arg(long)]
    pub dbus: bool,
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

pub fn parse_timed_text(s: &str) -> Result<(f64, String), String> {
    let (t, rest) = s.split_once(':').ok_or("expected SECONDS:TEXT")?;
    Ok((
        t.parse::<f64>().map_err(|e| e.to_string())?,
        rest.to_string(),
    ))
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
            let ctx = state.action_context();
            let plan = translate_action(action, &ctx);
            state.execute_plan(plan);
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
            state.pointer_moved_to((x, y).into(), 0, None);
        })?;
    }
    for (t, (x, y)) in cli.clicks.iter().copied() {
        schedule(event_loop, t, move |state| {
            use smithay::backend::input::ButtonState;
            state.pointer_moved_to((x, y).into(), 0, None);
            state.pointer_button(crate::events::BTN_LEFT, ButtonState::Pressed, 0);
            state.pointer_button(crate::events::BTN_LEFT, ButtonState::Released, 0);
        })?;
    }
    for (t, (x, y)) in cli.right_clicks.iter().copied() {
        schedule(event_loop, t, move |state| {
            use smithay::backend::input::ButtonState;
            state.pointer_moved_to((x, y).into(), 0, None);
            state.pointer_button(crate::events::BTN_RIGHT, ButtonState::Pressed, 0);
            state.pointer_button(crate::events::BTN_RIGHT, ButtonState::Released, 0);
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
            state.pointer_moved_to((x, y).into(), 0, None);
            state.pointer_button(
                crate::events::BTN_LEFT,
                smithay::backend::input::ButtonState::Pressed,
                0,
            );
        })?;
    }
    for (t, (x, y)) in cli.releases.iter().copied() {
        schedule(event_loop, t, move |state| {
            state.pointer_moved_to((x, y).into(), 0, None);
            state.pointer_button(
                crate::events::BTN_LEFT,
                smithay::backend::input::ButtonState::Released,
                0,
            );
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

/// How often the supervisor looks for exited shell processes; the restart throttle is 1 s, so a
/// restart is at most this late.
const SUPERVISION_INTERVAL: Duration = Duration::from_millis(100);

/// Poll the shell processes of the configuration from the event loop (starting them on the first tick).
pub fn start_shell_supervision(
    state: &Cupertino,
    event_loop: &EventLoop<'static, Cupertino>,
) -> Result<()> {
    if state.config.shell_processes.is_empty() {
        return Ok(());
    }
    let timer = Timer::immediate();
    event_loop
        .handle()
        .insert_source(timer, |_, _, state| {
            let socket = state
                .socket_name
                .clone()
                .expect("the server listens on a socket when it supervises shell processes");
            let display = state.display_handle.clone();
            state.supervisor.poll(Instant::now(), &socket, &mut |spec| {
                crate::shell_connection::connect_bound_client(
                    &display,
                    crate::shell_connection::shell_process_state(&spec.roles),
                )
            });
            TimeoutAction::ToDuration(SUPERVISION_INTERVAL)
        })
        .map_err(|e| anyhow::anyhow!("inserting the shell supervision timer: {e}"))?;
    Ok(())
}

/// Start the compositor with the given options and run until it exits.
pub fn run(cli: Cli) -> Result<()> {
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
    if let Some(f) = cli.time_scale.filter(|f| *f > 0.0) {
        state.time_scale = f;
    }
    let deadline = cli
        .run_for
        .map(|s| Instant::now() + Duration::from_secs_f64(s));
    // the user's session keeps its accessibility settings; offscreen runs keep them in memory
    if mode != Mode::Headless {
        use cupertino_defaults::accessibility::{ACCESSIBILITY_ENV, ACCESSIBILITY_FILE};
        match cupertino_defaults::config_file(ACCESSIBILITY_ENV, ACCESSIBILITY_FILE) {
            Some(path) => state.use_accessibility_store(path)?,
            None => tracing::warn!(
                "neither XDG_CONFIG_HOME nor HOME is set: accessibility settings are not stored"
            ),
        }
    }
    state.start_settings_watch(files, crate::live::SettingsPaths::from_env());
    schedule_script(&cli, &event_loop)?;
    start_shell_supervision(&state, &event_loop)?;
    state.overlays.hold_modifiers = cli.hold_modifiers;
    state.connect_input_method();
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
            crate::backend::headless::init(
                &mut state,
                (cli.size.0 as i32, cli.size.1 as i32),
                cli.scale.unwrap_or(1.0),
            )?;
            export_socket(&state);
            crate::backend::headless::run(&mut state, &mut event_loop, deadline)?;
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
            crate::backend::udev::run(&mut state, &mut event_loop, deadline, cli.scale)?;
        }
        #[cfg(not(feature = "udev"))]
        Mode::Udev => return Err(anyhow!("built without the udev feature")),
    }
    tracing::info!("shutting down");
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
    crate::power::start_power_bus(state);
    crate::quit::start_session_service(state);
}

#[cfg(not(feature = "dbus"))]
fn start_dbus(_state: &mut Cupertino) {}

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
