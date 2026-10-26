//! Assemble the complete set of generated files.
//!
//! Layout of the output directory (all paths relative):
//!
//! ```text
//! themes/<Name>/index.theme, gtk-3.0/{gtk,gtk-dark}.css, gtk-4.0/{gtk,gtk-dark}.css   light theme
//! themes/<Name>-dark/...                                                              dark theme
//! config/gtk-4.0/{gtk,gtk-dark}.css        libadwaita overrides (for ~/.config/gtk-4.0)
//! config/gtk-{3,4}.0/settings.ini          font and theme name, double-click and drag thresholds
//! qt/color-schemes/<Name>{Light,Dark}.colors             KDE colour schemes
//! qt/qt5ct/colors/*.conf, qt/qt6ct/colors/*.conf         palette schemes
//! qt/qt6ct/qt6ct.conf.snippet                            settings to merge (not installed)
//! fontconfig/conf.d/60-cupertino-ui-font.conf
//! README.txt
//! ```

use crate::css;
use crate::fontconfig::{self, FontOptions};
use crate::metrics::{ControlSizeName, Metrics};
use crate::palette::{trim_float, Accent, Palette};
use crate::qt;
use cupertino_theme::Theme;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct GeneratorOptions {
    pub name: String,
    pub accent: Accent,
    pub size: ControlSizeName,
    pub high_contrast: bool,
    pub map_sans_serif: bool,
}

impl Default for GeneratorOptions {
    fn default() -> Self {
        Self { name: "Cupertino".into(), accent: Accent::Blue, size: ControlSizeName::Regular, high_contrast: false, map_sans_serif: false }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedFile {
    pub path: PathBuf,
    pub contents: String,
}

fn file(path: impl Into<PathBuf>, contents: String) -> GeneratedFile {
    GeneratedFile { path: path.into(), contents }
}

fn compact(name: &str) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

fn index_theme(name: &str, comment: &str) -> String {
    format!("[Desktop Entry]\nType=X-GNOME-Metatheme\nName={name}\nComment={comment}\n\n[X-GNOME-Metatheme]\nGtkTheme={name}\nMetacityTheme={name}\nIconTheme=hicolor\nButtonLayout=close,minimize,maximize:\n")
}

/// GTK settings: theme, font and button layout, and the pointer timing and distances of the
/// reference: double-click interval 0.5 s, double-click distance
/// `min(6t + 2, 14) = 5 px for it (formula, the fresh value) and the
/// 5 pt drag-start threshold.
fn settings_ini(theme: &str, m: &Metrics) -> String {
    let click = cupertino_input::DEFAULT_DOUBLE_CLICK_TIME;
    format!(
        "[Settings]\ngtk-theme-name={theme}\ngtk-font-name={} {}\ngtk-decoration-layout=close,minimize,maximize:\ngtk-double-click-time={}\ngtk-double-click-distance={}\ngtk-dnd-drag-threshold={}\n",
        m.font_family,
        trim_float(m.font_pt, 1),
        (click * 1000.0).round(),
        cupertino_input::double_click_space(click).round(),
        cupertino_input::DRAG_START_THRESHOLD.round(),
    )
}

pub const FONTCONFIG_FILE: &str = "60-cupertino-ui-font.conf";

pub fn generate(theme: &Theme, o: &GeneratorOptions) -> Vec<GeneratedFile> {
    let m = Metrics::from_theme(theme, o.size);
    let light = Palette::from_theme(theme, false, o.high_contrast, o.accent);
    let dark = Palette::from_theme(theme, true, o.high_contrast, o.accent);
    let mut files = Vec::new();
    let dark_name = format!("{}-dark", o.name);

    for (dir, mode) in [(o.name.as_str(), false), (dark_name.as_str(), true)] {
        let base = Path::new("themes").join(dir);
        let (own, other) = if mode { (&dark, &dark) } else { (&light, &dark) };
        files.push(file(base.join("index.theme"), index_theme(dir, if mode { "Dark appearance generated from the design tokens" } else { "Light appearance generated from the design tokens" })));
        files.push(file(base.join("gtk-3.0/gtk.css"), css::gtk3_theme(own, &m)));
        files.push(file(base.join("gtk-3.0/gtk-dark.css"), css::gtk3_theme(other, &m)));
        files.push(file(base.join("gtk-4.0/gtk.css"), css::gtk4_theme(own, &m)));
        files.push(file(base.join("gtk-4.0/gtk-dark.css"), css::gtk4_theme(other, &m)));
    }

    files.push(file("config/gtk-4.0/gtk.css", css::libadwaita_override(&light, &m)));
    files.push(file("config/gtk-4.0/gtk-dark.css", css::libadwaita_override(&dark, &m)));
    files.push(file("config/gtk-3.0/settings.ini", settings_ini(&o.name, &m)));
    files.push(file("config/gtk-4.0/settings.ini", settings_ini(&o.name, &m)));

    let stem = compact(&o.name);
    for (palette, mode, other_palette) in [(&light, "Light", &dark), (&dark, "Dark", &light)] {
        let file_stem = format!("{stem}{mode}");
        files.push(file(format!("qt/color-schemes/{file_stem}.colors"), qt::kde_colors(palette, other_palette, &format!("{} {mode}", o.name), &file_stem)));
        let scheme = qt::qtct_scheme(palette);
        files.push(file(format!("qt/qt5ct/colors/{file_stem}.conf"), scheme.clone()));
        files.push(file(format!("qt/qt6ct/colors/{file_stem}.conf"), scheme));
    }
    files.push(file("qt/qt6ct/qt6ct.conf.snippet", qt::qtct_conf_snippet(&format!("~/.config/qt6ct/colors/{stem}Light.conf"), &m.font_family, m.font_pt)));

    files.push(file(Path::new("fontconfig/conf.d").join(FONTCONFIG_FILE), fontconfig::snippet(&FontOptions { family: m.font_family.clone(), map_sans_serif: o.map_sans_serif, rendering: true })));
    files.push(file("README.txt", readme(o, &m, &files)));
    files
}

fn readme(o: &GeneratorOptions, m: &Metrics, files: &[GeneratedFile]) -> String {
    let mut s = format!(
        "Cupertino toolkit themes (generated)\n\naccent: {}, control size: {:?}, high contrast: {}, UI font: {} {} pt\n\nNothing here is installed automatically; run the generator with --install to copy the files\ninto the XDG directories of the current user (existing files are kept unless --force).\n\nFiles:\n",
        o.accent.name(),
        m.size,
        o.high_contrast,
        m.font_family,
        trim_float(m.font_pt, 1)
    );
    for f in files {
        s.push_str(&format!("  {}\n", f.path.display()));
    }
    s.push_str("\nGTK 3/4 apps: select the theme `");
    s.push_str(&o.name);
    s.push_str("` (or `-dark`) in the settings; libadwaita apps read config/gtk-4.0/gtk.css.\nQt apps: use the colour scheme in your Qt platform theme (KDE) or qt5ct/qt6ct with the Fusion style.\n");
    s
}

/// Write `files` below `dir`, creating directories. Returns the written paths.
pub fn write_files(files: &[GeneratedFile], dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for f in files {
        let p = dir.join(&f.path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, &f.contents)?;
        out.push(p);
    }
    Ok(out)
}
