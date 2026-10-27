//! Renders the ST-23 samples into `output/`: the Wi-Fi, Desktop & Dock, Sound and Keyboard panes in the light and the
//! dark appearance (715 × 625 at 1×), and the search results window.
//!
//! `cargo run -p cupertino-settings --example settings_samples`
//!
//! Needs `CUPERTINO_ASSET_DIR` (fonts and symbols). The settings are the defaults (no files are read); the live
//! readings are example data, not a machine's. The sidebar material is the server's: here a flat fill of the window
//! background stands in for it.

use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_settings::form::{self, RowSpec, SectionSpec};
use cupertino_settings::history::History;
use cupertino_settings::live::{Device, Live, Mouse, Network, Reading, Volume, Wifi};
use cupertino_settings::paint::{Painter, Palette, PaneView, WindowState};
use cupertino_settings::panes::{self, Session};
use cupertino_settings::search::{popup, SearchField};
use cupertino_settings::settings::{Paths, Settings};
use cupertino_settings::sidebar::{self, RowSize};
use cupertino_settings::window::{self, WindowLayout};
use cupertino_settings::PaneId;
use cupertino_theme::{Appearance, Theme};
use tiny_skia::{Color, Pixmap};

/// The interface width factor of English (approximate).
const WIDTH_FACTOR: f64 = 1.0;

/// Example readings: a Wi-Fi device with four networks and the mixer at 60 %.
/// A generated picture `name` under `scratch` with its thumbnail set on the painter.
fn picture(
    painter: &mut Painter,
    scratch: &std::path::Path,
    name: &str,
    (from, to): ([u8; 3], [u8; 3]),
) -> cupertino_assets::WallpaperFile {
    let mut pm = Pixmap::new(256, 160).unwrap();
    for (i, px) in pm.pixels_mut().iter_mut().enumerate() {
        let t = (i / 256) as f32 / 159.0;
        let mix = |a: u8, b: u8| (f32::from(a) * (1.0 - t) + f32::from(b) * t) as u8;
        *px = tiny_skia::ColorU8::from_rgba(
            mix(from[0], to[0]),
            mix(from[1], to[1]),
            mix(from[2], to[2]),
            255,
        )
        .premultiply();
    }
    let image = scratch.join(format!("{name}.jpg"));
    let thumbnail = scratch.join(format!("{name}-thumb.png"));
    pm.save_png(&thumbnail).unwrap();
    painter.set_thumbnail(&image, &thumbnail);
    cupertino_assets::WallpaperFile {
        name: name.to_string(),
        light: image,
        dark: None,
        credit: None,
    }
}

/// [`picture`] with a title and a photographer, and a dark file when `dark`.
fn credited(
    painter: &mut Painter,
    scratch: &std::path::Path,
    name: &str,
    title: &str,
    author: &str,
    colors: ([u8; 3], [u8; 3]),
    dark: bool,
) -> cupertino_assets::WallpaperFile {
    let mut file = picture(painter, scratch, name, colors);
    file.dark = dark.then(|| scratch.join(format!("{name}-dark.jpg")));
    file.credit = Some(cupertino_assets::Credit {
        title: title.to_string(),
        author: author.to_string(),
    });
    file
}

fn live() -> Live {
    let network = |ssid: &str, signal, security: &str, saved, connected| Network {
        ssid: ssid.into(),
        signal,
        security: security.into(),
        saved,
        connected,
    };
    let wifi = Wifi {
        enabled: true,
        devices: vec![Device {
            index: 2,
            name: "wlan0".into(),
            kind: "wifi".into(),
            state: "connected".into(),
            ssid: "Home".into(),
            failure: String::new(),
            address: "10.0.0.2".into(),
        }],
        state: "connected".into(),
        networks: vec![
            network("Home", -42, "wpa2", true, true),
            network("Office", -61, "wpa2", true, false),
            network("Cafe", -70, "open", false, false),
            network("Neighbour", -80, "wpa2", false, false),
        ],
        joining: None,
        failure: None,
    };
    Live {
        wifi: Reading::Ready(wifi),
        volume: Reading::Ready(Volume {
            level: 0.6,
            muted: false,
        }),
        battery: Reading::Ready(None),
        ..Live::default()
    }
}

