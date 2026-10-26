use cupertino_gtk_theme::cssck::{self, parse_color};
use cupertino_gtk_theme::generate::{generate, write_files, GeneratedFile, GeneratorOptions};
use cupertino_gtk_theme::qt::{self, QT_ROLES};
use cupertino_gtk_theme::{Accent, ControlSizeName};
use cupertino_theme::{Appearance, SemanticColor, Theme};
use std::path::Path;
use std::process::Command;

fn theme() -> Theme {
    Theme::load().unwrap()
}

fn files(o: &GeneratorOptions) -> Vec<GeneratedFile> {
    generate(&theme(), o)
}

fn get<'a>(f: &'a [GeneratedFile], path: &str) -> &'a str {
    &f.iter()
        .find(|x| x.path == Path::new(path))
        .unwrap_or_else(|| panic!("{path} not generated"))
        .contents
}

fn rgba(sheet: &cssck::Stylesheet, name: &str) -> [u8; 4] {
    parse_color(&sheet.defines[name])
        .unwrap_or_else(|| panic!("{name} = {:?} is not a colour", sheet.defines[name]))
}

const LIBADWAITA_NAMES: [&str; 53] = [
    "accent_bg_color",
    "accent_fg_color",
    "accent_color",
    "destructive_bg_color",
    "destructive_fg_color",
    "destructive_color",
    "success_bg_color",
    "success_fg_color",
    "success_color",
    "warning_bg_color",
    "warning_fg_color",
    "warning_color",
    "error_bg_color",
    "error_fg_color",
    "error_color",
    "window_bg_color",
    "window_fg_color",
    "view_bg_color",
    "view_fg_color",
    "headerbar_bg_color",
    "headerbar_fg_color",
    "headerbar_border_color",
    "headerbar_backdrop_color",
    "headerbar_shade_color",
    "headerbar_darker_shade_color",
    "card_bg_color",
    "card_fg_color",
    "card_shade_color",
    "thumbnail_bg_color",
    "thumbnail_fg_color",
    "dialog_bg_color",
    "dialog_fg_color",
    "popover_bg_color",
    "popover_fg_color",
    "popover_shade_color",
    "shade_color",
    "scrollbar_outline_color",
    "sidebar_bg_color",
    "sidebar_fg_color",
    "sidebar_backdrop_color",
    "sidebar_shade_color",
    "sidebar_border_color",
    "secondary_sidebar_bg_color",
    "secondary_sidebar_fg_color",
    "secondary_sidebar_backdrop_color",
    "secondary_sidebar_shade_color",
    "secondary_sidebar_border_color",
    "theme_bg_color",
    "theme_fg_color",
    "theme_base_color",
    "theme_text_color",
    "theme_selected_bg_color",
    "theme_selected_fg_color",
];

#[test]
fn every_generated_css_file_is_well_formed() {
    let all = files(&GeneratorOptions::default());
    let css: Vec<&GeneratedFile> = all
        .iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "css"))
        .collect();
    assert_eq!(
        css.len(),
        2 * 4 + 2,
        "gtk3 and gtk4 theme files for light and dark, plus two libadwaita overrides"
    );
    for f in css {
        let sheet =
            cssck::parse(&f.contents).unwrap_or_else(|e| panic!("{}: {e}", f.path.display()));
        assert!(
            !sheet.rules.is_empty() && !sheet.defines.is_empty(),
            "{}",
            f.path.display()
        );
        for name in sheet.referenced_colors() {
            assert!(
                sheet.defines.contains_key(&name),
                "{}: @{name} is used but not defined",
                f.path.display()
            );
        }
        for (name, value) in &sheet.defines {
            if !value.starts_with('@') {
                assert!(
                    parse_color(value).is_some(),
                    "{}: @define-color {name} {value}",
                    f.path.display()
                );
            }
        }
    }
}

