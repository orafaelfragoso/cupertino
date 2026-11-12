//! The system's symbols from the asset packs, and the running application's own icon, for the widgets' [`Icons`].

use cupertino_assets::{Assets, IconName};
use cupertino_icons::{SystemIcons, ThemeIcons};
use cupertino_ui_core::{next_resource_id, IconImage, Icons, PlaceholderIcons};
use cupertino_ui_render::{ImageResource, PixelFormat};
use cupertino_ui_theme::Color;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// A symbol as asked for: its name, device pixels, colour in 8-bit steps, and whether it is mirrored.
type Request = (String, u32, [u8; 4], bool);

/// [`SystemIcons`] of the asset packs. Each request keeps its own id and pixels for the life of the app, so a node
/// repainted and a node reused from the paint cache never name two images with one id.
pub(crate) struct PackIcons {
    state: Mutex<(SystemIcons, HashMap<Request, IconImage>)>,
    /// What a name that is not a valid icon name draws.
    invalid: PlaceholderIcons,
    /// The application's icon: where to find its desktop entry, the entry's `Icon=` once read, the host's icon
    /// themes for an icon the packs do not have, and the images drawn by size.
    application: Application,
}

/// The pieces [`PackIcons::application`] works from. Nothing is read until an alert first asks for the icon.
struct Application {
    /// The desktop entry id: the running program's file name.
    entry: Option<String>,
    dirs: Vec<PathBuf>,
    icon: OnceLock<Option<String>>,
    /// The packs the host's themes are searched with, the first time an icon is not in the packs.
    assets: Assets,
    themes: OnceLock<Mutex<ThemeIcons>>,
    drawn: Mutex<HashMap<u32, Option<IconImage>>>,
    /// The icons of installed applications by name and size.
    named: Mutex<HashMap<(String, u32), IconImage>>,
}

/// The pack's icon for an application without one of its own.
const GENERIC_APPLICATION: &str = "application.generic";

impl PackIcons {
    pub(crate) fn new(assets: &Assets) -> Self {
        let entry = std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.file_name()?.to_str()?.to_owned()));
        Self::of(
            SystemIcons::new(assets),
            Application {
                entry,
                dirs: cupertino_apps::entry::xdg_application_dirs(),
                icon: OnceLock::new(),
                assets: assets.clone(),
                themes: OnceLock::new(),
                drawn: Mutex::new(HashMap::new()),
                named: Mutex::new(HashMap::new()),
            },
        )
    }

    fn of(icons: SystemIcons, application: Application) -> Self {
        Self {
            state: Mutex::new((icons, HashMap::new())),
            invalid: PlaceholderIcons::default(),
            application,
        }
    }

    /// `Icon=` of the application's desktop entry, read once.
    fn entry_icon(&self) -> Option<&str> {
        let app = &self.application;
        app.icon
            .get_or_init(|| {
                let id = app.entry.as_deref()?;
                let text = app.dirs.iter().find_map(|dir| {
                    std::fs::read_to_string(dir.join(format!("{id}.desktop"))).ok()
                })?;
                cupertino_apps::entry::parse_handler(
                    id,
                    &text,
                    &cupertino_apps::entry::Locale::from_env(),
                    &cupertino_apps::entry::Visibility::from_env(),
                )
                .ok()?
                .entry
                .icon
            })
            .as_deref()
    }

    /// The application's icon at `px`: its entry's icon from the packs, else from the host's themes or a file, else
    /// the packs' generic application icon (or its placeholder).
    fn draw_application(&self, px: u32) -> Option<IconImage> {
        let icon = self.entry_icon();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let icons = &mut state.0;
        if let Some(name) = icon.filter(|n| IconName::new(n).is_ok()) {
            if icons.has_raster(name, px) {
                let p = icons.raster(name, px);
                return Some(image_of(p.width(), p.height(), p.data()));
            }
        }
        if let Some(name) = icon {
            let app = &self.application;
            let found = app
                .themes
                .get_or_init(|| Mutex::new(ThemeIcons::new(&app.assets)))
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .load(name, px);
            if let Some(p) = found {
                return Some(image_of(p.width(), p.height(), p.data()));
            }
        }
        let p = icons.raster(GENERIC_APPLICATION, px);
        Some(image_of(p.width(), p.height(), p.data()))
    }
}

impl PackIcons {
    /// The icon of the application `name` at `px`: a pack's icon of that name, else the host's themes or a file, else
    /// the packs' generic application icon.
    fn draw_named(&self, name: &str, px: u32) -> IconImage {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let icons = &mut state.0;
        if !name.is_empty() {
            if IconName::new(name).is_ok() && icons.has_raster(name, px) {
                let p = icons.raster(name, px);
                return image_of(p.width(), p.height(), p.data());
            }
            let app = &self.application;
            let found = app
                .themes
                .get_or_init(|| Mutex::new(ThemeIcons::new(&app.assets)))
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .load(name, px);
            if let Some(p) = found {
                return image_of(p.width(), p.height(), p.data());
            }
        }
        let p = icons.raster(GENERIC_APPLICATION, px);
        image_of(p.width(), p.height(), p.data())
    }
}

/// Premultiplied RGBA pixels as an image with an id of its own.
fn image_of(width: u32, height: u32, data: &[u8]) -> IconImage {
    IconImage {
        id: next_resource_id(),
        image: ImageResource {
            revision: 1,
            width,
            height,
            stride: width * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: Arc::from(data),
        },
    }
}

impl fmt::Debug for PackIcons {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PackIcons")
    }
}

