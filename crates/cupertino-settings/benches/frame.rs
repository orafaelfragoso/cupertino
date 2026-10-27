//! What one Settings frame costs on the CPU at a 4K output: clearing the buffer and painting the window, the best of
//! several runs.
//!
//! `cargo bench -p cupertino-settings --bench frame`

use cupertino_assets::Assets;
use cupertino_settings::history::History;
use cupertino_settings::paint::{Painter, Palette, PaneView, WindowState};
use cupertino_settings::search::SearchField;
use cupertino_settings::sidebar::{self, RowSize};
use cupertino_settings::window::WindowLayout;
use cupertino_settings::{PaneId, Size};
use cupertino_theme::{Appearance, Theme};
use std::time::{Duration, Instant};
use tiny_skia::{Color, Pixmap};

fn best(mut f: impl FnMut()) -> Duration {
    (0..15)
        .map(|_| {
            let t0 = Instant::now();
            f();
            t0.elapsed()
        })
        .min()
        .expect("runs")
}

fn main() {
    let theme = Theme::load().expect("the design tokens of the repository");
    let pal = Palette::from_theme(&theme, Appearance::Light).unwrap();
    let mut painter = Painter::new(&Assets::builtin()).unwrap();
    let history = History::new(PaneId::Appearance);
    let search = SearchField::default();
    for (w, h, scale) in [(2560.0, 1440.0, 1.5_f32), (1920.0, 1080.0, 2.0)] {
        let layout = WindowLayout::new(
            Size { w, h },
            1.0,
            RowSize::Medium,
            &sidebar::groups(),
            false,
        );
        let state = WindowState {
            selected: PaneId::Appearance,
            has_battery: None,
            active: true,
            history: &history,
            search: &search,
            search_focused: false,
            search_caret: false,
        };
        let view = PaneView {
            sections: &[],
            layout: &[],
            scroll: 0.0,
            dragging: None,
        };
        let (pw, ph) = ((w * f64::from(scale)) as u32, (h * f64::from(scale)) as u32);
        let mut pm = Pixmap::new(pw, ph).unwrap();
        let clear = best(|| pm.fill(Color::TRANSPARENT));
        let paint = best(|| {
            pm.fill(Color::TRANSPARENT);
            painter.paint_window(&mut pm, scale, &layout, &state, &view, &pal);
        }) - clear;
        println!(
            "{w}x{h}@{scale} ({pw}x{ph} px): clear {:.2} ms, paint {:.2} ms",
            clear.as_secs_f64() * 1e3,
            paint.as_secs_f64() * 1e3
        );
    }
}
