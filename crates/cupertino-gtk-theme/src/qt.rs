//! Qt colour schemes: a KDE `.colors` file and a `qt5ct`/`qt6ct` palette. No Kvantum theme is
//! involved: both formats only carry colours and rely on the Fusion style (set in `qt5ct`/`qt6ct`)
//! or on the KDE style.
//!
//! The KDE file follows the standard section layout (`[Colors:View]`, `[Colors:Window],
//! `[Colors:Button]`, `[Colors:Selection]`, `[Colors:Tooltip]`, `[Colors:Complementary],
//! `[Colors:Header]`, the two `[ColorEffects:*]` sections, `[General]`, `[KDE]`, `[WM]); all values
//! are opaque `r,g,b` composited over the relevant background. The `[ColorEffects] numbers are
//! the usual defaults of such files, not values of the design.
//!
//! The palette file lists the 21 `QPalette` roles in `ColorRole` order for the active, disabled and
//! inactive groups as `#aarrggbb`. Light, Midlight, Dark and Mid are derived from the button
//! colour (approximate).

use crate::palette::{over, Palette};
use cupertino_theme::Color;

fn rgb(c: Color) -> String {
    let [r, g, b, _] = c.to_rgba8();
    format!("{r},{g},{b}")
}

fn argb(c: Color) -> String {
    let [r, g, b, a] = c.to_rgba8();
    format!("#{a:02x}{r:02x}{g:02x}{b:02x}")
}

fn section(out: &mut String, name: &str, pairs: &[(&str, String)]) {
    out.push_str(&format!("[{name}]\n"));
    for (k, v) in pairs {
        out.push_str(&format!("{k}={v}\n"));
    }
    out.push('\n');
}

struct Group {
    bg: Color,
    bg_alt: Color,
    fg: Color,
    fg_inactive: Color,
}

fn colors_section(out: &mut String, name: &str, p: &Palette, g: &Group) {
    section(
        out,
        &format!("Colors:{name}"),
        &[
            ("BackgroundAlternate", rgb(g.bg_alt)),
            ("BackgroundNormal", rgb(g.bg)),
            ("DecorationFocus", rgb(p.accent)),
            ("DecorationHover", rgb(p.accent)),
            ("ForegroundActive", rgb(p.accent)),
            ("ForegroundInactive", rgb(g.fg_inactive)),
            ("ForegroundLink", rgb(over(p.link, g.bg))),
            ("ForegroundNegative", rgb(p.error)),
            ("ForegroundNeutral", rgb(p.warning)),
            ("ForegroundNormal", rgb(g.fg)),
            ("ForegroundPositive", rgb(p.success)),
            ("ForegroundVisited", rgb(over(p.link_visited, g.bg))),
        ],
    );
}

