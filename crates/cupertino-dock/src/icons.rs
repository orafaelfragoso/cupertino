//! The icons the dock shows, loaded for the toolkit's images: an application's own icon (an absolute path or the
//! `hicolor` theme), the icon a window set for itself, and the system's generic application and trash icons from the
//! asset directory. Every surface of the process draws them through CupertinoKit ([`crate::view`], the switcher's
//! panel of [`crate::switcher_kit`]).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_icons::{SystemIcons, ThemeIcons};
use cupertino_shell_client::{DockWindowIcon, WindowIconImage};
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

use crate::desktop::DesktopIndex;

/// Semantic names of the generic application icon and of the trash's two states.
const GENERIC_APPLICATION_ICON: &str = "application.generic";
const TRASH_EMPTY_ICON: &str = "trash.empty";
const TRASH_FULL_ICON: &str = "trash.full";

/// How long the trash's state is trusted before the folder is looked at again.
const TRASH_CHECK_EVERY: Duration = Duration::from_secs(1);

/// Whether the home trash holds anything: `$XDG_DATA_HOME/Trash/files` (`~/.local/share` by default) has an entry.
fn trash_is_full() -> bool {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    data_home.is_some_and(|d| {
        std::fs::read_dir(d.join("Trash/files")).is_ok_and(|mut entries| entries.next().is_some())
    })
}

/// Icon pixmaps by name and pixel size, loaded once: an application's own icon (an absolute path or the `hicolor`
/// theme) and the system's icons from the asset directory; and the icons windows set for themselves.
pub struct Icons {
    apps: ThemeIcons,
    system: SystemIcons,
    cache: HashMap<(String, u32), Option<Pixmap>>,
    /// Windows' own icons by window id.
    windows: BTreeMap<u32, WindowIcon>,
    trash: Option<(Instant, bool)>,
}

/// The icon a window set for itself: a theme name, an image, or both.
struct WindowIcon {
    /// The window's application id and the dock's key for it (its tile's).
    apps: [String; 2],
    name: Option<String>,
    image: Option<Pixmap>,
    /// The image resampled to each smaller size it is drawn at.
    scaled: BTreeMap<u32, Pixmap>,
}

impl WindowIcon {
    /// The image at `px`: resampled once when it is larger, as it is when not.
    fn at(&mut self, px: u32) -> &Pixmap {
        let image = self.image.as_ref().expect("has an image");
        if image.width() <= px {
            return image;
        }
        self.scaled
            .entry(px)
            .or_insert_with(|| resampled(image, px))
    }
}

/// Where an application's icon comes from.
enum IconSource {
    Theme(String),
    Window(u32),
    Generic,
}

/// A window's ARGB8888 image (premultiplied, little-endian) as a pixmap; a colour channel over its alpha is cut to it.
fn window_pixmap(image: &WindowIconImage) -> Option<Pixmap> {
    let rgba = image
        .argb
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[b, g, r, a]| [r.min(a), g.min(a), b.min(a), a])
        .collect();
    Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(image.size, image.size)?)
}

/// A square pixmap at half its side, each pixel the mean of the (up to) four it covers.
fn halved(image: &Pixmap) -> Pixmap {
    let side = image.width();
    let half = side.div_ceil(2);
    let src = image.data();
    let mut out = Pixmap::new(half, half).expect("a non-empty side");
    let at = |x: u32, y: u32| (y.min(side - 1) * side + x.min(side - 1)) as usize * 4;
    for (i, px) in out.data_mut().as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (2 * (i as u32 % half), 2 * (i as u32 / half));
        let corners = [at(x, y), at(x + 1, y), at(x, y + 1), at(x + 1, y + 1)];
        for (c, v) in px.iter_mut().enumerate() {
            let sum: u32 = corners.iter().map(|&o| u32::from(src[o + c])).sum();
            *v = ((sum + 2) / 4) as u8;
        }
    }
    out
}

