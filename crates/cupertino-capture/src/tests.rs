use super::*;

fn args(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

fn options(line: &str) -> Result<Options, String> {
    parse("settings", &["login", "view"], args(line))
}

const LIGHT: Look = Look {
    dark: false,
    increase_contrast: false,
    reduce_transparency: false,
};

#[test]
fn a_surface_alone_is_both_appearances_at_one_x_into_target_captures() {
    assert_eq!(
        options("settings/accessibility"),
        Ok(Options {
            surface: Some("accessibility".into()),
            state: State::new(),
            scale: 1.0,
            dark: vec![false, true],
            increase_contrast: false,
            reduce_transparency: false,
            size: None,
            out: PathBuf::from("target/captures"),
            sheet: false,
        })
    );
}

#[test]
fn every_flag_is_read_and_state_flags_keep_the_apps_order() {
    let o = options(
        "--view list settings/files --scale 2 --appearance dark --increase-contrast --reduce-transparency \
         --size 440x360 --out shots --sheet --login saving",
    )
    .unwrap();
    assert_eq!(o.surface.as_deref(), Some("files"));
    assert_eq!(
        o.state,
        vec![
            ("login".to_string(), "saving".to_string()),
            ("view".to_string(), "list".to_string())
        ]
    );
    assert_eq!(
        (o.scale, o.dark, o.size, o.out, o.sheet),
        (
            2.0,
            vec![true],
            Some((440.0, 360.0)),
            PathBuf::from("shots"),
            true
        )
    );
    assert!(o.increase_contrast && o.reduce_transparency);
}

#[test]
fn a_repeated_state_flag_keeps_its_last_value() {
    let o = options("settings/accessibility --login saving --login refused").unwrap();
    assert_eq!(o.state, vec![("login".to_string(), "refused".to_string())]);
}

#[test]
fn bad_command_lines_say_what_is_wrong() {
    let cases = [
        ("", USAGE),
        (
            "files/window",
            "files/window: the surface is settings/SURFACE",
        ),
        ("settings/", "settings/: the surface is settings/SURFACE"),
        ("settings/a settings/b", "settings/b: one surface at a time"),
        (
            "settings/a --drop-target Home",
            "settings has no flag --drop-target",
        ),
        ("settings/a --scale 0", "--scale 0: not a positive number"),
        ("settings/a --scale", "--scale needs a value"),
        (
            "settings/a --appearance dim",
            "--appearance dim: light, dark or both",
        ),
        (
            "settings/a --size 440",
            "--size 440: WIDTHxHEIGHT in points",
        ),
        (
            "settings/a --size 0x10",
            "--size 0x10: WIDTHxHEIGHT in points",
        ),
        ("settings/a settings", "settings: one surface at a time"),
        ("settings/a --list", "settings has no flag --list"),
    ];
    for (line, error) in cases {
        assert_eq!(options(line), Err(error.to_string()), "{line:?}");
    }
}

#[test]
fn file_names_carry_the_state_appearance_settings_and_scale() {
    let state = vec![("login".to_string(), "saving".to_string())];
    assert_eq!(
        file_name(
            "settings",
            "accessibility",
            &state,
            Look {
                dark: true,
                ..LIGHT
            },
            2.0
        ),
        "settings-accessibility-login-saving-dark@2x.png"
    );
    assert_eq!(
        file_name("settings", "accessibility", &State::new(), LIGHT, 1.0),
        "settings-accessibility-light@1x.png"
    );
    let drop = vec![("drop-target".to_string(), "My Files (old)".to_string())];
    let look = Look {
        increase_contrast: true,
        reduce_transparency: true,
        ..LIGHT
    };
    assert_eq!(
        file_name("files", "sidebar", &drop, look, 1.5),
        "files-sidebar-drop-target-my-files-old-light-hc-rt@1.5x.png"
    );
    assert_eq!(
        sheet_name("files", "sidebar", look, 2.0),
        "files-sidebar-sheet-hc-rt@2x.png"
    );
}

fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Pixmap {
    let mut pm = Pixmap::new(w, h).unwrap();
    pm.fill(Color::from_rgba8(rgb[0], rgb[1], rgb[2], 255));
    pm
}

fn rgb(pm: &Pixmap, x: u32, y: u32) -> [u8; 3] {
    let p = pm.pixel(x, y).unwrap();
    [p.red(), p.green(), p.blue()]
}

#[test]
fn a_sheet_puts_each_cell_at_the_top_left_of_an_equal_slot() {
    let rows = vec![
        vec![solid(10, 4, [255, 0, 0]), solid(6, 6, [0, 255, 0])],
        vec![solid(10, 4, [0, 0, 255])],
    ];
    let pm = sheet(&rows, 0.5).unwrap();
    // 8-pixel gaps around two 10x6 slots across and two down
    assert_eq!((pm.width(), pm.height()), (8 + 2 * 18, 8 + 2 * 14));
    assert_eq!(rgb(&pm, 8, 8), [255, 0, 0]);
    assert_eq!(rgb(&pm, 17, 11), [255, 0, 0]);
    assert_eq!(rgb(&pm, 8, 12), SHEET_BACKGROUND, "below the short cell");
    assert_eq!(rgb(&pm, 26, 13), [0, 255, 0]);
    assert_eq!(rgb(&pm, 8, 22), [0, 0, 255]);
    assert_eq!(rgb(&pm, 26, 22), SHEET_BACKGROUND, "the empty slot");
    assert_eq!(rgb(&pm, 0, 0), SHEET_BACKGROUND);
}

/// What [`Fake`] was asked to draw: the surface, state, look, scale and size.
type Drawn = (String, State, Look, f32, Option<(f64, f64)>);

/// Draws each state as a 3x2-point fill: red by default, blue for `login saving`; `nope` is no surface it can draw.
struct Fake {
    drawn: Vec<Drawn>,
}

impl App for Fake {
    const NAME: &'static str = "settings";
    const STATE_FLAGS: &'static [&'static str] = &["login"];

    fn surfaces(&self) -> Vec<String> {
        vec!["accessibility".into(), "nope".into()]
    }

    fn sheet_states(&self, _: &str) -> Vec<State> {
        vec![
            State::new(),
            vec![("login".to_string(), "saving".to_string())],
        ]
    }

    fn capture(&mut self, r: &Request<'_>) -> Result<Pixmap, String> {
        if r.surface == "nope" {
            return Err("nope cannot be drawn".into());
        }
        self.drawn
            .push((r.surface.into(), r.state.clone(), r.look, r.scale, r.size));
        let color = if r.state.is_empty() {
            [255, 0, 0]
        } else {
            [0, 0, 255]
        };
        Ok(solid((3.0 * r.scale) as u32, (2.0 * r.scale) as u32, color))
    }
}