fn fill(pm: &mut Pixmap, rgba: [f32; 4]) {
    pm.fill(
        Color::from_rgba(rgba[0], rgba[1], rgba[2], rgba[3]).expect("palette colours are in range"),
    );
}

fn main() {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("output");
    std::fs::create_dir_all(&out).unwrap();
    let assets = cupertino_assets::Assets::discover();
    let theme = Theme::load().unwrap_or_else(|e| panic!("{e}"));
    let mut painter = Painter::new(&assets).unwrap_or_else(|e| panic!("{e}"));
    let paths = Paths {
        dock: None,
        compositor: None,
        menubar: None,
        keyboard: None,
        power: None,
        accessibility: None,
    };
    let settings = Settings::load(&paths, 1080.0).unwrap_or_else(|e| panic!("{e}"));
    let live = live();
    let mice = |list: &[(&str, &str, u32, bool)]| Live {
        mice: Reading::Ready(
            list.iter()
                .map(|(key, name, dpi, stored)| Mouse {
                    key: (*key).into(),
                    name: (*name).into(),
                    dpi: *dpi,
                    stored: *stored,
                })
                .collect(),
        ),
        ..live.clone()
    };
    // the Wallpaper pane with generated pictures: gradients under a temporary folder, their thumbnails set directly
    let scratch =
        std::env::temp_dir().join(format!("cupertino-settings-samples-{}", std::process::id()));
    std::fs::create_dir_all(scratch.join("Trips")).unwrap();
    let bundled = vec![
        credited(
            &mut painter,
            &scratch,
            "dusk",
            "Dusk",
            "A. Tester",
            ([250, 170, 120], [60, 50, 120]),
            false,
        ),
        credited(
            &mut painter,
            &scratch,
            "ridge",
            "Ridge",
            "B. Tester",
            ([235, 235, 240], [120, 130, 150]),
            true,
        ),
        credited(
            &mut painter,
            &scratch,
            "ocean",
            "Ocean",
            "C. Tester",
            ([120, 190, 230], [10, 60, 120]),
            false,
        ),
    ];
    let folder = vec![
        picture(
            &mut painter,
            &scratch,
            "meadow",
            ([170, 220, 170], [40, 110, 90]),
        ),
        picture(
            &mut painter,
            &scratch,
            "plum",
            ([200, 140, 200], [70, 30, 90]),
        ),
    ];
    let current = bundled[1].light.clone();
    let wallpaper_live = Live {
        pictures: cupertino_settings::live::Pictures {
            bundled,
            folders: vec![(scratch.join("Trips"), folder)],
            default: Some(current),
        },
        ..live.clone()
    };
    let one_mouse = mice(&[(
        "046d:c077:USB Optical Mouse",
        "USB Optical Mouse",
        800,
        false,
    )]);
    let two_mice = mice(&[
        (
            "046d:c077:USB Optical Mouse",
            "USB Optical Mouse",
            1600,
            true,
        ),
        ("1532:0043:Gaming Mouse", "Gaming Mouse", 2400, true),
    ]);
    let size = window::default_size(WIDTH_FACTOR);
    let layout = WindowLayout::new(
        size,
        WIDTH_FACTOR,
        RowSize::default(),
        &sidebar::groups(),
        false,
    );
    for (dark, name) in [(false, "light"), (true, "dark")] {
        let pal = Palette::from_theme(
            &theme,
            if dark {
                Appearance::Dark
            } else {
                Appearance::Light
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let session = Session {
            dark,
            display: DisplayOptions {
                reduce_motion: false,
                reduce_transparency: false,
                increase_contrast: false,
                differentiate_without_color: false,
            },
        };
        for (pane, tag, live) in [
            (PaneId::Wifi, "", &live),
            (PaneId::DesktopDock, "", &live),
            (PaneId::Sound, "", &live),
            (PaneId::Keyboard, "", &live),
            (PaneId::Mouse, "-one-mouse", &one_mouse),
            (PaneId::Wallpaper, "", &wallpaper_live),
            (PaneId::Mouse, "-two-mice", &two_mice),
        ] {
            let sections = panes::sections(pane, &settings, session, live);
            let specs: Vec<SectionSpec<'_>> = sections
                .iter()
                .map(|s| SectionSpec {
                    header: s.header.is_some(),
                    rows: s
                        .rows
                        .iter()
                        .map(|r| RowSpec {
                            control: &r.control,
                            error: r.error.is_some() || r.disabled.is_some(),
                        })
                        .collect(),
                    footer: s.footer.is_some(),
                })
                .collect();
            let (form, _) =
                form::layout(&specs, layout.content.x, layout.content.w, layout.content.y);
            let history = History::new(pane);
            let search = SearchField::default();
            let state = WindowState {
                selected: pane,
                has_battery: live.has_battery(),
                active: true,
                history: &history,
                search: &search,
                search_focused: false,
                search_caret: false,
            };
            let mut pm = Pixmap::new(size.w as u32, size.h as u32).unwrap();
            fill(&mut pm, pal.window_background);
            painter.paint_window(
                &mut pm,
                1.0,
                &layout,
                &state,
                &PaneView {
                    sections: &sections,
                    layout: &form,
                    scroll: 0.0,
                    dragging: None,
                },
                &pal,
            );
            pm.save_png(out.join(format!("{}{tag}-{name}.png", pane.key())))
                .unwrap();
        }
        // the chooser sheet over the Wallpaper pane
        {
            let layout = layout.clone();
            let picks = scratch.join("Pictures");
            std::fs::create_dir_all(picks.join("Summer")).unwrap();
            std::fs::create_dir_all(picks.join("Screenshots")).unwrap();
            for f in ["beach.jpg", "city.png", "mountain.jpg", "notes.txt"] {
                std::fs::write(picks.join(f), b"x").unwrap();
            }
            let mut sheet =
                cupertino_settings::sheet::Sheet::picker(&picks, cupertino_filepicker::Mode::Photo);
            sheet.key(cupertino_settings::sheet::SheetKey::Down);
            sheet.key(cupertino_settings::sheet::SheetKey::Down);
            sheet.key(cupertino_settings::sheet::SheetKey::Down);
            let sheet_layout = sheet.layout(&layout.content);
            let sections = panes::sections(PaneId::Wallpaper, &settings, session, &wallpaper_live);
            let history = History::new(PaneId::Wallpaper);
            let search = SearchField::default();
            let state = WindowState {
                selected: PaneId::Wallpaper,
                has_battery: live.has_battery(),
                active: true,
                history: &history,
                search: &search,
                search_focused: false,
            };
            let specs: Vec<SectionSpec<'_>> = sections
                .iter()
                .map(|sec| SectionSpec {
                    header: sec.header.is_some(),
                    rows: sec
                        .rows
                        .iter()
                        .map(|r| RowSpec {
                            control: &r.control,
                            error: false,
                        })
                        .collect(),
                    footer: sec.footer.is_some(),
                })
                .collect();
            let (form, _) =
                form::layout(&specs, layout.content.x, layout.content.w, layout.content.y);
            let mut pm = Pixmap::new(size.w as u32, size.h as u32).unwrap();
            fill(&mut pm, pal.window_background);
            painter.paint_window(
                &mut pm,
                1.0,
                &layout,
                &state,
                &PaneView {
                    sections: &sections,
                    layout: &form,
                    scroll: 0.0,
                    dragging: None,
                },
                &pal,
            );
            painter.paint_sheet(&mut pm, 1.0, &sheet, &sheet_layout, &layout.content, &pal);
            pm.save_png(out.join(format!("wallpaper-chooser-{name}.png")))
                .unwrap();
        }
        // the results window of a search that finds several panes
        let mut search = SearchField::default();
        search.edit(|f| f.insert("scroll"));
        let (w, h) = popup::size(WIDTH_FACTOR, search.results().len(), RowSize::default());
        let rows = popup::rows(search.results().len(), w, RowSize::default());
        let mut pm = Pixmap::new(w as u32, h.ceil() as u32).unwrap();
        fill(&mut pm, pal.window_background);
        painter.paint_popup(&mut pm, 1.0, &search, &rows, Some(0), &pal);
        pm.save_png(out.join(format!("search-results-{name}.png")))
            .unwrap();
    }
    std::fs::remove_dir_all(&scratch).ok();
    println!("wrote {}", out.display());
}