/// A square image larger than `px` at `px` a side: halved while it is at least twice as large, then bilinear.
fn resampled(image: &Pixmap, px: u32) -> Pixmap {
    let px = px.max(1);
    let mut image = image.clone();
    while image.width() >= 2 * px {
        image = halved(&image);
    }
    if image.width() == px {
        return image;
    }
    let mut out = Pixmap::new(px, px).expect("a non-empty side");
    let fit = px as f32 / image.width() as f32;
    out.draw_pixmap(
        0,
        0,
        image.as_ref(),
        &PixmapPaint {
            quality: FilterQuality::Bilinear,
            ..PixmapPaint::default()
        },
        Transform::from_scale(fit, fit),
        None,
    );
    out
}

impl Icons {
    /// The generic application icon and the trash's are reported at startup when no asset pack provides them; they
    /// are then drawn as placeholders.
    pub fn new(assets: &Assets) -> Self {
        let system = SystemIcons::required(
            assets,
            &[GENERIC_APPLICATION_ICON, TRASH_EMPTY_ICON, TRASH_FULL_ICON],
            &[],
        );
        Self {
            apps: ThemeIcons::new(assets),
            system,
            cache: HashMap::new(),
            windows: BTreeMap::new(),
            trash: None,
        }
    }

    /// A window set its own icon, or removed it; `apps` are its application id and the dock's key for it.
    pub fn window_icon(&mut self, apps: [String; 2], icon: DockWindowIcon) {
        let image = icon.image.as_ref().and_then(window_pixmap);
        if icon.name.is_none() && image.is_none() {
            self.windows.remove(&icon.id);
        } else {
            self.windows.insert(
                icon.id,
                WindowIcon {
                    apps,
                    name: icon.name,
                    image,
                    scaled: BTreeMap::new(),
                },
            );
        }
    }

    pub fn window_closed(&mut self, id: u32) {
        self.windows.remove(&id);
    }

    /// Whether the theme has icon `name` at `px`, loading it once.
    fn has_themed(&mut self, name: &str, px: u32) -> bool {
        let apps = &mut self.apps;
        self.cache
            .entry((name.to_owned(), px))
            .or_insert_with(|| apps.load(name, px))
            .is_some()
    }

    /// The theme's icon `name`, else the asset `application.generic`.
    pub fn named(&mut self, name: &str, px: u32) -> &Pixmap {
        if self.has_themed(name, px) {
            self.cache[&(name.to_owned(), px)].as_ref().expect("loaded")
        } else {
            self.system.raster(GENERIC_APPLICATION_ICON, px)
        }
    }

    fn source(&mut self, desktop: &DesktopIndex, app_id: &str, px: u32) -> IconSource {
        if let Some(name) = desktop.entry_icon(app_id) {
            if self.has_themed(name, px) {
                return IconSource::Theme(name.to_owned());
            }
        }
        let window = self
            .windows
            .iter()
            .find(|(_, w)| w.apps.iter().any(|a| a == app_id))
            .map(|(id, w)| (*id, w.name.clone(), w.image.is_some()));
        if let Some((id, name, image)) = window {
            if let Some(name) = name.filter(|n| self.has_themed(n, px)) {
                return IconSource::Theme(name);
            }
            if image {
                return IconSource::Window(id);
            }
        }
        if self.has_themed(app_id, px) {
            return IconSource::Theme(app_id.to_owned());
        }
        IconSource::Generic
    }

    /// The icon of an application: its desktop entry's, else the one its window set for itself (a theme name, then
    /// its image), else the theme's icon named as the application, else the asset `application.generic`.
    pub fn app(&mut self, desktop: &DesktopIndex, app_id: &str, px: u32) -> &Pixmap {
        match self.source(desktop, app_id, px) {
            IconSource::Theme(name) => self.cache[&(name, px)].as_ref().expect("loaded"),
            IconSource::Window(id) => self.windows.get_mut(&id).expect("listed").at(px),
            IconSource::Generic => self.system.raster(GENERIC_APPLICATION_ICON, px),
        }
    }

