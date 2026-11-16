//! The material inventory of the README, read as a table: every component the toolkit and the shell draw on a
//! material of the compositor's, the call that asks for it and what each display option makes of it. A row fails when
//! its file no longer holds its call (the component stopped drawing its material, or draws another), and a row that
//! draws a theme surface fails when the theme no longer does with Reduce Transparency or Increase Contrast what the
//! row says.

use cupertino_theme::Theme;
use cupertino_ui_theme::{Appearance, Surface, ThemePreferences, ThemeTokens};
use std::path::{Path, PathBuf};

const README: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/README.md");
const HEADING: &str = "## Material inventory";
const COLUMNS: &str = "| Component | Drawn in | Draws | Reduce Transparency | Increase Contrast |";

/// What a row says a display option makes of the component's material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fallback {
    /// The component still asks for it; the compositor draws the option's recipe.
    Kept,
    /// Kept in the dark appearance, not drawn in the light one.
    KeptInDark,
    /// No material: the theme's opaque window colour.
    Opaque,
    /// No material: an opaque body of the component's own.
    Painted,
}

impl Fallback {
    fn parse(cell: &str) -> Option<Fallback> {
        Some(match cell {
            "kept" => Fallback::Kept,
            "kept in dark" => Fallback::KeptInDark,
            "opaque" => Fallback::Opaque,
            "painted" => Fallback::Painted,
            _ => return None,
        })
    }

    /// Whether the component asks for its material in the `dark` or light appearance.
    fn asks(self, dark: bool) -> bool {
        match self {
            Fallback::Kept => true,
            Fallback::KeptInDark => dark,
            Fallback::Opaque | Fallback::Painted => false,
        }
    }
}

#[derive(Debug)]
struct Row {
    component: String,
    file: PathBuf,
    call: String,
    reduce_transparency: Fallback,
    increase_contrast: Fallback,
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// A cell's text without the code marks around it.
fn plain(cell: &str) -> &str {
    cell.trim().trim_matches('`')
}

/// The rows of the README's inventory.
fn inventory() -> Vec<Row> {
    let readme = std::fs::read_to_string(README).unwrap();
    let section = readme
        .split_once(HEADING)
        .unwrap_or_else(|| panic!("the README has no {HEADING:?}"))
        .1;
    let section = section.split("\n## ").next().unwrap();
    let table = section
        .split_once(COLUMNS)
        .unwrap_or_else(|| panic!("the inventory's columns are not {COLUMNS:?}"))
        .1;
    table
        .lines()
        .skip(2)
        .take_while(|line| line.starts_with('|'))
        .map(|line| {
            let cells: Vec<&str> = line.trim_matches('|').split(" | ").collect();
            let [component, file, call, reduced, contrast] = cells.as_slice() else {
                panic!("a row of five cells: {line}");
            };
            let fallback = |cell: &str| {
                Fallback::parse(plain(cell))
                    .unwrap_or_else(|| panic!("{component}: {cell:?} is no fallback"))
            };
            Row {
                component: component.trim().to_owned(),
                file: workspace().join(plain(file)),
                call: plain(call).to_owned(),
                reduce_transparency: fallback(reduced),
                increase_contrast: fallback(contrast),
            }
        })
        .collect()
}

/// The theme surface a call names. Every surface is here, so a new one must be given a name, and then a row or a
/// place in [`WITHOUT_A_ROW`].
fn surfaces() -> Vec<(&'static str, Surface)> {
    let name = |surface| match surface {
        Surface::Sidebar => "Surface::Sidebar",
        Surface::Menu => "Surface::Menu",
        Surface::Popover => "Surface::Popover",
        Surface::Hud => "Surface::Hud",
        Surface::AppGrid => "Surface::AppGrid",
        Surface::Switcher => "Surface::Switcher",
        Surface::Dock => "Surface::Dock",
        Surface::DockIndicator => "Surface::DockIndicator",
        Surface::DockBar => "Surface::DockBar",
        Surface::Tooltip => "Surface::Tooltip",
        Surface::WindowBackground => "Surface::WindowBackground",
        Surface::TextBackground => "Surface::TextBackground",
        Surface::ToolbarBand => "Surface::ToolbarBand",
    };
    [
        Surface::Sidebar,
        Surface::Menu,
        Surface::Popover,
        Surface::Hud,
        Surface::AppGrid,
        Surface::Switcher,
        Surface::Dock,
        Surface::DockIndicator,
        Surface::DockBar,
        Surface::Tooltip,
        Surface::WindowBackground,
        Surface::TextBackground,
        Surface::ToolbarBand,
    ]
    .map(|surface| (name(surface), surface))
    .to_vec()
}

/// Theme surfaces no component asks the compositor for by name: the dock's bar reaches the dock's surface through its
/// own, and the others are the fills of components that lie on glass.
const WITHOUT_A_ROW: [&str; 3] = [
    "Surface::AppGrid",
    "Surface::Dock",
    "Surface::TextBackground",
];

fn tokens(appearance: Appearance, reduce_transparency: bool) -> ThemeTokens {
    let preferences = ThemePreferences {
        appearance,
        reduce_transparency,
        reduce_motion: false,
    };
    ThemeTokens::cupertino(&Theme::load().unwrap(), preferences).unwrap()
}

#[test]
fn every_component_of_the_inventory_still_asks_for_its_material() {
    let rows = inventory();
    assert!(rows.len() >= 30, "the whole table was read: {}", rows.len());
    for row in &rows {
        let source = std::fs::read_to_string(&row.file)
            .unwrap_or_else(|e| panic!("{}: {}: {e}", row.component, row.file.display()));
        // the call outside the file's comments, so a note about a material that was removed does not stand in for it
        let drawn = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .any(|line| line.contains(&row.call));
        assert!(
            drawn,
            "{}: {} no longer holds `{}`",
            row.component,
            row.file.display(),
            row.call
        );
    }
}

#[test]
fn a_theme_surface_of_the_inventory_does_with_each_display_option_what_its_row_says() {
    let rows = inventory();
    let surfaces = surfaces();
    let mut seen = Vec::new();
    for row in &rows {
        let Some(&(name, surface)) = surfaces.iter().find(|(name, _)| *name == row.call) else {
            continue;
        };
        seen.push(name);
        for (dark, plain, contrast) in [
            (false, Appearance::Light, Appearance::LightHighContrast),
            (true, Appearance::Dark, Appearance::DarkHighContrast),
        ] {
            let reduced = tokens(plain, true).surface(surface);
            assert_eq!(
                reduced.material.is_some(),
                row.reduce_transparency.asks(dark),
                "{} ({name}) with Reduce Transparency, dark {dark}: {:?}",
                row.component,
                reduced.material
            );
            let contrasted = tokens(contrast, false).surface(surface);
            assert_eq!(
                contrasted.material.is_some(),
                row.increase_contrast.asks(dark),
                "{} ({name}) with Increase Contrast, dark {dark}: {:?}",
                row.component,
                contrasted.material
            );
            // where it asks for none, the fill it paints instead is opaque
            for look in [reduced, contrasted] {
                if look.material.is_none() {
                    assert_eq!(look.fill.map(|c| c.a), Some(1.0), "{}", row.component);
                }
            }
        }
    }
    for (name, _) in &surfaces {
        assert!(
            seen.contains(name) != WITHOUT_A_ROW.contains(name),
            "{name} has a row of the inventory, or is one of the surfaces without a row, and not both"
        );
    }
}
