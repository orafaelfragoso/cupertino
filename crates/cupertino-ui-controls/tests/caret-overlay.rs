//! The caret is drawn over the field, on whole device pixels of the window wherever the field lies, and its blink draws
//! only that: no node paints again.

mod support;

use cupertino_geometry::Insets;
use cupertino_ui_controls::{Stack, TextChange, TextField};
use cupertino_ui_core::{overlay_id, Application, Command, Element, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_platform::{Timestamp, WindowEvent};
use cupertino_ui_render::{PaintNodeId, PaintRevision};
use cupertino_ui_text::ContentPurpose;
use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use support::{window, Harness, TAB};

const MS: u64 = 1_000_000;

thread_local! {
    /// How far down the field sits, in points: off the device pixel grid at fractional scales.
    static TOP: Cell<f64> = const { Cell::new(0.0) };
}

struct Form;

impl Application for Form {
    type Model = String;
    type Message = TextChange;

    fn init() -> (String, Vec<Command<TextChange>>) {
        (String::new(), Vec::new())
    }

    fn update(model: &mut String, change: TextChange) -> Vec<Command<TextChange>> {
        *model = change.text;
        Vec::new()
    }

    fn view(model: &String) -> Vec<Window<TextChange>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets {
                top: TOP.with(Cell::get),
                ..Insets::ZERO
            },
            spacing: 0.0,
            align: Align::Start,
            children: vec![Element::new(TextField {
                label: "Name".into(),
                placeholder: String::new(),
                value: model.clone(),
                revision: 0,
                purpose: ContentPurpose::Name,
                enabled: true,
                on_change: Some(Box::new(|change| change)),
                on_submit: None,
                selection: None,
                initial: false,
            })
            .key("name")],
        }))
    }
}

/// A focused, empty field `top` points down a window at `scale`: its caret in the on phase of its blink.
fn focused(top: f64, scale: f64) -> Harness<Form> {
    TOP.set(top);
    let mut h = Harness::<Form>::new();
    h.send(WindowEvent::Scale(scale));
    h.tap(TAB);
    assert!(h.focused("name"));
    h
}

fn at(h: &mut Harness<Form>, ms: u64) {
    h.clock().advance_to(Timestamp(ms * MS)).unwrap();
    h.settle();
}

/// The window's last presented pixels: width, height and 4 bytes a pixel.
fn pixels(h: &Harness<Form>) -> (usize, Vec<u8>) {
    let frames = h.rt.window_system().frames(h.window).unwrap();
    let frame = frames.last().expect("a presented frame");
    (frame.width as usize, frame.pixels.clone())
}

#[test]
fn every_pixel_of_the_caret_is_covered_whole_or_not_at_all_at_fractional_scales() {
    for scale in [1.25, 1.5, 1.75] {
        for top in [10.1, 10.3, 10.45, 10.7] {
            let mut h = focused(top, scale);
            let (width, on) = pixels(&h);
            at(&mut h, 500);
            let (_, off) = pixels(&h);
            assert_eq!(on.len(), off.len());
            // the caret's pixels: what the blink changed
            let changed: Vec<(usize, usize)> = (0..on.len() / 4)
                .filter(|&i| on[4 * i..4 * i + 4] != off[4 * i..4 * i + 4])
                .map(|i| (i % width, i / width))
                .collect();
            let case = format!("at {scale}x, {top} points down");
            assert!(!changed.is_empty(), "the caret blinked {case}");
            let xs: BTreeSet<usize> = changed.iter().map(|p| p.0).collect();
            let ys: BTreeSet<usize> = changed.iter().map(|p| p.1).collect();
            let (columns, rows) = (
                xs.last().unwrap() - xs.first().unwrap() + 1,
                ys.last().unwrap() - ys.first().unwrap() + 1,
            );
            assert_eq!(
                changed.len(),
                columns * rows,
                "the caret is a solid rectangle {case}"
            );
            // a pixel the caret covers in part blends with the field under it: every one must be the caret's colour
            let (x, y) = changed[0];
            let caret = &on[4 * (y * width + x)..4 * (y * width + x) + 4];
            for &(x, y) in &changed {
                let i = 4 * (y * width + x);
                assert_eq!(
                    &on[i..i + 4],
                    caret,
                    "pixel ({x}, {y}) is covered in part {case}: {columns}×{rows} caret"
                );
            }
            assert_eq!(
                columns,
                scale.round().max(1.0) as usize,
                "one point wide in whole pixels {case}"
            );
        }
    }
}

/// Each node's paint revision in the window's display list.
fn revisions(h: &Harness<Form>) -> HashMap<PaintNodeId, PaintRevision> {
    h.snapshot()
        .display_list
        .nodes
        .iter()
        .map(|n| (n.id, n.revision))
        .collect()
}

#[test]
fn the_blink_paints_no_node_again_and_only_the_overlay_changes() {
    let mut h = focused(10.3, 1.5);
    let field = h.rt.find(h.window, &"name".into()).unwrap();
    let overlay = overlay_id(field);
    let before = revisions(&h);
    assert!(before.contains_key(&overlay), "the caret shows");
    let frames = h.rt.window_system().frames(h.window).unwrap().len();
    for (ms, shown) in [(500, false), (1000, true), (1500, false), (2000, true)] {
        at(&mut h, ms);
        let now = revisions(&h);
        assert!(
            h.rt.window_system().frames(h.window).unwrap().len() > frames,
            "the blink at {ms} ms drew a frame"
        );
        assert_eq!(now.contains_key(&overlay), shown, "at {ms} ms");
        let nodes: Vec<_> = before.keys().filter(|id| **id != overlay).collect();
        let hits = nodes
            .iter()
            .filter(|id| now.get(id) == before.get(id))
            .count();
        assert_eq!(hits, nodes.len(), "every node kept its paint at {ms} ms");
    }
    // a caret drawn again in the same place is the same paint
    assert_eq!(revisions(&h).get(&overlay), before.get(&overlay));
}
