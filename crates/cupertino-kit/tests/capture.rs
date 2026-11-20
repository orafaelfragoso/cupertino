//! The capture command's shared part: how a command line is read, the names of the pictures it writes, the contact
//! sheet, what carrying it out draws and writes, and the picture of a window's last frame.

#[allow(dead_code)]
mod support;

use std::num::NonZeroU32;
use std::path::PathBuf;

use cupertino_kit::core::CommandRegistry;
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{DecorationMode, Timestamp};
use cupertino_kit::prelude::*;
use cupertino_kit::render::PixelFormat;
use cupertino_kit::test::capture::{
    capture, file_name, parse, presented, sheet, sheet_name, App, Look, Options, Request, State,
};
use cupertino_kit::test::WindowStates;
use tiny_skia::{Color, Pixmap};

fn args(line: &str) -> Vec<String> {
    line.split_whitespace().map(String::from).collect()
}

fn state(pairs: &[(&str, &str)]) -> State {
    pairs
        .iter()
        .map(|(flag, value)| (flag.to_string(), value.to_string()))
        .collect()
}

#[test]
fn a_command_line_names_a_surface_its_looks_and_the_states_in_the_order_its_app_takes_them() {
    let parsed = |line: &str| parse("files", &["view", "selected"], args(line));

    let plain = parsed("files/window").unwrap();
    assert_eq!(plain.surface.as_deref(), Some("window"));
    assert_eq!(
        (plain.scale, plain.dark.as_slice(), plain.sheet),
        (1.0, [false, true].as_slice(), false)
    );
    assert!(plain.state.is_empty() && plain.size.is_none() && !plain.increase_contrast);

    let full = parsed(
        "files/window --scale 2 --appearance dark --increase-contrast --reduce-transparency \
         --size 800x600 --out /tmp/out --selected a.txt --view list --view icon",
    )
    .unwrap();
    assert_eq!(
        full,
        Options {
            surface: Some("window".into()),
            state: state(&[("view", "icon"), ("selected", "a.txt")]),
            scale: 2.0,
            dark: vec![true],
            increase_contrast: true,
            reduce_transparency: true,
            size: Some((800.0, 600.0)),
            out: PathBuf::from("/tmp/out"),
            sheet: false,
        },
        "the state in the app's order, a flag given twice as it was last given"
    );

    let listing = parsed("files").unwrap();
    assert_eq!(listing.surface, None, "the app alone lists its surfaces");
    assert_eq!(
        parsed("files/window --appearance light --sheet")
            .unwrap()
            .dark,
        [false]
    );
    assert!(parsed("files/window --sheet").unwrap().sheet);
}

#[test]
fn a_command_line_that_cannot_be_carried_out_is_refused_with_what_is_wrong_with_it() {
    let rows = [
        ("nothing", "", "usage"),
        ("a scale of no size", "files/window --scale 0", "--scale 0"),
        (
            "a scale that is no number",
            "files/window --scale big",
            "--scale big",
        ),
        ("a missing value", "files/window --scale", "needs a value"),
        (
            "an appearance not there",
            "files/window --appearance purple",
            "--appearance purple",
        ),
        (
            "a size without a height",
            "files/window --size 800",
            "--size 800",
        ),
        (
            "a size of no area",
            "files/window --size 0x600",
            "--size 0x600",
        ),
        (
            "a flag the app does not take",
            "files/window --bogus 1",
            "no flag --bogus",
        ),
        ("another app's surface", "settings/window", "files/SURFACE"),
        ("a surface of no name", "files/", "files/SURFACE"),
        (
            "two surfaces",
            "files/window files/sidebar",
            "one surface at a time",
        ),
        (
            "a surface and the listing",
            "files files/window",
            "one surface at a time",
        ),
        (
            "a sheet of a chosen state",
            "files/window --sheet --view list",
            "--sheet",
        ),
    ];
    for (what, line, expected) in rows {
        let error = parse("files", &["view"], args(line)).expect_err(what);
        assert!(error.contains(expected), "{what}: {error}");
    }
}

#[test]
fn a_picture_is_named_by_its_app_surface_state_look_and_scale() {
    let look = |dark, increase_contrast, reduce_transparency| Look {
        dark,
        increase_contrast,
        reduce_transparency,
    };
    let plain = look(false, false, false);
    let rows = [
        (state(&[]), plain, 1.0, "files-window-light@1x.png"),
        (
            state(&[("view", "list"), ("selected", "My File.txt")]),
            look(true, true, false),
            2.0,
            "files-window-view-list-selected-my-file-txt-dark-hc@2x.png",
        ),
        (
            state(&[("view", "icon")]),
            look(false, true, true),
            1.5,
            "files-window-view-icon-light-hc-rt@1.5x.png",
        ),
    ];
    for (state, look, scale, expected) in rows {
        assert_eq!(file_name("files", "window", &state, look, scale), expected);
    }
    assert_eq!(
        sheet_name("files", "window", plain, 2.0),
        "files-window-sheet@2x.png"
    );
    assert_eq!(
        sheet_name("files", "window", look(true, true, true), 1.0),
        "files-window-sheet-hc-rt@1x.png"
    );
}