#[test]
fn libadwaita_override_defines_every_libadwaita_name_with_token_colours() {
    let t = theme();
    let all = files(&GeneratorOptions::default());
    let light = cssck::parse(get(&all, "config/gtk-4.0/gtk.css")).unwrap();
    let dark = cssck::parse(get(&all, "config/gtk-4.0/gtk-dark.css")).unwrap();
    for n in LIBADWAITA_NAMES {
        assert!(
            light.defines.contains_key(n) && dark.defines.contains_key(n),
            "@{n}"
        );
    }
    let tok = |a: Appearance, c: SemanticColor| t.tokens.semantic(a, c).unwrap().to_rgba8();
    assert_eq!(
        rgba(&light, "window_bg_color"),
        tok(Appearance::Light, SemanticColor::WindowBackground)
    );
    assert_eq!(
        rgba(&dark, "window_bg_color"),
        tok(Appearance::Dark, SemanticColor::WindowBackground)
    );
    assert_eq!(
        rgba(&light, "view_bg_color"),
        tok(Appearance::Light, SemanticColor::ControlBackground)
    );
    assert_eq!(
        rgba(&dark, "view_bg_color"),
        tok(Appearance::Dark, SemanticColor::ControlBackground)
    );
    assert_eq!(
        rgba(&light, "window_fg_color"),
        tok(Appearance::Light, SemanticColor::Label)
    );
    assert_eq!(
        rgba(&dark, "destructive_bg_color"),
        tok(Appearance::Dark, SemanticColor::SystemRed)
    );
    assert_eq!(
        rgba(&light, "success_bg_color"),
        tok(Appearance::Light, SemanticColor::SystemGreen)
    );
    assert_eq!(
        rgba(&light, "cupertino_focus_ring"),
        tok(Appearance::Light, SemanticColor::KeyboardFocusIndicator)
    );
    assert_eq!(
        rgba(&dark, "cupertino_focus_ring"),
        tok(Appearance::Dark, SemanticColor::KeyboardFocusIndicator)
    );
    assert_eq!(
        rgba(&light, "cupertino_link"),
        tok(Appearance::Light, SemanticColor::Link)
    );
    assert_eq!(light.defines["theme_bg_color"], "@window_bg_color");
}

#[test]
fn accent_follows_the_selected_token() {
    let t = theme();
    for a in Accent::ALL {
        let f = files(&GeneratorOptions {
            accent: a,
            ..Default::default()
        });
        let light = cssck::parse(get(&f, "config/gtk-4.0/gtk.css")).unwrap();
        let dark = cssck::parse(get(&f, "config/gtk-4.0/gtk-dark.css")).unwrap();
        assert_eq!(
            rgba(&light, "accent_bg_color"),
            t.tokens
                .color(Appearance::Light, a.token())
                .unwrap()
                .to_rgba8(),
            "{}",
            a.name()
        );
        assert_eq!(
            rgba(&dark, "accent_bg_color"),
            t.tokens
                .color(Appearance::Dark, a.token())
                .unwrap()
                .to_rgba8(),
            "{}",
            a.name()
        );
        assert_eq!(
            rgba(&light, "accent_color"),
            rgba(&light, "accent_bg_color")
        );
        assert_eq!(light.defines["theme_selected_bg_color"], "@accent_bg_color");
    }
    let blue = files(&GeneratorOptions::default());
    assert_eq!(
        rgba(
            &cssck::parse(get(&blue, "config/gtk-4.0/gtk.css")).unwrap(),
            "accent_bg_color"
        ),
        [0, 122, 255, 255]
    );
}

#[test]
fn control_heights_radii_and_focus_ring_come_from_the_metrics() {
    let f = files(&GeneratorOptions::default());
    for path in [
        "config/gtk-4.0/gtk.css",
        "themes/Cupertino/gtk-3.0/gtk.css",
        "themes/Cupertino/gtk-4.0/gtk.css",
    ] {
        let s = cssck::parse(get(&f, path)).unwrap();
        assert_eq!(
            s.values("button", "min-height"),
            vec!["18px"],
            "{path}: 20 pt alignment height minus the 1 px borders"
        );
        assert_eq!(s.values("button", "border-radius"), vec!["6px"]);
        assert_eq!(s.values("check", "min-width"), vec!["14px"]);
        assert_eq!(s.values("switch", "min-width"), vec!["38px"]);
        assert_eq!(s.values("switch", "min-height"), vec!["22px"]);
        assert_eq!(s.values("scale trough", "min-height"), vec!["4px"]);
        assert_eq!(
            s.values("headerbar", "background-color"),
            vec!["@headerbar_bg_color"]
        );
        assert_eq!(
            s.values("*:selected", "background-color"),
            vec!["@theme_selected_bg_color"]
        );
        assert!(s
            .values("button:focus", "outline")
            .first()
            .or(s.values("button:focus-visible", "outline").first())
            .unwrap()
            .starts_with("3px solid @cupertino_focus_ring"));
    }
    let gtk3 = cssck::parse(get(&f, "themes/Cupertino/gtk-3.0/gtk.css")).unwrap();
    assert_eq!(
        gtk3.values("decoration", "border-radius"),
        vec!["12px 12px 0 0"]
    );
    let gtk4 = cssck::parse(get(&f, "themes/Cupertino/gtk-4.0/gtk.css")).unwrap();
    assert_eq!(
        gtk4.values("window.csd", "border-radius"),
        vec!["12px 12px 0 0"]
    );
    assert_eq!(
        gtk4.values("modelbutton:hover", "border-radius"),
        vec!["4px"],
        "menu selection radius"
    );
}

