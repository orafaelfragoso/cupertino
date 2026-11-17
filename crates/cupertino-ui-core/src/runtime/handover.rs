//! A popup's transitions handed to the compositor ([`Capabilities::present_popups`]): a popover's growth out of its
//! anchor and its shrinking back ([`super::morph`]) and a closing menu's fade ([`super::fade`]) become changes of the
//! popup's presentation, on the same curves, so the compositor runs them on its clock and the popup draws no frame for
//! them. The presentation scales about the popup's centre; growing out of another point `p` moves it by
//! `(p - c) (1 - s)` along the same curve, which keeps `p` where it is.
//!
//! [`Capabilities::present_popups`]: crate::platform::Capabilities::present_popups

use crate::platform::{PresentChange, PresentProperty};
use cupertino_animation::{CubicBezier, LayerCurve};
use cupertino_geometry::{Point, Size};

use super::morph;

/// `1 - (1 - t)²`, the progress of a popover shrinking into its anchor as a cubic Bézier.
const SHRINK: CubicBezier = CubicBezier::new(1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0, 1.0);
/// `1 - (1 - t)³`, the progress of a closing menu's fade as a cubic Bézier.
const MENU_FADE: CubicBezier = CubicBezier::new(1.0 / 3.0, 1.0, 2.0 / 3.0, 1.0);

fn change(property: PresentProperty, to: f64, curve: Option<LayerCurve>) -> PresentChange {
    PresentChange {
        property,
        to,
        curve,
        velocity: 0.0,
    }
}

/// How far the presentation moves a popup of `size` scaled by `s` about its centre so that `origin` stays put.
fn offset(origin: Point, size: Size, s: f64) -> (f64, f64) {
    (
        (origin.x - size.width / 2.0) * (1.0 - s),
        (origin.y - size.height / 2.0) * (1.0 - s),
    )
}

/// A popover of `size` growing out of `origin` (its coordinates): from [`morph::OPEN_START_SCALE`] to its size on the
/// opening spring.
pub(super) fn grow(origin: Point, size: Size) -> Vec<PresentChange> {
    let spring = Some(LayerCurve::Spring {
        response: morph::OPEN_RESPONSE,
        damping: morph::OPEN_DAMPING,
    });
    let (x, y) = offset(origin, size, morph::OPEN_START_SCALE);
    vec![
        change(PresentProperty::Scale, morph::OPEN_START_SCALE, None),
        change(PresentProperty::OffsetX, x, None),
        change(PresentProperty::OffsetY, y, None),
        change(PresentProperty::Scale, 1.0, spring),
        change(PresentProperty::OffsetX, 0.0, spring),
        change(PresentProperty::OffsetY, 0.0, spring),
    ]
}

/// A popup of `size` closing: a popover shrinks into `into` and fades as it does, over [`morph::CLOSE_NS`]; a menu
/// fades over [`super::fade::MENU_FADE_NS`].
pub(super) fn close(into: Option<Point>, size: Size) -> Vec<PresentChange> {
    let timed = |ns: u64, bezier| {
        Some(LayerCurve::Timed {
            duration: ns as f64 / 1e9,
            bezier,
        })
    };
    let Some(into) = into else {
        return vec![change(
            PresentProperty::Opacity,
            0.0,
            timed(super::fade::MENU_FADE_NS, MENU_FADE),
        )];
    };
    let curve = timed(morph::CLOSE_NS, SHRINK);
    let (x, y) = offset(into, size, 0.0);
    vec![
        change(PresentProperty::Scale, 0.0, curve),
        change(PresentProperty::Opacity, 0.0, curve),
        change(PresentProperty::OffsetX, x, curve),
        change(PresentProperty::OffsetY, y, curve),
    ]
}
