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
use cupertino_settings::live::{Device, Live, Network, Reading, Volume, Wifi};
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
        for pane in [
            PaneId::Wifi,
            PaneId::DesktopDock,
            PaneId::Sound,
            PaneId::Keyboard,
        ] {
            let sections = panes::sections(pane, &settings, session, &live);
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
            pm.save_png(out.join(format!("{}-{name}.png", pane.key())))
                .unwrap();
        }
        // the results window of a search that finds several panes
        let mut search = SearchField::default();
        for c in "scroll".chars() {
            search.type_char(c);
        }
        let (w, h) = popup::size(WIDTH_FACTOR, search.results().len(), RowSize::default());
        let rows = popup::rows(search.results().len(), w, RowSize::default());
        let mut pm = Pixmap::new(w as u32, h.ceil() as u32).unwrap();
        fill(&mut pm, pal.window_background);
        painter.paint_popup(&mut pm, 1.0, &search, &rows, Some(0), &pal);
        pm.save_png(out.join(format!("search-results-{name}.png")))
            .unwrap();
    }
    println!("wrote {}", out.display());
}