#[test]
fn control_size_option_changes_heights() {
    let f = files(&GeneratorOptions {
        size: ControlSizeName::Large,
        ..Default::default()
    });
    let s = cssck::parse(get(&f, "config/gtk-4.0/gtk.css")).unwrap();
    assert_eq!(s.values("button", "min-height"), vec!["26px"]);
    let f = files(&GeneratorOptions {
        size: ControlSizeName::Small,
        ..Default::default()
    });
    assert_eq!(
        cssck::parse(get(&f, "config/gtk-4.0/gtk.css"))
            .unwrap()
            .values("button", "min-height"),
        vec!["14px"]
    );
}

#[test]
fn theme_directories_import_the_builtin_base_themes() {
    let f = files(&GeneratorOptions::default());
    let l3 = cssck::parse(get(&f, "themes/Cupertino/gtk-3.0/gtk.css")).unwrap();
    let d3 = cssck::parse(get(&f, "themes/Cupertino-dark/gtk-3.0/gtk.css")).unwrap();
    assert_eq!(
        l3.imports,
        vec!["resource:///org/gtk/libgtk/theme/Adwaita/gtk-contained.css"]
    );
    assert_eq!(
        d3.imports,
        vec!["resource:///org/gtk/libgtk/theme/Adwaita/gtk-contained-dark.css"]
    );
    let l4 = cssck::parse(get(&f, "themes/Cupertino/gtk-4.0/gtk.css")).unwrap();
    let d4 = cssck::parse(get(&f, "themes/Cupertino-dark/gtk-4.0/gtk.css")).unwrap();
    assert_eq!(
        l4.imports,
        vec!["resource:///org/gtk/libgtk/theme/Default/Default-light.css"]
    );
    assert_eq!(
        d4.imports,
        vec!["resource:///org/gtk/libgtk/theme/Default/Default-dark.css"]
    );
    assert_eq!(rgba(&d3, "theme_bg_color"), [0x32, 0x32, 0x32, 255]);
    assert_eq!(rgba(&l3, "theme_bg_color"), [0xEC, 0xEC, 0xEC, 255]);
    let light_dark = cssck::parse(get(&f, "themes/Cupertino/gtk-3.0/gtk-dark.css")).unwrap();
    assert_eq!(
        rgba(&light_dark, "theme_bg_color"),
        [0x32, 0x32, 0x32, 255],
        "gtk-dark.css of the light theme is the dark appearance"
    );
    assert!(get(&f, "themes/Cupertino/index.theme").contains("GtkTheme=Cupertino"));
    assert!(get(&f, "themes/Cupertino-dark/index.theme").contains("GtkTheme=Cupertino-dark"));
}

#[test]
fn kde_colour_scheme_matches_tokens() {
    let t = theme();
    let f = files(&GeneratorOptions::default());
    let light = qt::parse_kde(get(&f, "qt/color-schemes/CupertinoLight.colors"));
    let dark = qt::parse_kde(get(&f, "qt/color-schemes/CupertinoDark.colors"));
    let v = |m: &std::collections::BTreeMap<(String, String), String>, s: &str, k: &str| {
        m[&(s.to_string(), k.to_string())].clone()
    };
    let rgb = |c: cupertino_theme::Color| {
        let [r, g, b, _] = c.to_rgba8();
        format!("{r},{g},{b}")
    };
    assert_eq!(
        v(&light, "Colors:Window", "BackgroundNormal"),
        rgb(t
            .tokens
            .semantic(Appearance::Light, SemanticColor::WindowBackground)
            .unwrap())
    );
    assert_eq!(
        v(&dark, "Colors:Window", "BackgroundNormal"),
        rgb(t
            .tokens
            .semantic(Appearance::Dark, SemanticColor::WindowBackground)
            .unwrap())
    );
    assert_eq!(v(&light, "Colors:View", "BackgroundNormal"), "255,255,255");
    assert_eq!(v(&dark, "Colors:View", "BackgroundNormal"), "30,30,30");
    assert_eq!(
        v(&light, "Colors:Selection", "BackgroundNormal"),
        "0,122,255"
    );
    assert_eq!(
        v(&light, "Colors:Selection", "ForegroundNormal"),
        "255,255,255"
    );
    assert_eq!(v(&light, "Colors:Window", "DecorationFocus"), "0,122,255");
    assert_eq!(v(&dark, "Colors:Window", "ForegroundNegative"), "255,69,58");
    assert_eq!(v(&light, "General", "ColorScheme"), "CupertinoLight");
    assert_eq!(v(&dark, "General", "Name"), "Cupertino Dark");
    assert_eq!(
        v(&light, "Colors:Complementary", "BackgroundNormal"),
        v(&dark, "Colors:Window", "BackgroundNormal"),
        "complementary is the opposite brightness"
    );
    for m in [&light, &dark] {
        for s in [
            "Button",
            "Complementary",
            "Header",
            "Selection",
            "Tooltip",
            "View",
            "Window",
        ] {
            for k in [
                "BackgroundAlternate",
                "BackgroundNormal",
                "DecorationFocus",
                "DecorationHover",
                "ForegroundActive",
                "ForegroundInactive",
                "ForegroundLink",
                "ForegroundNegative",
                "ForegroundNeutral",
                "ForegroundNormal",
                "ForegroundPositive",
                "ForegroundVisited",
            ] {
                let val = v(m, &format!("Colors:{s}"), k);
                let parts: Vec<_> = val.split(',').collect();
                assert_eq!(parts.len(), 3, "{s}/{k} = {val}");
                assert!(parts.iter().all(|p| p.parse::<u8>().is_ok()));
            }
        }
        assert!(m.contains_key(&("WM".to_string(), "activeBackground".to_string())));
        assert!(m.contains_key(&(
            "ColorEffects:Disabled".to_string(),
            "ContrastAmount".to_string()
        )));
    }
}

