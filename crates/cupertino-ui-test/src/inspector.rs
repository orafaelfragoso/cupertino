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