    /// Whether the trash folder holds anything; looked at again once [`TRASH_CHECK_EVERY`] passed.
    pub fn trash_full(&mut self) -> bool {
        let now = cupertino_shell_client::test_clock::display_now();
        match self.trash {
            Some((at, full)) if now.duration_since(at) < TRASH_CHECK_EVERY => full,
            _ => {
                let full = trash_is_full();
                self.trash = Some((now, full));
                full
            }
        }
    }

    /// The trash's icon, empty or full.
    pub fn trash(&mut self, full: bool, px: u32) -> &Pixmap {
        self.system.raster(
            if full {
                TRASH_FULL_ICON
            } else {
                TRASH_EMPTY_ICON
            },
            px,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key no application has an icon for, with an asset directory without application icons (`bare_icons`), so
    /// the result does not depend on the icon theme of the machine.
    const FIRST: &str = "zz-unthemed-first";

    /// An asset directory in `root` whose only icons are the solid-colour 16 px rasters `icons`.
    fn assets_with(root: &std::path::Path, icons: &[(&str, [u8; 3])]) -> Assets {
        let entries: Vec<String> = icons
            .iter()
            .map(|(n, _)| format!("\"{n}\": {{\"raster\": [16]}}"))
            .collect();
        let manifest = format!("{{\"version\": 1, \"fonts\": {{\"ui\": \"F\"}}, \"icons\": {{{}}}, \"cursors\": \"c\", \"wallpaper\": \"w.png\"}}", entries.join(", "));
        std::fs::write(root.join("manifest.json"), manifest).unwrap();
        for rel in ["fonts/a.ttf", "cursors/c/index.theme", "w.png"] {
            std::fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
            std::fs::write(root.join(rel), b"x").unwrap();
        }
        for (name, [r, g, b]) in icons {
            std::fs::create_dir_all(root.join("icons").join(name)).unwrap();
            let mut icon = tiny_skia::Pixmap::new(16, 16).unwrap();
            icon.fill(tiny_skia::Color::from_rgba8(*r, *g, *b, 255));
            icon.save_png(root.join("icons").join(name).join("16.png"))
                .unwrap();
        }
        Assets::open(root).unwrap()
    }

    /// Icons over a temporary asset directory, which lives as long as they do (the icons load lazily).
    struct BareIcons {
        _dir: tempfile::TempDir,
        icons: Icons,
    }

    impl std::ops::Deref for BareIcons {
        type Target = Icons;
        fn deref(&self) -> &Icons {
            &self.icons
        }
    }

    impl std::ops::DerefMut for BareIcons {
        fn deref_mut(&mut self) -> &mut Icons {
            &mut self.icons
        }
    }

    /// Icons from an asset directory that has the three system icons the dock needs and no application icon.
    fn bare_icons() -> BareIcons {
        let dir = tempfile::tempdir().unwrap();
        let icons = Icons::new(&assets_with(dir.path(), &SYSTEM_ICONS));
        BareIcons { _dir: dir, icons }
    }

    /// What a pixel inside the generic application icon of [`SYSTEM_ICONS`] is.
    const GENERIC_APPLICATION_PIXEL: [u8; 4] = [0, 0, 255, 255];

    /// The icons `Icons::new` requires, as distinct solid colours.
    const SYSTEM_ICONS: [(&str, [u8; 3]); 3] = [
        ("application.generic", [0, 0, 255]),
        ("trash.empty", [10, 0, 0]),
        ("trash.full", [200, 0, 0]),
    ];

    fn rgba(icon: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let p = icon.pixel(x, y).unwrap();
        [p.red(), p.green(), p.blue(), p.alpha()]
    }

    /// The pixel at the middle of an icon, whatever size it came at.
    fn middle(icon: &Pixmap) -> [u8; 4] {
        rgba(icon, icon.width() / 2, icon.height() / 2)
    }

    #[test]
    fn an_application_without_an_icon_gets_the_generic_application_icon_of_the_assets() {
        let mut icons = bare_icons();
        let desktop = DesktopIndex::default();
        assert_eq!(
            rgba(icons.app(&desktop, FIRST, 16), 8, 8),
            GENERIC_APPLICATION_PIXEL
        );
    }

    #[test]
    fn the_trash_draws_the_empty_or_the_full_trash_icon() {
        let mut icons = bare_icons();
        assert_eq!(icons.trash(false, 16).pixel(8, 8).unwrap().red(), 10);
        assert_eq!(icons.trash(true, 16).pixel(8, 8).unwrap().red(), 200);
    }

    #[test]
    fn an_icon_no_pack_provides_is_drawn_as_the_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        let mut icons = Icons::new(&assets_with(dir.path(), &[("trash.empty", [10, 0, 0])]));
        let desktop = DesktopIndex::default();
        assert!(
            rgba(icons.app(&desktop, FIRST, 16), 8, 8)[3] > 0,
            "no generic application icon: the placeholder"
        );
    }

    /// Window 1's own icon: an 8 px image of opaque red, in the server's ARGB8888 bytes.
    fn red_window_icon() -> DockWindowIcon {
        DockWindowIcon {
            id: 1,
            name: None,
            image: Some(WindowIconImage {
                size: 8,
                argb: [0, 0, 255, 255].repeat(64),
            }),
        }
    }

    #[test]
    fn an_application_without_a_desktop_icon_shows_its_windows_own_image() {
        let mut icons = bare_icons();
        let desktop = DesktopIndex::default();
        icons.window_icon([FIRST.into(), FIRST.into()], red_window_icon());
        assert_eq!(middle(icons.app(&desktop, FIRST, 48)), [255, 0, 0, 255]);

        icons.window_closed(1);
        assert_eq!(
            middle(icons.app(&desktop, FIRST, 48)),
            GENERIC_APPLICATION_PIXEL
        );
    }

    /// A 512 px checkerboard of 4 px squares: two by two squares averaged into each pixel of the 64 px a tile draws at.
    #[test]
    fn a_large_window_image_is_averaged_down_to_the_tile_not_sampled() {
        let mut icons = bare_icons();
        let desktop = DesktopIndex::default();
        let checker = (0..512 * 512)
            .flat_map(|i| {
                [(i % 512 / 4 + i / 512 / 4) % 2 * 255; 3]
                    .into_iter()
                    .chain([255])
            })
            .map(|v| v as u8)
            .collect();
        icons.window_icon(
            [FIRST.into(), FIRST.into()],
            DockWindowIcon {
                id: 1,
                name: None,
                image: Some(WindowIconImage {
                    size: 512,
                    argb: checker,
                }),
            },
        );
        let icon = icons.app(&desktop, FIRST, 64);
        for (x, y) in [(28, 28), (32, 32), (35, 30), (37, 37)] {
            let p = rgba(icon, x, y);
            assert!(
                p[..3].iter().all(|v| (120..=136).contains(v)),
                "({x}, {y}) is mid grey, not a sampled black or white: {p:?}"
            );
        }
    }

    /// A 96 px image, black left of column 40 and white from it, loaded for a 48 px tile: taken at the 48 px it is
    /// drawn at, the edge falls on a whole pixel (column 20), with no blended column.
    #[test]
    fn an_icon_loaded_at_its_drawn_size_keeps_a_straight_edge_sharp() {
        let mut icons = bare_icons();
        let desktop = DesktopIndex::default();
        let split = (0..96 * 96)
            .flat_map(|i| {
                [if i % 96 < 40 { 0 } else { 255 }; 3]
                    .into_iter()
                    .chain([255])
            })
            .map(|v| v as u8)
            .collect();
        icons.window_icon(
            [FIRST.into(), FIRST.into()],
            DockWindowIcon {
                id: 1,
                name: None,
                image: Some(WindowIconImage {
                    size: 96,
                    argb: split,
                }),
            },
        );
        let icon = icons.app(&desktop, FIRST, 48);
        assert_eq!(icon.width(), 48);
        let blended: Vec<_> = (0..48)
            .map(|x| rgba(icon, x, 24)[0])
            .filter(|v| (16..=239).contains(v))
            .collect();
        assert_eq!(blended, Vec::<u8>::new());
        assert_eq!(rgba(icon, 19, 24)[0], 0);
        assert_eq!(rgba(icon, 20, 24)[0], 255);
    }
}