impl Icons for PackIcons {
    fn symbol(&self, name: &str, px: u32, color: Color, mirrored: bool) -> IconImage {
        if IconName::new(name).is_err() {
            return self.invalid.symbol(name, px, color, mirrored);
        }
        let rgba = [color.r, color.g, color.b, color.a];
        let request = (
            name.to_owned(),
            px,
            rgba.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8),
            mirrored,
        );
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (icons, drawn) = &mut *state;
        if let Some(image) = drawn.get(&request) {
            return image.clone();
        }
        let pixmap = icons.symbol(name, px, rgba);
        let (width, height) = (pixmap.width(), pixmap.height());
        let mut pixels = pixmap.data().to_vec();
        if mirrored {
            for row in pixels.chunks_exact_mut(width as usize * 4) {
                row.as_chunks_mut::<4>().0.reverse();
            }
        }
        let image = IconImage {
            id: next_resource_id(),
            image: ImageResource {
                revision: 1,
                width,
                height,
                stride: width * 4,
                format: PixelFormat::Rgba8Premultiplied,
                pixels: Arc::from(pixels),
            },
        };
        drawn.insert(request, image.clone());
        image
    }

    fn named(&self, name: &str, px: u32) -> Option<IconImage> {
        let key = (name.to_owned(), px.max(1));
        let cached = self
            .application
            .named
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned();
        if let Some(image) = cached {
            return Some(image);
        }
        let image = self.draw_named(name, key.1);
        self.application
            .named
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, image.clone());
        Some(image)
    }

    fn application(&self, px: u32) -> Option<IconImage> {
        let px = px.max(1);
        if let Some(drawn) = self
            .application
            .drawn
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&px)
        {
            return drawn.clone();
        }
        let image = self.draw_application(px);
        self.application
            .drawn
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(px, image.clone());
        image
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icons() -> PackIcons {
        with_entry(None, Vec::new())
    }

    /// Icons of the built-in packs for an application whose desktop entry id is `entry`, looked up under `dirs`.
    fn with_entry(entry: Option<&str>, dirs: Vec<PathBuf>) -> PackIcons {
        let assets = Assets::builtin();
        let themes = OnceLock::new();
        let _ = themes.set(Mutex::new(ThemeIcons::with_roots(
            &assets,
            Vec::new(),
            Vec::new(),
        )));
        PackIcons::of(
            SystemIcons::without_host(&assets),
            Application {
                entry: entry.map(str::to_owned),
                dirs,
                icon: OnceLock::new(),
                assets: assets.clone(),
                themes,
                drawn: Mutex::new(HashMap::new()),
                named: Mutex::new(HashMap::new()),
            },
        )
    }

    /// Starting an application reads no icon theme and no desktop entry: only an alert's first icon does.
    #[test]
    fn the_application_icon_is_looked_up_only_when_first_asked_for() {
        let icons = PackIcons::new(&Assets::builtin());
        icons.symbol("symbol.house", 16, BLACK, false);
        assert!(icons.application.themes.get().is_none());
        assert!(icons.application.icon.get().is_none());
    }

    #[test]
    fn the_application_icon_is_its_entrys_else_the_generic_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut red = tiny_skia::Pixmap::new(64, 64).unwrap();
        red.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let file = dir.path().join("example.png");
        red.save_png(&file).unwrap();
        std::fs::write(
            dir.path().join("example-files.desktop"),
            format!(
                "[Desktop Entry]\nType=Application\nName=Files\nExec=example-files\nIcon={}\n",
                file.display()
            ),
        )
        .unwrap();
        let own = with_entry(Some("example-files"), vec![dir.path().to_owned()]);
        let image = own.application(64).expect("the entry's icon");
        assert_eq!((image.image.width, image.image.height), (64, 64));
        assert_eq!(
            own.application(64),
            Some(image.clone()),
            "drawn once per size"
        );
        assert_eq!(
            &image.image.pixels[..4],
            &[255, 0, 0, 255],
            "the entry's file"
        );
        let mut packs = SystemIcons::without_host(&Assets::builtin());

        let unknown = with_entry(Some("not-installed"), vec![dir.path().to_owned()]);
        let generic = unknown.application(64).expect("the generic icon");
        assert_eq!(
            &*generic.image.pixels,
            packs.raster(GENERIC_APPLICATION, 64).data()
        );
        assert_eq!(
            &*icons().application(32).unwrap().image.pixels,
            packs.raster(GENERIC_APPLICATION, 32).data(),
            "no entry at all"
        );
    }

    const BLACK: Color = Color::new(0.0, 0.0, 0.0, 1.0);

    #[test]
    fn a_request_keeps_its_id_and_a_mirrored_one_has_its_own_reversed_pixels() {
        let icons = icons();
        let plain = icons.symbol("symbol.chevron.left", 24, BLACK, false);
        assert_eq!(icons.symbol("symbol.chevron.left", 24, BLACK, false), plain);
        let mirrored = icons.symbol("symbol.chevron.left", 24, BLACK, true);
        assert_ne!(mirrored.id, plain.id);
        let row = plain.image.width as usize * 4;
        for (a, b) in plain
            .image
            .pixels
            .chunks_exact(row)
            .zip(mirrored.image.pixels.chunks_exact(row))
        {
            let reversed: Vec<[u8; 4]> = a.as_chunks::<4>().0.iter().rev().copied().collect();
            assert_eq!(b.as_chunks::<4>().0, &reversed[..]);
        }
        assert_ne!(
            icons.symbol("symbol.chevron.left", 32, BLACK, false).id,
            plain.id
        );
    }

    #[test]
    fn a_name_that_is_not_an_icon_name_draws_the_placeholder() {
        let image = icons().symbol("../not a name", 16, BLACK, false);
        assert_eq!((image.image.width, image.image.height), (16, 16));
    }
}