#[test]
fn qt_palette_scheme_has_the_21_roles_in_order() {
    let t = theme();
    let f = files(&GeneratorOptions::default());
    for (file, appearance) in [
        ("qt/qt6ct/colors/CupertinoLight.conf", Appearance::Light),
        ("qt/qt5ct/colors/CupertinoDark.conf", Appearance::Dark),
    ] {
        let text = get(&f, file);
        assert!(text.starts_with("[ColorScheme]\n"));
        let line = |key: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(key))
                .unwrap_or_else(|| panic!("{key} missing in {file}"))
                .to_string()
        };
        for g in ["active_colors=", "disabled_colors=", "inactive_colors="] {
            let colors = qt::parse_qtct_line(&line(g)).unwrap();
            assert_eq!(colors.len(), QT_ROLES.len(), "{g}");
            assert!(
                colors.iter().all(|c| c[3] == 255 || QT_ROLES.len() == 21),
                "palette entries are opaque"
            );
        }
        let active = qt::parse_qtct_line(&line("active_colors=")).unwrap();
        let idx = |name: &str| QT_ROLES.iter().position(|r| *r == name).unwrap();
        let wb = t
            .tokens
            .semantic(appearance, SemanticColor::WindowBackground)
            .unwrap()
            .to_rgba8();
        assert_eq!(active[idx("Window")][..3], wb[..3]);
        let accent = t
            .tokens
            .color(appearance, "control-accent-blue")
            .unwrap()
            .to_rgba8();
        assert_eq!(active[idx("Highlight")][..3], accent[..3]);
        assert_eq!(active[idx("HighlightedText")][..3], [255, 255, 255]);
        assert_eq!(
            active[idx("Base")][..3],
            t.tokens
                .semantic(appearance, SemanticColor::ControlBackground)
                .unwrap()
                .to_rgba8()[..3]
        );
        let inactive = qt::parse_qtct_line(&line("inactive_colors=")).unwrap();
        assert_ne!(
            inactive[idx("Highlight")],
            active[idx("Highlight")],
            "unfocused selection is neutral"
        );
        let disabled = qt::parse_qtct_line(&line("disabled_colors=")).unwrap();
        assert_ne!(disabled[idx("WindowText")], active[idx("WindowText")]);
    }
}

#[test]
fn fontconfig_snippet_maps_the_ui_font() {
    let f = files(&GeneratorOptions::default());
    let x = get(&f, "fontconfig/conf.d/60-cupertino-ui-font.conf");
    assert!(
        x.contains("<family>system-ui</family>")
            && x.contains("<prefer><family>Inter</family></prefer>")
    );
    let settings = get(&f, "config/gtk-4.0/settings.ini");
    assert!(settings.contains("gtk-font-name=Inter 9.5"), "{settings}");
    assert!(settings.contains("gtk-theme-name=Cupertino"));
}

#[test]
fn nothing_mentions_the_reference_vendor() {
    for f in files(&GeneratorOptions::default()) {
        let lower = f.contents.to_lowercase();
        for banned in [
            concat!("app", "le"),
            concat!("mac", "os"),
            concat!("sf", " pro"),
            concat!("coc", "oa"),
            concat!("app", "kit"),
        ] {
            assert!(
                !lower.contains(banned),
                "{} contains {banned}",
                f.path.display()
            );
        }
    }
}