/// KDE colour scheme file contents. `complementary` supplies the opposite-brightness palette for
/// `[Colors:Complementary]`.
pub fn kde_colors(
    p: &Palette,
    complementary: &Palette,
    display_name: &str,
    file_stem: &str,
) -> String {
    let mut out = String::new();
    let win = p.window_bg;
    let view = p.view_bg;
    let window = Group {
        bg: win,
        bg_alt: over(p.fill_tertiary, win),
        fg: over(p.window_fg, win),
        fg_inactive: over(p.secondary_fg, win),
    };
    let viewg = Group {
        bg: view,
        bg_alt: over(p.zebra, view),
        fg: over(p.view_fg, view),
        fg_inactive: over(p.secondary_fg, view),
    };
    let button = Group {
        bg: over(p.control, win),
        bg_alt: over(p.fill_secondary, over(p.control, win)),
        fg: over(p.window_fg, win),
        fg_inactive: over(p.secondary_fg, win),
    };
    let selection = Group {
        bg: p.selection_bg,
        bg_alt: p.selection_bg,
        fg: p.selection_fg,
        fg_inactive: over(p.selection_fg.with_alpha(0.75), p.selection_bg),
    };
    let tooltip = Group {
        bg: p.tooltip_bg,
        bg_alt: p.tooltip_bg,
        fg: over(p.tooltip_fg, p.tooltip_bg),
        fg_inactive: over(p.secondary_fg, p.tooltip_bg),
    };
    let header = Group {
        bg: p.headerbar_bg,
        bg_alt: p.headerbar_backdrop,
        fg: over(p.headerbar_fg, p.headerbar_bg),
        fg_inactive: over(p.secondary_fg, p.headerbar_bg),
    };
    let comp = Group {
        bg: complementary.window_bg,
        bg_alt: over(complementary.fill_tertiary, complementary.window_bg),
        fg: over(complementary.window_fg, complementary.window_bg),
        fg_inactive: over(complementary.secondary_fg, complementary.window_bg),
    };

    section(
        &mut out,
        "ColorEffects:Disabled",
        &[
            ("Color", "56,56,56".into()),
            ("ColorAmount", "0".into()),
            ("ColorEffect", "0".into()),
            ("ContrastAmount", "0.65".into()),
            ("ContrastEffect", "1".into()),
            ("IntensityAmount", "0.1".into()),
            ("IntensityEffect", "2".into()),
        ],
    );
    section(
        &mut out,
        "ColorEffects:Inactive",
        &[
            ("ChangeSelectionColor", "true".into()),
            ("Color", "112,111,110".into()),
            ("ColorAmount", "0.025".into()),
            ("ColorEffect", "2".into()),
            ("ContrastAmount", "0.1".into()),
            ("ContrastEffect", "2".into()),
            ("Enable", "false".into()),
            ("IntensityAmount", "0".into()),
            ("IntensityEffect", "0".into()),
        ],
    );
    colors_section(&mut out, "Button", p, &button);
    colors_section(&mut out, "Complementary", complementary, &comp);
    colors_section(&mut out, "Header", p, &header);
    colors_section(&mut out, "Selection", p, &selection);
    colors_section(&mut out, "Tooltip", p, &tooltip);
    colors_section(&mut out, "View", p, &viewg);
    colors_section(&mut out, "Window", p, &window);
    section(
        &mut out,
        "General",
        &[
            ("ColorScheme", file_stem.to_string()),
            ("Name", display_name.to_string()),
            ("shadeSortColumn", "true".into()),
        ],
    );
    section(&mut out, "KDE", &[("contrast", "4".into())]);
    section(
        &mut out,
        "WM",
        &[
            ("activeBackground", rgb(p.headerbar_bg)),
            ("activeBlend", rgb(p.headerbar_fg.with_alpha(1.0))),
            (
                "activeForeground",
                rgb(over(p.headerbar_fg, p.headerbar_bg)),
            ),
            ("inactiveBackground", rgb(p.headerbar_backdrop)),
            (
                "inactiveBlend",
                rgb(over(p.secondary_fg, p.headerbar_backdrop)),
            ),
            (
                "inactiveForeground",
                rgb(over(p.secondary_fg, p.headerbar_backdrop)),
            ),
        ],
    );
    out.trim_end().to_string() + "\n"
}

