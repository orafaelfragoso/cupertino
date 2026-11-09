//! The caret is drawn over the field, on whole device pixels of the window wherever the field lies, and its blink draws
//! only that: no node paints again. The composition's underline and the selection, a field's and an editor's, lie on
//! whole device pixels too.

mod support;

use cupertino_geometry::Insets;
use cupertino_ui_controls::{Stack, TextChange, TextEditor, TextField};
use cupertino_ui_core::{overlay_id, Application, Command, Element, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_platform::{Mods, Timestamp, WindowEvent};
use cupertino_ui_render::{PaintNodeId, PaintRevision};
use cupertino_ui_text::{ContentPurpose, ImeBatch, ImeEvent, Preedit};
use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use support::{window, Harness, TAB};

const MS: u64 = 1_000_000;

thread_local! {
    /// How far down and in the field sits, in points: off the device pixel grid at fractional scales.
    static TOP: Cell<f64> = const { Cell::new(0.0) };
    /// The window holds a multi-line editor instead of the field.
    static EDITOR: Cell<bool> = const { Cell::new(false) };
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
                left: TOP.with(Cell::get),
                ..Insets::ZERO
            },
            spacing: 0.0,
            align: Align::Start,
            children: vec![if EDITOR.with(Cell::get) {
                Element::new(TextEditor {
                    label: "Name".into(),
                    placeholder: String::new(),
                    value: model.clone(),
                    revision: 0,
                    enabled: true,
                    bordered: false,
                    on_change: Some(Box::new(|change| change)),
                })
                .key("name")
            } else {
                Element::new(TextField {
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
                .key("name")
            }],
        }))
    }
}

/// A focused, empty field `top` points down and in a window at `scale`: its caret in the on phase of its blink.
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

/// The window's pixels drawn whole, not over the last frame: a frame at another scale, then one at `scale` again.
fn whole(h: &mut Harness<Form>, scale: f64) -> (usize, Vec<u8>) {
    h.send(WindowEvent::Scale(scale + 0.25));
    h.send(WindowEvent::Scale(scale));
    h.settle();
    pixels(h)
}

/// The pixels `on` (a window `width` pixels wide) changed from `off` are one solid rectangle of one colour: none is
/// covered in part and blended with what lies under it. Its size in pixels.
fn solid(width: usize, on: &[u8], off: &[u8], case: &str) -> (usize, usize) {
    assert_eq!(on.len(), off.len());
    let changed: Vec<(usize, usize)> = (0..on.len() / 4)
        .filter(|&i| on[4 * i..4 * i + 4] != off[4 * i..4 * i + 4])
        .map(|i| (i % width, i / width))
        .collect();
    assert!(!changed.is_empty(), "the mark drew {case}");
    let xs: BTreeSet<usize> = changed.iter().map(|p| p.0).collect();
    let ys: BTreeSet<usize> = changed.iter().map(|p| p.1).collect();
    let (columns, rows) = (
        xs.last().unwrap() - xs.first().unwrap() + 1,
        ys.last().unwrap() - ys.first().unwrap() + 1,
    );
    assert_eq!(
        changed.len(),
        columns * rows,
        "the mark is a solid rectangle {case}"
    );
    // a pixel the mark covers in part blends with the field under it: every one must be the mark's colour
    let (x, y) = changed[0];
    let mark = &on[4 * (y * width + x)..4 * (y * width + x) + 4];
    for &(x, y) in &changed {
        let i = 4 * (y * width + x);
        assert_eq!(
            &on[i..i + 4],
            mark,
            "pixel ({x}, {y}) is covered in part {case}: {columns}×{rows} mark"
        );
    }
    (columns, rows)
}

#[test]
fn every_pixel_of_the_caret_is_covered_whole_or_not_at_all_at_fractional_scales() {
    for (editor, scale, top) in cases() {
        EDITOR.set(editor);
        let mut h = focused(top, scale);
        let (width, on) = pixels(&h);
        at(&mut h, 500);
        let (_, off) = pixels(&h);
        // the caret's pixels: what the blink changed
        let what = if editor { "an editor" } else { "a field" };
        let case = format!("in {what} at {scale}x, {top} points down and in");
        let (columns, _) = solid(width, &on, &off, &case);
        assert_eq!(
            columns,
            scale.round().max(1.0) as usize,
            "one point wide in whole pixels {case}"
        );
    }
    EDITOR.set(false);
}

#[test]
fn every_pixel_of_the_compositions_underline_is_covered_whole_or_not_at_all_at_fractional_scales() {
    for (editor, scale, top) in cases() {
        EDITOR.set(editor);
        let mut h = focused(top, scale);
        // the caret in the off phase of its blink; both frames drawn whole, so what differs is what the composition drew
        at(&mut h, 500);
        let (width, off) = whole(&mut h, scale);
        // spaces with no caret: what changes is the underline alone
        h.send(WindowEvent::Ime(ImeEvent::Done(ImeBatch {
            preedit: Some(Preedit {
                text: "     ".into(),
                cursor: None,
            }),
            current: true,
            ..ImeBatch::default()
        })));
        h.settle();
        let (_, on) = whole(&mut h, scale);
        let what = if editor { "an editor" } else { "a field" };
        let case = format!("in {what} at {scale}x, {top} points down and in");
        let (columns, rows) = solid(width, &on, &off, &case);
        assert!(columns > 10, "{columns}×{rows} underline {case}");
    }
    EDITOR.set(false);
}

/// A field and an editor, at each fractional scale, at offsets around the device pixel grid.
fn cases() -> impl Iterator<Item = (bool, f64, f64)> {
    [false, true]
        .into_iter()
        .flat_map(|e| [1.25, 1.5, 1.75].map(|s| (e, s)))
        .flat_map(|(e, s)| [10.1, 10.3, 10.45, 10.7].map(|t| (e, s, t)))
}

#[test]
fn every_pixel_of_the_selection_is_covered_whole_or_not_at_all_at_fractional_scales() {
    for (editor, scale, top) in cases() {
        EDITOR.set(editor);
        let mut h = focused(top, scale);
        // spaces: no glyph inside the selection
        h.type_text("     ");
        assert_eq!(h.model(), "     ");
        // the caret in the off phase of its blink, so selecting changes the selection's pixels alone
        at(&mut h, 2_500);
        let (width, off) = pixels(&h);
        h.key_event(0x61, true, false, Mods::PRIMARY);
        h.key_event(0x61, false, false, Mods::PRIMARY);
        h.settle();
        let (_, on) = pixels(&h);
        let what = if editor { "an editor" } else { "a field" };
        let case = format!("in {what} at {scale}x, {top} points down and in");
        let (columns, rows) = solid(width, &on, &off, &case);
        assert!(
            columns > 10 && rows > 10,
            "{columns}×{rows} selection {case}"
        );
    }
    EDITOR.set(false);
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
