use cupertino_geometry::Rect;
use cupertino_ui_core::{AccessibilityNode, AccessibleValue, FrameSnapshot};
use cupertino_ui_render::PaintOp;
use std::fmt::Write;

/// Deterministic text dumps of a runtime's layout, accessibility tree and display lists, for golden tests.
///
/// A dump has four sections, in this order: pending work and focus, layout, semantics, display list. A protected
/// value shows as `<protected>`, and glyph runs show their count and position but not which glyphs they hold, so a
/// secret cannot reach a failure message through a dump.
#[derive(Debug, Default)]
pub struct Inspector {
    _private: (),
}

fn rect(r: &Rect) -> String {
    format!(
        "{},{} {}x{}",
        r.origin.x, r.origin.y, r.size.width, r.size.height
    )
}

fn node(out: &mut String, id: impl std::fmt::Debug, n: &AccessibilityNode) {
    let value = match &n.value {
        None => "none".to_string(),
        Some(AccessibleValue::Protected) => "<protected>".to_string(),
        Some(other) => format!("{other:?}"),
    };
    let live = n.live.map(|l| format!(" live={l:?}")).unwrap_or_default();
    let items = n
        .item_count
        .map(|c| format!(" items={c}"))
        .unwrap_or_default();
    let index = n
        .item_index
        .map(|i| format!(" index={i}"))
        .unwrap_or_default();
    let level = n.level.map(|l| format!(" level={l}")).unwrap_or_default();
    let column = n
        .column_index
        .map(|c| format!(" column={c}"))
        .unwrap_or_default();
    let sort = n.sort.map(|s| format!(" sort={s:?}")).unwrap_or_default();
    let _ = writeln!(
        out,
        "  {id:?} role={:?} name={:?} description={:?} value={value}{live}{items}{index}{level}{column}{sort} states={:?} bounds=[{}] actions={:?} \
         relations={:?} children={:?}",
        n.role,
        n.name,
        n.description,
        n.states,
        rect(&n.bounds),
        n.actions,
        n.relations,
        n.children
    );
}

impl Inspector {
    pub fn dump(&self, frame: &FrameSnapshot) -> String {
        let mut out = String::new();
        let p = frame.pending;
        let _ = writeln!(out, "window {}", frame.window.0);
        let _ = writeln!(
            out,
            "pending messages={} tasks={} frames={} animating={} next_wake={:?}",
            p.messages,
            p.tasks,
            p.frames,
            p.animating,
            p.next_wake.map(|t| t.0)
        );
        let _ = writeln!(out, "focus {:?}", frame.focus);
        let _ = writeln!(out, "pointer_capture {:?}", frame.pointer_capture);
        let _ = writeln!(out, "text_input {:?}", frame.text_input);

        let _ = writeln!(out, "layout");
        for n in &frame.nodes {
            let clip = n.clip.as_ref().map_or("none".to_string(), rect);
            let _ = writeln!(
                out,
                "  {:?} {} key={:?} parent={:?} children={:?} rect=[{}] clip=[{clip}]",
                n.id,
                n.widget,
                n.key,
                n.parent,
                n.children,
                rect(&n.rect)
            );
        }

        let _ = writeln!(out, "semantics");
        for (id, n) in &frame.accessibility {
            node(&mut out, id, n);
        }

        let _ = writeln!(out, "display");
        for n in &frame.display_list.nodes {
            let _ = writeln!(
                out,
                "  paint {} parent={:?} revision={} bounds=[{}] hit=[{}] clip={:?} transform={:?}",
                n.id.0,
                n.parent.map(|p| p.0),
                n.revision.0,
                rect(&n.bounds),
                rect(&n.hit),
                n.clip,
                n.transform.0
            );
            for op in &n.ops {
                match op {
                    PaintOp::Glyphs { origin, run, color } => {
                        let _ = writeln!(
                            out,
                            "    Glyphs origin={},{} font={:?} size={} count={} color={color:?}",
                            origin.x,
                            origin.y,
                            run.font,
                            run.size,
                            run.glyphs.len()
                        );
                    }
                    other => {
                        let _ = writeln!(out, "    {other:?}");
                    }
                }
            }
        }
        out
    }