fn pixmap(width: u32, height: u32, color: Color) -> Pixmap {
    let mut pixmap = Pixmap::new(width, height).unwrap();
    pixmap.fill(color);
    pixmap
}

fn at(pixmap: &Pixmap, x: u32, y: u32) -> [u8; 3] {
    let p = pixmap.pixel(x, y).unwrap();
    [p.red(), p.green(), p.blue()]
}

#[test]
fn a_contact_sheet_sets_its_cells_in_rows_in_slots_as_large_as_the_largest_with_a_gap_of_the_scale()
{
    let red = Color::from_rgba8(255, 0, 0, 255);
    let blue = Color::from_rgba8(0, 0, 255, 255);
    let rows = vec![
        vec![pixmap(10, 6, red), pixmap(4, 4, blue)],
        vec![pixmap(4, 8, blue)],
    ];
    let laid = sheet(&rows, 1.0).unwrap();
    let gap = 16;
    assert_eq!(
        (laid.width(), laid.height()),
        (gap + 2 * (10 + gap), gap + 2 * (8 + gap)),
        "two slots of the largest cell across and down, a gap around each"
    );
    let grey = [128, 128, 128];
    assert_eq!(at(&laid, 0, 0), grey);
    assert_eq!(at(&laid, gap, gap), [255, 0, 0], "the first cell's corner");
    assert_eq!(at(&laid, gap + 9, gap + 5), [255, 0, 0]);
    assert_eq!(at(&laid, gap + 10, gap), grey, "between the cells");
    assert_eq!(
        at(&laid, gap + 10 + gap, gap),
        [0, 0, 255],
        "the second slot"
    );
    assert_eq!(
        at(&laid, gap + 10 + gap, gap + 5),
        grey,
        "past the cell, in its slot"
    );
    assert_eq!(at(&laid, gap, gap + 8 + gap), [0, 0, 255], "the second row");

    // the gap is the scale's, and no cells make no sheet
    let cell = pixmap(4, 4, Color::from_rgba8(0, 255, 0, 255));
    let rows = vec![vec![cell]];
    let sheet2 = sheet(&rows, 2.0).unwrap();
    assert_eq!(
        (sheet2.width(), sheet2.height()),
        (32 + 4 + 32, 32 + 4 + 32)
    );
    assert!(sheet(&[], 1.0).is_none());
    assert!(sheet(&[Vec::new()], 1.0).is_none());
}

/// What a capture asked of the app: the surface, the state, whether dark and the size.
type Asked = (String, State, bool, Option<(f64, f64)>);

/// An app of one surface that draws a picture as large as its scale says, red in the dark appearance, and remembers
/// what it was asked for.
struct Demo {
    asked: Vec<Asked>,
}

impl App for Demo {
    const NAME: &'static str = "demo";
    const STATE_FLAGS: &'static [&'static str] = &["mode"];

    fn surfaces(&self) -> Vec<String> {
        vec!["main".into()]
    }

    fn sheet_states(&self, _: &str) -> Vec<State> {
        vec![state(&[]), state(&[("mode", "b")])]
    }

    fn capture(&mut self, request: &Request<'_>) -> Result<Pixmap, String> {
        self.asked.push((
            request.surface.to_string(),
            request.state.clone(),
            request.look.dark,
            request.size,
        ));
        if request.state.iter().any(|(_, value)| value == "broken") {
            return Err("cannot draw that state".into());
        }
        let side = (4.0 * request.scale) as u32;
        let color = if request.look.dark {
            Color::from_rgba8(255, 0, 0, 255)
        } else {
            Color::from_rgba8(255, 255, 255, 255)
        };
        Ok(pixmap(side, side, color))
    }
}

