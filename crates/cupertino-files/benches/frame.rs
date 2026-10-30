//! What one Files frame costs on the CPU at a 4K output: clearing the buffer, painting the window and converting it to
//! the shm pixel order, each the best of several runs.
//!
//! `cargo bench -p cupertino-files --features app --bench frame`

use cupertino_assets::Assets;
use cupertino_files::browser::Browser;
use cupertino_files::places::{Place, PlaceGroup, PlaceKind};
use cupertino_files::render::{Palette, Renderer};
use cupertino_files::window::{Bars, ViewMode, WindowPainter, WindowState};
use cupertino_files::{Size, SortSpec};
use cupertino_icons::document::FileIcons;
use cupertino_text_field::TextField;
use cupertino_ui_wayland::rgba_to_argb8888;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tiny_skia::{Color, Pixmap};

const RUNS: usize = 15;

fn best(mut f: impl FnMut()) -> Duration {
    (0..RUNS)
        .map(|_| {
            let t0 = Instant::now();
            f();
            t0.elapsed()
        })
        .min()
        .expect("runs")
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn main() {
    let dir = std::env::temp_dir().join(format!("cupertino-frame-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..40 {
        std::fs::write(dir.join(format!("file {i:02}.txt")), b"x").unwrap();
    }
    let mut browser = Browser::open(
        dir.clone(),
        SortSpec::default(),
        PathBuf::from("/nonexistent/recently-used.xbel"),
        None,
        Arc::new(|| {}),
    )
    .unwrap();
    while browser.entries().len() < 40 {
        browser.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    let places = vec![
        Place {
            name: "Home".into(),
            path: PathBuf::from("/home/ada"),
            group: PlaceGroup::Favorites,
            kind: PlaceKind::Home,
        },
        Place {
            name: "Computer".into(),
            path: PathBuf::from("/"),
            group: PlaceGroup::Locations,
            kind: PlaceKind::Computer,
        },
    ];
    let assets = Assets::builtin();
    let icons = FileIcons::new(
        &assets,
        cupertino_apps::MimeDatabase::load(&[]).unwrap(),
        std::collections::HashMap::new(),
    );
    let mut painter = WindowPainter::new(Renderer::new(icons, &assets).unwrap());
    let pal = Palette::builtin(false);
    let search = TextField::new();
    for (w, h, scale) in [(2560.0, 1440.0, 1.5_f32), (1920.0, 1080.0, 2.0)] {
        let frame = painter.frame(
            Size { w, h },
            &places,
            Bars {
                path: true,
                status: true,
            },
            112.0,
            &dir,
            ViewMode::List,
            browser.outline(),
            0.0,
            scale,
        );
        let state = WindowState {
            browser: &browser,
            places: &places,
            mode: ViewMode::List,
            active: true,
            search: &search,
            search_focused: false,
            search_caret: false,
            title: "Here",
            now: SystemTime::now(),
            edit: None,
            edit_caret: false,
            drop_target: None,
            increase_contrast: false,
            marquee: None,
            status: "40 items",
        };
        let (pw, ph) = ((w * f64::from(scale)) as u32, (h * f64::from(scale)) as u32);
        let mut pm = Pixmap::new(pw, ph).unwrap();
        let mut shm = vec![0u8; pm.data().len()];
        let clear = best(|| pm.fill(Color::TRANSPARENT));
        let paint = best(|| {
            pm.fill(Color::TRANSPARENT);
            painter.paint(&mut pm, scale, &frame, &state, &pal);
        }) - clear;
        let convert = best(|| rgba_to_argb8888(pm.data(), &mut shm));
        // a damaged frame: only the rows of the search field's caret blink, and of a hovered list row
        let s = f64::from(scale);
        let rows_of = |areas: &[cupertino_geometry::Rect]| -> Vec<(u32, u32)> {
            areas
                .iter()
                .map(|r| {
                    (
                        (r.y() * s).floor() as u32,
                        ((r.y() + r.height()) * s).ceil() as u32,
                    )
                })
                .collect()
        };
        let caret = rows_of(&cupertino_files::window::caret_areas(&frame, None).unwrap());
        let row_top = ((frame.window.content.y + 100.0) * s) as u32;
        let hover = [(row_top, row_top + (24.0 * s) as u32)];
        // a drag moving its drop outline from one item to the next, and a rubber band growing by 40 pt
        let outline =
            rows_of(&[3, 4].map(|i| cupertino_files::window::item_area(&frame, i).unwrap()));
        let origin = cupertino_files::Point { x: 40.0, y: 60.0 };
        let band = rows_of(&[300.0, 340.0].map(|y| {
            cupertino_files::window::marquee_area(
                &frame,
                origin,
                cupertino_files::Point { x: 600.0, y },
            )
            .unwrap()
        }));
        let mut damaged = |rows: &[(u32, u32)]| {
            best(|| {
                pm.fill(Color::TRANSPARENT);
                painter.paint_rows(&mut pm, scale, &frame, &state, &pal, rows);
            })
            .saturating_sub(clear)
        };
        let (caret_paint, hover_paint) = (damaged(&caret), damaged(&hover));
        let (outline_paint, band_paint) = (damaged(&outline), damaged(&band));
        println!(
            "  damaged paint: search caret {:.2} ms, one list row {:.2} ms, drop outline move {:.2} ms, rubber band move {:.2} ms",
            ms(caret_paint),
            ms(hover_paint),
            ms(outline_paint),
            ms(band_paint)
        );
        // the parts: the content view's own pixmap, its blit, a full-window opaque fill
        let content = frame.window.content;
        let size = Size {
            w: content.w,
            h: content.h,
        };
        let mut view = None;
        let list = best(|| {
            view = Some(painter.renderer.list_view(
                browser.entries(),
                browser.outline(),
                &[],
                true,
                &painter.list,
                browser.sort(),
                size,
                0.0,
                None,
                scale,
                &pal,
                SystemTime::now(),
            ))
        });
        let view = view.unwrap();
        let blit = best(|| {
            pm.draw_pixmap(
                0,
                0,
                view.as_ref(),
                &tiny_skia::PixmapPaint::default(),
                tiny_skia::Transform::identity(),
                None,
            )
        });
        let mut white = tiny_skia::Paint::default();
        white.set_color_rgba8(255, 255, 255, 255);
        let full = tiny_skia::Rect::from_xywh(0.0, 0.0, pw as f32, ph as f32).unwrap();
        let fill = best(|| pm.fill_rect(full, &white, tiny_skia::Transform::identity(), None));
        // the alternatives: the view copied in by rows (it is opaque), the buffer copied as is (a native byte order shm
        // format), and only the band a scroll of 40 pt exposes rendered
        let opaque = view.pixels().iter().all(|p| p.alpha() == 255);
        let row = view.width() as usize * 4;
        let stride = pw as usize * 4;
        let copy_rows = best(|| {
            let (src, dst) = (view.data(), pm.data_mut());
            for y in 0..view.height() as usize {
                dst[y * stride..y * stride + row].copy_from_slice(&src[y * row..(y + 1) * row]);
            }
        });
        let memcpy = best(|| shm.copy_from_slice(pm.data()));
        let band = best(|| {
            std::hint::black_box(painter.renderer.list_view(
                browser.entries(),
                browser.outline(),
                &[],
                true,
                &painter.list,
                browser.sort(),
                Size { w: size.w, h: 40.0 },
                0.0,
                None,
                scale,
                &pal,
                SystemTime::now(),
            ));
        });
        let scroll_shift = best(|| {
            let d = pm.data_mut();
            let shift = (40.0 * scale) as usize * stride;
            d.copy_within(shift.., 0);
        });
        println!(
            "  alternatives: view opaque {opaque}, copied by rows {:.2} ms; buffer memcpy {:.2} ms; a 40 pt band of the list {:.2} ms; shifting the buffer for a scroll {:.2} ms",
            ms(copy_rows),
            ms(memcpy),
            ms(band),
            ms(scroll_shift)
        );
        println!(
            "  parts: list view {:.2} ms ({}x{} px), its blit {:.2} ms, one full-window opaque fill {:.2} ms",
            ms(list),
            view.width(),
            view.height(),
            ms(blit),
            ms(fill)
        );
        println!(
            "{w}x{h}@{scale} ({pw}x{ph} px): clear {:.2} ms, paint {:.2} ms, convert to shm {:.2} ms, total {:.2} ms",
            ms(clear),
            ms(paint),
            ms(convert),
            ms(clear + paint + convert)
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