/// `QPalette::ColorRole` names in the order of a `qt5ct`/`qt6ct` palette line.
pub const QT_ROLES: [&str; 21] = [
    "WindowText",
    "Button",
    "Light",
    "Midlight",
    "Dark",
    "Mid",
    "Text",
    "BrightText",
    "ButtonText",
    "Base",
    "Window",
    "Shadow",
    "Highlight",
    "HighlightedText",
    "Link",
    "LinkVisited",
    "AlternateBase",
    "NoRole",
    "ToolTipBase",
    "ToolTipText",
    "PlaceholderText",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteGroup {
    Active,
    Disabled,
    Inactive,
}

/// The 21 colours of one palette group.
pub fn qt_palette_group(p: &Palette, group: PaletteGroup) -> [Color; 21] {
    let black = Color::new(0.0, 0.0, 0.0, 1.0);
    let white = Color::new(1.0, 1.0, 1.0, 1.0);
    let window = p.window_bg;
    let base = p.view_bg;
    let button = over(p.control, window);
    let text = over(p.window_fg, window);
    let view_text = over(p.view_fg, base);
    let disabled_text = over(p.disabled_fg, window);
    let (highlight, highlighted_text) = match group {
        PaletteGroup::Inactive => (p.selection_inactive_bg, p.selection_inactive_fg),
        PaletteGroup::Disabled => (
            over(p.selection_bg.with_alpha(0.4), window),
            over(p.selection_fg.with_alpha(0.6), p.selection_bg),
        ),
        PaletteGroup::Active => (p.selection_bg, p.selection_fg),
    };
    let (fg, vt, bt) = match group {
        PaletteGroup::Disabled => (disabled_text, over(p.disabled_fg, base), disabled_text),
        _ => (text, view_text, text),
    };
    let (light, midlight, dark, mid) = if p.dark {
        (
            button.lerp(white, 0.30),
            button.lerp(white, 0.15),
            button.lerp(black, 0.45),
            button.lerp(black, 0.25),
        )
    } else {
        (
            button.lerp(white, 0.80),
            button.lerp(white, 0.45),
            button.lerp(black, 0.30),
            button.lerp(black, 0.15),
        )
    };
    [
        fg,
        button,
        light,
        midlight,
        dark,
        mid,
        vt,
        white,
        bt,
        base,
        window,
        black,
        highlight,
        highlighted_text,
        over(p.link, window),
        over(p.link_visited, window),
        over(p.zebra, base),
        window,
        p.tooltip_bg,
        over(p.tooltip_fg, p.tooltip_bg),
        over(p.placeholder, base),
    ]
}

/// `qt5ct`/`qt6ct` colour scheme file (`[ColorScheme]` with three palette lines).
pub fn qtct_scheme(p: &Palette) -> String {
    let line = |g: PaletteGroup| {
        qt_palette_group(p, g)
            .iter()
            .map(|c| argb(*c))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "[ColorScheme]\nactive_colors={}\ndisabled_colors={}\ninactive_colors={}\n",
        line(PaletteGroup::Active),
        line(PaletteGroup::Disabled),
        line(PaletteGroup::Inactive)
    )
}

/// Settings fragment to merge into `qt5ct.conf` / `qt6ct.conf` (not installed automatically).
pub fn qtct_conf_snippet(scheme_path: &str, font_family: &str, font_pt: f64) -> String {
    format!(
        "# merge into ~/.config/qt6ct/qt6ct.conf (and qt5ct.conf)\n[Appearance]\ncolor_scheme_path={scheme_path}\ncustom_palette=true\nstandard_dialogs=default\nstyle=Fusion\n\n[Fonts]\ngeneral=\"{font_family},{},-1,5,50,0,0,0,0,0\"\n",
        crate::palette::trim_float(font_pt, 1)
    )
}

/// Parse the values of a KDE colour scheme into `(section, key) -> "r,g,b"` (used by tests and the
/// generator's self-check).
pub fn parse_kde(text: &str) -> std::collections::BTreeMap<(String, String), String> {
    let mut map = std::collections::BTreeMap::new();
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(s) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = s.to_string();
        } else if let Some((k, v)) = line.split_once('=') {
            map.insert((section.clone(), k.to_string()), v.to_string());
        }
    }
    map
}

/// Parse a `qt5ct`/`qt6ct` palette line into RGBA8 colours.
pub fn parse_qtct_line(line: &str) -> Option<Vec<[u8; 4]>> {
    line.split(',')
        .map(|s| {
            let h = s.trim().strip_prefix('#')?;
            if h.len() != 8 {
                return None;
            }
            let v = u32::from_str_radix(h, 16).ok()?;
            Some([(v >> 16) as u8, (v >> 8) as u8, v as u8, (v >> 24) as u8])
        })
        .collect()
}