/// A folder of the test's own under the target directory, removed when the test is done.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("kit-capture-{test}"));
        let _ = std::fs::remove_dir_all(&root);
        Scratch(root)
    }

    fn files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .map(|entries| {
                entries
                    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn carried_out(scratch: &Scratch, line: &str) -> (Result<(), String>, Demo) {
    let out = scratch.0.display();
    let options = parse("demo", &["mode"], args(&format!("{line} --out {out}"))).unwrap();
    let mut demo = Demo { asked: Vec::new() };
    (capture(&mut demo, &options), demo)
}

#[test]
fn a_capture_writes_one_picture_a_state_and_appearance_and_with_a_sheet_the_contact_sheet_too() {
    let scratch = Scratch::new("writes");
    let (done, demo) = carried_out(&scratch, "demo/main --scale 2 --size 300x200 --mode a");
    assert_eq!(done, Ok(()));
    assert_eq!(
        scratch.files(),
        [
            "demo-main-mode-a-dark@2x.png",
            "demo-main-mode-a-light@2x.png"
        ]
    );
    assert_eq!(
        demo.asked,
        [
            (
                "main".to_string(),
                state(&[("mode", "a")]),
                false,
                Some((300.0, 200.0))
            ),
            (
                "main".to_string(),
                state(&[("mode", "a")]),
                true,
                Some((300.0, 200.0))
            ),
        ],
        "light before dark, with the size and state asked for"
    );
    let drawn = Pixmap::load_png(scratch.0.join("demo-main-mode-a-dark@2x.png")).unwrap();
    assert_eq!((drawn.width(), at(&drawn, 0, 0)), (8, [255, 0, 0]));

    let scratch = Scratch::new("sheet");
    let (done, demo) = carried_out(&scratch, "demo/main --sheet --appearance dark");
    assert_eq!(done, Ok(()));
    assert_eq!(
        scratch.files(),
        [
            "demo-main-dark@1x.png",
            "demo-main-mode-b-dark@1x.png",
            "demo-main-sheet@1x.png"
        ]
    );
    assert_eq!(
        demo.asked.len(),
        2,
        "every state the app lists, in the appearances asked for"
    );
    let contact = Pixmap::load_png(scratch.0.join("demo-main-sheet@1x.png")).unwrap();
    assert_eq!(
        (contact.width(), contact.height()),
        (16 + 2 * (4 + 16), 16 + 4 + 16)
    );
}

#[test]
fn a_capture_of_a_surface_the_app_lacks_or_a_state_it_cannot_draw_fails_with_what_it_could_not_do()
{
    let scratch = Scratch::new("fails");
    let (done, demo) = carried_out(&scratch, "demo/nowhere");
    assert!(done.unwrap_err().contains("not a surface"));
    assert!(demo.asked.is_empty());
    assert!(scratch.files().is_empty(), "nothing is written for it");

    let (done, _) = carried_out(&scratch, "demo/main --mode broken");
    assert_eq!(done, Err("cannot draw that state".into()));
}

/// One window over the theme's background with a labelled button, whose text has smoothed edges.
struct Plain;

impl Application for Plain {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update((): &mut (), (): ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view((): &()) -> Vec<Window<()>> {
        vec![Window {
            key: WindowKey("plain".into()),
            spec: WindowSpec {
                title: "Plain".into(),
                app_id: "org.cupertino.tests.Plain".into(),
                size: Size::new(120.0, 60.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(Button {
                label: "Plain".into(),
                role: ButtonRole::Normal,
                style: ButtonStyle::Bordered,
                enabled: true,
                busy: false,
                on_press: None,
            }),
            menus: None,
        }]
    }
}

#[test]
fn the_picture_of_a_window_is_its_last_frame_with_its_pixels_premultiplied() {
    let turns = NonZeroU32::new(1000).unwrap();
    let mut driver = support::driver::<Plain>(support::text_system());
    driver.run_until_idle(turns).unwrap();
    let window = driver.runtime().windows()[0].1;
    assert_eq!(
        presented(driver.runtime().window_system(), window).unwrap_err(),
        "the window presented no frame"
    );

    let system = driver.window_system_mut();
    system
        .configure(window, None, WindowStates::default())
        .unwrap();
    for _ in 0..16 {
        driver.run_until_idle(turns).unwrap();
        let system = driver.window_system_mut();
        if !system.frame_requested(window).unwrap() {
            break;
        }
        system.frame_ready(window, Timestamp(0)).unwrap();
    }
    let system = driver.runtime().window_system();
    let frame = system.frames(window).unwrap().last().unwrap().clone();
    let picture = presented(system, window).unwrap();
    assert_eq!(
        (picture.width(), picture.height()),
        (frame.width, frame.height)
    );
    let drawn = system
        .composited(window)
        .unwrap()
        .unwrap_or_else(|| frame.pixels.clone());
    let mut partly_covered = 0;
    for (got, drawn) in picture.data().chunks(4).zip(drawn.chunks(4)) {
        let a = u16::from(drawn[3]);
        assert_eq!(got[3], drawn[3], "the alpha is the frame's");
        partly_covered += usize::from(a > 0 && a < 255);
        for c in 0..3 {
            let want = match frame.format {
                PixelFormat::Rgba8 => ((u16::from(drawn[c]) * a + 127) / 255) as u8,
                _ => drawn[c],
            };
            assert_eq!(got[c], want, "{:?} {drawn:?}", frame.format);
        }
    }
    assert!(
        frame.format != PixelFormat::Rgba8 || partly_covered > 0,
        "the window has partly covered pixels, which premultiplying changes"
    );
}
