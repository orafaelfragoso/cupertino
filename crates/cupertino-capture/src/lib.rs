//! The look-check capture command's shared part. Each app's `<app>_capture` example draws its own surfaces with its own
//! painter and hands them to [`run`], which reads the flags, writes one PNG per state and appearance under the names of
//! [`file_name`] and, with `--sheet`, one contact sheet: the states as columns, the appearances as rows.
//!
//! `ci/capture.sh APP/SURFACE [flags]` runs the example of APP; `ci/capture.sh APP` lists its surfaces.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cupertino_assets::Assets;
use tiny_skia::{Color, Pixmap, PixmapPaint, Transform};

/// The session settings a capture is drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub dark: bool,
    pub increase_contrast: bool,
    pub reduce_transparency: bool,
}

/// A surface's state: its state flags and their values, in the order the app takes them. Empty is the surface's
/// default state.
pub type State = Vec<(String, String)>;

/// One picture to draw.
#[derive(Debug)]
pub struct Request<'a> {
    /// The surface's name inside its app (`accessibility` of `settings/accessibility`).
    pub surface: &'a str,
    pub state: &'a State,
    pub look: Look,
    /// Device pixels per point.
    pub scale: f32,
    /// The size in points given with `--size`; `None` is the surface's own size.
    pub size: Option<(f64, f64)>,
}

/// An app's surfaces.
pub trait App {
    /// The app's part of surface and file names (`settings`).
    const NAME: &'static str;
    /// The state flags it takes, without the dashes (`login`).
    const STATE_FLAGS: &'static [&'static str];

    fn surfaces(&self) -> Vec<String>;

    /// The states a contact sheet of `surface` shows, the default first.
    fn sheet_states(&self, surface: &str) -> Vec<State>;

    /// Draw one picture; an error names what cannot be drawn (an unknown surface or state value).
    fn capture(&mut self, request: &Request<'_>) -> Result<Pixmap, String>;
}

/// What the command line asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// `None` when the command line names the app alone, which lists its surfaces.
    pub surface: Option<String>,
    pub state: State,
    pub scale: f32,
    /// The appearances to draw, light before dark.
    pub dark: Vec<bool>,
    pub increase_contrast: bool,
    pub reduce_transparency: bool,
    pub size: Option<(f64, f64)>,
    pub out: PathBuf,
    pub sheet: bool,
}

pub const USAGE: &str =
    "usage: capture APP/SURFACE [--scale N] [--appearance light|dark|both] [--increase-contrast] \
[--reduce-transparency] [--size WxH] [--out DIR] [--sheet] [--STATE-FLAG VALUE]...
       capture APP                           (lists its surfaces)";

/// The default output directory.
pub const OUT: &str = "target/captures";

/// The pixels between the cells of a contact sheet, per point of scale.
const SHEET_GAP: f32 = 16.0;

/// The fill around and between the cells of a contact sheet, a neutral grey that is neither appearance's background.
const SHEET_BACKGROUND: [u8; 3] = [128, 128, 128];

/// Read `args` (without the program name) of app `app`, which takes `state_flags`.
pub fn parse(
    app: &str,
    state_flags: &[&str],
    args: impl IntoIterator<Item = String>,
) -> Result<Options, String> {
    let mut options = Options {
        surface: None,
        state: State::new(),
        scale: 1.0,
        dark: vec![false, true],
        increase_contrast: false,
        reduce_transparency: false,
        size: None,
        out: PathBuf::from(OUT),
        sheet: false,
    };
    let mut list = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| args.next().ok_or_else(|| format!("{flag} needs a value"));
        match arg.as_str() {
            "--scale" => {
                let v = value("--scale")?;
                options.scale = v
                    .parse()
                    .ok()
                    .filter(|s: &f32| s.is_finite() && *s > 0.0)
                    .ok_or_else(|| format!("--scale {v}: not a positive number"))?;
            }
            "--appearance" => {
                options.dark = match value("--appearance")?.as_str() {
                    "light" => vec![false],
                    "dark" => vec![true],
                    "both" => vec![false, true],
                    other => return Err(format!("--appearance {other}: light, dark or both")),
                }
            }
            "--increase-contrast" => options.increase_contrast = true,
            "--reduce-transparency" => options.reduce_transparency = true,
            "--size" => {
                let v = value("--size")?;
                options.size = Some(
                    v.split_once('x')
                        .and_then(|(w, h)| Some((w.parse::<f64>().ok()?, h.parse::<f64>().ok()?)))
                        .filter(|(w, h)| *w > 0.0 && *h > 0.0)
                        .ok_or_else(|| format!("--size {v}: WIDTHxHEIGHT in points"))?,
                );
            }
            "--out" => options.out = PathBuf::from(value("--out")?),
            "--sheet" => options.sheet = true,
            flag => match flag.strip_prefix("--") {
                Some(name) if state_flags.contains(&name) => {
                    let v = value(flag)?;
                    options.state.retain(|(n, _)| n != name);
                    options.state.push((name.to_string(), v));
                }
                Some(_) => return Err(format!("{app} has no flag {flag}")),
                None if arg == app && !list => list = true,
                None if options.surface.is_none() => {
                    let surface = match arg.split_once('/') {
                        Some((a, s)) if a == app && !s.is_empty() => s.to_string(),
                        _ => return Err(format!("{arg}: the surface is {app}/SURFACE")),
                    };
                    options.surface = Some(surface);
                }
                None => return Err(format!("{arg}: one surface at a time")),
            },
        }
    }
    let order = |(name, _): &(String, String)| state_flags.iter().position(|f| f == name);
    options.state.sort_by_key(order);
    match (&options.surface, list) {
        (None, false) => Err(USAGE.to_string()),
        (Some(_), true) => Err(format!("{app}: one surface at a time")),
        _ => Ok(options),
    }
}