#[test]
fn write_files_creates_the_tree() {
    let d = tempfile::tempdir().unwrap();
    let f = files(&GeneratorOptions::default());
    let written = write_files(&f, d.path()).unwrap();
    assert_eq!(written.len(), f.len());
    assert!(d.path().join("themes/Cupertino/gtk-4.0/gtk.css").is_file());
    assert!(d.path().join("README.txt").is_file());
    assert_eq!(
        std::fs::read_to_string(d.path().join("config/gtk-4.0/gtk.css")).unwrap(),
        get(&f, "config/gtk-4.0/gtk.css")
    );
}

#[test]
fn custom_theme_name_changes_paths_and_metadata() {
    let f = files(&GeneratorOptions {
        name: "Reference-Green".into(),
        accent: Accent::Green,
        ..Default::default()
    });
    assert!(f
        .iter()
        .any(|x| x.path == Path::new("themes/Reference-Green-dark/gtk-4.0/gtk.css")));
    assert!(f
        .iter()
        .any(|x| x.path == Path::new("qt/color-schemes/ReferenceGreenLight.colors")));
    assert!(get(&f, "themes/Reference-Green/index.theme").contains("Name=Reference-Green"));
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cupertino-gtk-theme"))
}

#[test]
fn cli_writes_only_to_the_output_directory_by_default() {
    let out = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let status = bin()
        .args(["--output", out.path().to_str().unwrap()])
        .env("HOME", home.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert!(out.path().join("themes/Cupertino/index.theme").is_file());
    assert_eq!(
        std::fs::read_dir(home.path()).unwrap().count(),
        0,
        "the home directory is untouched without --install"
    );
}

#[test]
fn cli_install_dry_run_install_and_force() {
    let out = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let run = |extra: &[&str]| {
        let mut args = vec![
            "--output",
            out.path().to_str().unwrap(),
            "--install",
            "--home",
            home.path().to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        let o = bin()
            .args(&args)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    let dry = run(&["--dry-run"]);
    assert!(dry.contains("would write"));
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    run(&[]);
    assert!(home.path().join(".config/gtk-4.0/gtk.css").is_file());
    assert!(home
        .path()
        .join(".local/share/themes/Cupertino-dark/gtk-3.0/gtk.css")
        .is_file());
    assert!(home
        .path()
        .join(".local/share/color-schemes/CupertinoDark.colors")
        .is_file());
    assert!(home
        .path()
        .join(".config/qt6ct/colors/CupertinoLight.conf")
        .is_file());
    assert!(home
        .path()
        .join(".config/fontconfig/conf.d/60-cupertino-ui-font.conf")
        .is_file());
    assert!(!home
        .path()
        .join(".config/qt6ct/qt6ct.conf.snippet")
        .exists());

    std::fs::write(home.path().join(".config/gtk-4.0/gtk.css"), "/* user */").unwrap();
    let kept = run(&["--accent", "pink"]);
    assert!(kept.contains("kept existing"));
    assert_eq!(
        std::fs::read_to_string(home.path().join(".config/gtk-4.0/gtk.css")).unwrap(),
        "/* user */"
    );
    run(&["--accent", "pink", "--force"]);
    assert_eq!(
        std::fs::read_to_string(home.path().join(".config/gtk-4.0/gtk.css.bak")).unwrap(),
        "/* user */"
    );
    assert!(
        std::fs::read_to_string(home.path().join(".config/gtk-4.0/gtk.css"))
            .unwrap()
            .contains("accent: pink")
    );
}

#[test]
fn cli_reports_errors_with_exit_code_2() {
    let o = bin().args(["--accent", "teal"]).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("unknown accent"));
    let o = bin().arg("--list-accents").output().unwrap();
    assert!(String::from_utf8_lossy(&o.stdout)
        .lines()
        .eq(Accent::ALL.iter().map(|a| a.name())));
}

#[test]
fn gtk_settings_carry_the_double_click_and_drag_thresholds() {
    let f = files(&GeneratorOptions::default());
    for path in ["config/gtk-3.0/settings.ini", "config/gtk-4.0/settings.ini"] {
        let settings = get(&f, path);
        for line in [
            "gtk-double-click-time=500",
            "gtk-double-click-distance=5",
            "gtk-dnd-drag-threshold=5",
        ] {
            assert!(
                settings.lines().any(|l| l == line),
                "{line} in {path}: {settings}"
            );
        }
    }
}