fn written(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn a_capture_writes_one_png_per_appearance_of_the_asked_state() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("new");
    let mut app = Fake { drawn: Vec::new() };
    let mut o = options("settings/accessibility --scale 2 --login saving --size 30x20").unwrap();
    o.out = out.clone();
    capture(&mut app, &o).unwrap();
    assert_eq!(
        written(&out),
        [
            "settings-accessibility-login-saving-dark@2x.png",
            "settings-accessibility-login-saving-light@2x.png"
        ]
    );
    let saving = vec![("login".to_string(), "saving".to_string())];
    assert_eq!(
        app.drawn,
        [
            (
                "accessibility".into(),
                saving.clone(),
                LIGHT,
                2.0,
                Some((30.0, 20.0))
            ),
            (
                "accessibility".into(),
                saving,
                Look {
                    dark: true,
                    ..LIGHT
                },
                2.0,
                Some((30.0, 20.0))
            ),
        ]
    );
    let png =
        Pixmap::load_png(out.join("settings-accessibility-login-saving-light@2x.png")).unwrap();
    assert_eq!(
        (png.width(), png.height(), rgb(&png, 5, 3)),
        (6, 4, [0, 0, 255])
    );
}

#[test]
fn a_sheet_draws_every_state_in_every_asked_appearance_and_lays_them_out() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = Fake { drawn: Vec::new() };
    let mut o = options("settings/accessibility --sheet --login refused").unwrap();
    o.out = dir.path().to_path_buf();
    capture(&mut app, &o).unwrap();
    assert_eq!(
        written(dir.path()),
        [
            "settings-accessibility-dark@1x.png",
            "settings-accessibility-light@1x.png",
            "settings-accessibility-login-saving-dark@1x.png",
            "settings-accessibility-login-saving-light@1x.png",
            "settings-accessibility-sheet@1x.png",
        ]
    );
    let pm = Pixmap::load_png(dir.path().join("settings-accessibility-sheet@1x.png")).unwrap();
    // 16-pixel gaps around two 3x2 slots across (default, saving) and two down (light, dark)
    assert_eq!((pm.width(), pm.height()), (16 + 2 * 19, 16 + 2 * 18));
    assert_eq!(rgb(&pm, 16, 34), [255, 0, 0]);
    assert_eq!(rgb(&pm, 35, 34), [0, 0, 255]);
}

#[test]
fn list_prints_and_unknown_or_undrawable_surfaces_fail() {
    let mut app = Fake { drawn: Vec::new() };
    let list = parse("settings", &["login"], args("settings")).unwrap();
    assert_eq!(list.surface, None);
    assert_eq!(capture(&mut app, &list), Ok(()));
    let dir = tempfile::tempdir().unwrap();
    for (surface, error) in [
        (
            "settings/wifi",
            "settings/wifi: not a surface (`capture settings` lists them)",
        ),
        ("settings/nope", "nope cannot be drawn"),
    ] {
        let mut o = options(surface).unwrap();
        o.out = dir.path().to_path_buf();
        assert_eq!(capture(&mut app, &o), Err(error.to_string()));
    }
    assert!(app.drawn.is_empty());
}