    /// Describes the first line where `actual` differs from `expected`, or `None` when they are equal.
    pub fn first_difference(expected: &str, actual: &str) -> Option<String> {
        let mut e = expected.lines();
        let mut a = actual.lines();
        for line in 1.. {
            match (e.next(), a.next()) {
                (None, None) => return None,
                (Some(x), Some(y)) if x == y => {}
                (x, y) => {
                    return Some(format!(
                        "line {line}:\n  expected: {}\n  actual:   {}",
                        x.unwrap_or("<end>"),
                        y.unwrap_or("<end>")
                    ));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_geometry::{Point, Size};
    use cupertino_ui_core::{
        AccessibleStates, Action, NodeId, NodeSnapshot, Pending, Role, SortOrder,
    };
    use cupertino_ui_platform::{Timestamp, WindowId};
    use cupertino_ui_render::{DisplayList, PaintNode, PaintNodeId, PaintRevision, Transform};
    use std::num::NonZeroU32;

    fn id(index: u32) -> NodeId {
        NodeId::new(index, NonZeroU32::new(1).unwrap())
    }

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect {
            origin: Point { x, y },
            size: Size {
                width: w,
                height: h,
            },
        }
    }

    fn snapshot(value: AccessibleValue) -> FrameSnapshot {
        FrameSnapshot {
            window: WindowId(1),
            nodes: vec![NodeSnapshot {
                id: id(0),
                parent: None,
                children: vec![],
                widget: "Field",
                key: None,
                rect: r(0.0, 0.0, 200.0, 24.0),
                clip: Some(r(0.0, 0.0, 100.0, 24.0)),
            }],
            focus: Some(id(0)),
            pointer_capture: None,
            text_input: Some(id(0)),
            accessibility: vec![(
                id(0),
                AccessibilityNode {
                    role: Role::PasswordInput,
                    name: Some("Password".into()),
                    description: None,
                    value: Some(value),
                    states: AccessibleStates::default(),
                    bounds: r(0.0, 0.0, 200.0, 24.0),
                    actions: vec![Action::Focus],
                    relations: vec![],
                    children: vec![],
                    live: None,
                    item_count: None,
                    level: None,
                    item_index: None,
                    column_index: None,
                    sort: None,
                },
            )],
            accessibility_focus: Some(id(0)),
            display_list: DisplayList {
                nodes: vec![PaintNode {
                    layer: None,
                    id: PaintNodeId(7),
                    parent: None,
                    revision: PaintRevision(2),
                    transform: Transform::IDENTITY,
                    clip: None,
                    group: None,
                    bounds: r(0.0, 0.0, 200.0, 24.0),
                    hit: r(0.0, 0.0, 200.0, 24.0),
                    ops: vec![PaintOp::PopClip],
                }],
            },
            pending: Pending {
                messages: 1,
                tasks: 2,
                frames: 3,
                animating: true,
                next_wake: Some(Timestamp(500)),
            },
            layout: Default::default(),
        }
    }

    #[test]
    fn a_dump_lists_pending_work_layout_semantics_and_display_in_order() {
        let dump = Inspector::default().dump(&snapshot(AccessibleValue::Protected));
        let expected = "\
window 1
pending messages=1 tasks=2 frames=3 animating=true next_wake=Some(500)
focus Some(NodeId(0v1))
pointer_capture None
text_input Some(NodeId(0v1))
layout
  NodeId(0v1) Field key=None parent=None children=[] rect=[0,0 200x24] clip=[0,0 100x24]
semantics
  NodeId(0v1) role=PasswordInput name=Some(\"Password\") description=None value=<protected> states=AccessibleStates { disabled: false, focusable: false, focused: false, selected: false, expanded: None, busy: false, read_only: false, required: false, invalid: false, modal: false, multiselectable: false } bounds=[0,0 200x24] actions=[Focus] relations=[] children=[]
display
  paint 7 parent=None revision=2 bounds=[0,0 200x24] hit=[0,0 200x24] clip=None transform=[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
    PopClip
";
        assert_eq!(Inspector::first_difference(expected, &dump), None);
    }

    #[test]
    fn a_text_value_is_shown_and_a_protected_one_is_not() {
        let shown = Inspector::default().dump(&snapshot(AccessibleValue::Text {
            value: "visible".into(),
            editing: None,
        }));
        assert!(shown.contains("value=Text { value: \"visible\""), "{shown}");
        let hidden = Inspector::default().dump(&snapshot(AccessibleValue::Protected));
        assert!(hidden.contains("value=<protected>"), "{hidden}");
    }

    #[test]
    fn an_items_place_and_a_headers_sort_are_shown_only_when_set() {
        let mut frame = snapshot(AccessibleValue::Protected);
        let node = &mut frame.accessibility[0].1;
        node.item_count = Some(6);
        node.item_index = Some(0);
        node.level = Some(2);
        node.column_index = Some(1);
        node.sort = Some(SortOrder::Descending);
        let dump = Inspector::default().dump(&frame);
        assert!(
            dump.contains(
                "value=<protected> items=6 index=0 level=2 column=1 sort=Descending states="
            ),
            "{dump}"
        );
    }

    #[test]
    fn the_first_differing_line_is_named() {
        assert_eq!(Inspector::first_difference("a\nb\nc\n", "a\nb\nc\n"), None);
        assert_eq!(
            Inspector::first_difference("a\nb\nc\n", "a\nx\nc\n").unwrap(),
            "line 2:\n  expected: b\n  actual:   x"
        );
        assert_eq!(
            Inspector::first_difference("a\n", "a\nb\n").unwrap(),
            "line 2:\n  expected: <end>\n  actual:   b"
        );
    }
}