/// A state value as it appears in a file name: lower case, each run of other characters than letters and digits one `-`.
fn kebab(value: &str) -> String {
    value
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// `<app>-<surface>[-<flag>-<value>]...-<light|dark>[-hc][-rt]@<scale>x.png`.
pub fn file_name(app: &str, surface: &str, state: &State, look: Look, scale: f32) -> String {
    let mut name = format!("{app}-{surface}");
    for (flag, value) in state {
        name += &format!("-{flag}-{}", kebab(value));
    }
    name += if look.dark { "-dark" } else { "-light" };
    name += &suffix(look.increase_contrast, look.reduce_transparency, scale);
    name + ".png"
}

/// `<app>-<surface>[-hc][-rt]-sheet@<scale>x.png`.
pub fn sheet_name(app: &str, surface: &str, look: Look, scale: f32) -> String {
    format!(
        "{app}-{surface}-sheet{}.png",
        suffix(look.increase_contrast, look.reduce_transparency, scale)
    )
}

fn suffix(increase_contrast: bool, reduce_transparency: bool, scale: f32) -> String {
    format!(
        "{}{}@{scale}x",
        if increase_contrast { "-hc" } else { "" },
        if reduce_transparency { "-rt" } else { "" },
    )
}

/// The cells laid out in rows on [`SHEET_BACKGROUND`], [`SHEET_GAP`] pixels per point of `scale` apart and around:
/// each cell at the top left of a slot as large as the largest cell.
pub fn sheet(rows: &[Vec<Pixmap>], scale: f32) -> Option<Pixmap> {
    let gap = (SHEET_GAP * scale).round() as u32;
    let cells = rows.iter().flatten();
    let slot_w = cells.clone().map(Pixmap::width).max()?;
    let slot_h = cells.map(Pixmap::height).max()?;
    let columns = rows.iter().map(Vec::len).max()? as u32;
    let mut pm = Pixmap::new(
        gap + columns * (slot_w + gap),
        gap + rows.len() as u32 * (slot_h + gap),
    )?;
    let [r, g, b] = SHEET_BACKGROUND;
    pm.fill(Color::from_rgba8(r, g, b, 255));
    for (y, row) in rows.iter().enumerate() {
        for (x, cell) in row.iter().enumerate() {
            pm.draw_pixmap(
                (gap + x as u32 * (slot_w + gap)) as i32,
                (gap + y as u32 * (slot_h + gap)) as i32,
                cell.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
    }
    Some(pm)
}

/// The asset packs, or an error when none is found: a capture with the built-in fallback alone does not show the
/// surfaces as they look.
pub fn assets() -> Result<Assets, String> {
    let assets = Assets::discover();
    if assets.roots().next().is_none() {
        return Err("no asset pack found: set CUPERTINO_ASSET_DIR to the asset pack".to_string());
    }
    Ok(assets)
}

fn save(pm: &Pixmap, out: &Path, name: &str) -> Result<(), String> {
    let path = out.join(name);
    pm.save_png(&path)
        .map_err(|e| format!("writing {}: {e}", path.display()))?;
    println!("{}", path.display());
    Ok(())
}

/// Carry out `options` for `app`.
pub fn capture<A: App>(app: &mut A, options: &Options) -> Result<(), String> {
    let Some(surface) = options.surface.as_deref() else {
        for surface in app.surfaces() {
            println!("{}/{surface}", A::NAME);
        }
        return Ok(());
    };
    if !app.surfaces().iter().any(|s| s == surface) {
        return Err(format!(
            "{0}/{surface}: not a surface (`capture {0}` lists them)",
            A::NAME
        ));
    }
    std::fs::create_dir_all(&options.out)
        .map_err(|e| format!("creating {}: {e}", options.out.display()))?;
    let states = if options.sheet {
        app.sheet_states(surface)
    } else {
        vec![options.state.clone()]
    };
    let look = |dark| Look {
        dark,
        increase_contrast: options.increase_contrast,
        reduce_transparency: options.reduce_transparency,
    };
    let mut rows = Vec::new();
    for &dark in &options.dark {
        let mut row = Vec::new();
        for state in &states {
            let request = Request {
                surface,
                state,
                look: look(dark),
                scale: options.scale,
                size: options.size,
            };
            let pm = app.capture(&request)?;
            save(
                &pm,
                &options.out,
                &file_name(A::NAME, surface, state, look(dark), options.scale),
            )?;
            row.push(pm);
        }
        rows.push(row);
    }
    if options.sheet {
        let pm = sheet(&rows, options.scale).ok_or("the contact sheet is too large")?;
        save(
            &pm,
            &options.out,
            &sheet_name(A::NAME, surface, look(false), options.scale),
        )?;
    }
    Ok(())
}

/// The `main` of an app's capture example: the process's arguments carried out, a message and exit status 2 on an
/// error.
pub fn run<A: App>(app: impl FnOnce() -> Result<A, String>) -> ExitCode {
    let result = parse(A::NAME, A::STATE_FLAGS, std::env::args().skip(1))
        .and_then(|options| capture(&mut app()?, &options));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("capture: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests;
