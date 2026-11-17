//! The geometry of each control family at each control size, with the normal and the compact metrics: the height the
//! control lays out at, a fixed width where the family has one, the corner shape, the label's type size and the room
//! beside the label. All values are in points.
//!
//! The height is the control's layout and alignment height. What the control paints may reach past it (a focus ring,
//! a shadow, the lens of a held thumb) and where it takes the pointer may be larger (the minimum target), but neither
//! changes the layout, so a press never moves the controls around it.

use crate::metrics::ControlSize;

/// A family of controls that share their size rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ControlFamily {
    /// An ordinary, default or destructive action button.
    PushButton,
    /// An action button drawn on its own translucent glass.
    GlassButton,
    Switch,
    /// A check box with its label.
    CheckBox,
    /// A radio button with its label.
    Radio,
    /// A pop-up or pull-down menu button.
    PopUp,
    /// An editable field with a disclosure button that lists choices.
    ComboBox,
    TextField,
    SecureField,
    SearchField,
    Slider,
    Segmented,
    Stepper,
    /// A date written out in a field, with a stepper beside it.
    DatePicker,
    /// A bordered square button with a chevron that shows or hides more.
    Disclosure,
}

impl ControlFamily {
    pub const ALL: [ControlFamily; 15] = [
        ControlFamily::PushButton,
        ControlFamily::GlassButton,
        ControlFamily::Switch,
        ControlFamily::CheckBox,
        ControlFamily::Radio,
        ControlFamily::PopUp,
        ControlFamily::ComboBox,
        ControlFamily::TextField,
        ControlFamily::SecureField,
        ControlFamily::SearchField,
        ControlFamily::Slider,
        ControlFamily::Segmented,
        ControlFamily::Stepper,
        ControlFamily::DatePicker,
        ControlFamily::Disclosure,
    ];
}

/// The corners of a control's chrome.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Corners {
    /// A rounded rectangle with corners of this radius.
    Radius(f64),
    /// Round ends: the radius is half the height.
    Capsule,
}

impl Corners {
    /// The radius for a chrome `height` points high.
    pub fn radius(self, height: f64) -> f64 {
        match self {
            Corners::Radius(r) => r.min(height / 2.0),
            Corners::Capsule => height / 2.0,
        }
    }
}

/// One family's geometry at one size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlGeometry {
    /// The layout and alignment height.
    pub height: f64,
    /// The width of a family whose width does not follow its content (a switch, a stepper, a date picker's field).
    pub width: Option<f64>,
    pub corners: Corners,
    /// The label's type size.
    pub label_size: f64,
    /// The room between the chrome's side and its label, on each side.
    pub inset: f64,
}

/// Values per size, mini to extra large.
type BySize = [f64; 5];

fn at(values: BySize, size: ControlSize) -> f64 {
    let i = ControlSize::ASCENDING
        .iter()
        .position(|s| *s == size)
        .unwrap_or(2);
    values[i]
}

/// Bezel heights: action buttons, menu buttons, combo boxes, search fields, segmented controls and steppers.
const BEZEL: BySize = [16.0, 20.0, 24.0, 28.0, 36.0];
/// The same with the compact metrics: the three smaller sizes are tighter, large and extra large are unchanged.
const BEZEL_COMPACT: BySize = [13.0, 16.0, 20.0, 28.0, 36.0];
/// A glass button's mini size is already compact.
const GLASS: BySize = [13.0, 20.0, 24.0, 28.0, 36.0];
const GLASS_COMPACT: BySize = [13.0, 18.0, 24.0, 28.0, 36.0];
/// A check box or radio button with its label.
const CHOICE: BySize = [12.0, 14.0, 16.0, 18.0, 18.0];
const CHOICE_COMPACT: BySize = [11.0, 14.0, 16.0, 18.0, 18.0];
/// A slider: the height of its knob.
const SLIDER: BySize = [12.0, 14.0, 16.0, 20.0, 20.0];
/// A plain or secure text field is one height at every size.
const FIELD: f64 = 24.0;
/// A switch is one size at every control size: 54 × 24 pt, or 38 × 20 pt with the compact metrics.
const SWITCH: (f64, f64) = (54.0, 24.0);
const SWITCH_COMPACT: (f64, f64) = (38.0, 20.0);
/// A stepper's width.
const STEPPER_WIDTH: BySize = [13.0, 17.0, 20.0, 23.0, 30.0];
const STEPPER_WIDTH_COMPACT: BySize = [9.0, 11.0, 13.0, 23.0, 30.0];
/// A date picker's field and its stepper, side by side.
const DATE_HEIGHT: BySize = [21.0, 21.0, 24.0, 28.0, 36.0];
const DATE_WIDTH: BySize = [155.0, 159.0, 162.0, 165.0, 172.0];
/// A disclosure button's side, the same with the compact metrics.
const DISCLOSURE: BySize = [13.0, 16.0, 20.0, 24.0, 24.0];
/// The label's type size.
const LABEL: BySize = [9.0, 11.0, 13.0, 13.0, 15.0];
/// The room beside an action button's label.
const BUTTON_INSET: BySize = [9.5, 11.5, 13.5, 15.5, 17.0];
const BUTTON_INSET_COMPACT: BySize = [6.5, 7.5, 9.5, 15.5, 17.0];
/// The corner radius of a rounded-rectangle bezel; the large sizes are capsules.
const BEZEL_RADIUS: BySize = [4.0, 5.0, 6.0, 0.0, 0.0];
/// The room beside a text field's text.
const FIELD_INSET: f64 = 6.0;
const FIELD_RADIUS: f64 = 6.0;

fn bezel_corners(size: ControlSize) -> Corners {
    match size {
        ControlSize::Large | ControlSize::ExtraLarge => Corners::Capsule,
        _ => Corners::Radius(at(BEZEL_RADIUS, size)),
    }
}

/// The geometry of `family` at `size`, with the compact metrics when `compact`.
pub fn control_geometry(
    family: ControlFamily,
    size: ControlSize,
    compact: bool,
) -> ControlGeometry {
    use ControlFamily::*;
    let pick = |normal: BySize, tight: BySize| at(if compact { tight } else { normal }, size);
    let label_size = at(LABEL, size);
    let bezel = ControlGeometry {
        height: pick(BEZEL, BEZEL_COMPACT),
        width: None,
        corners: bezel_corners(size),
        label_size,
        inset: pick(BUTTON_INSET, BUTTON_INSET_COMPACT),
    };
    match family {
        PushButton | PopUp | Segmented => bezel,
        GlassButton => ControlGeometry {
            height: pick(GLASS, GLASS_COMPACT),
            ..bezel
        },
        ComboBox | SearchField => ControlGeometry {
            inset: FIELD_INSET,
            corners: if family == SearchField {
                Corners::Capsule
            } else {
                bezel.corners
            },
            ..bezel
        },
        Stepper => ControlGeometry {
            width: Some(pick(STEPPER_WIDTH, STEPPER_WIDTH_COMPACT)),
            inset: 0.0,
            ..bezel
        },
        Switch => {
            let (width, height) = if compact { SWITCH_COMPACT } else { SWITCH };
            ControlGeometry {
                height,
                width: Some(width),
                corners: Corners::Capsule,
                label_size,
                inset: 0.0,
            }
        }
        CheckBox | Radio => ControlGeometry {
            height: pick(CHOICE, CHOICE_COMPACT),
            width: None,
            corners: if family == Radio {
                Corners::Capsule
            } else {
                Corners::Radius(at(BEZEL_RADIUS, size.min_regular()) - 2.0)
            },
            label_size,
            inset: 0.0,
        },
        TextField | SecureField => ControlGeometry {
            height: FIELD,
            width: None,
            corners: Corners::Radius(FIELD_RADIUS),
            label_size: at(LABEL, ControlSize::Regular),
            inset: FIELD_INSET,
        },
        Slider => ControlGeometry {
            height: at(SLIDER, size),
            width: None,
            corners: Corners::Capsule,
            label_size,
            inset: 0.0,
        },
        DatePicker => ControlGeometry {
            height: at(DATE_HEIGHT, size),
            width: Some(at(DATE_WIDTH, size)),
            corners: bezel_corners(size),
            label_size,
            inset: FIELD_INSET,
        },
        Disclosure => ControlGeometry {
            height: at(DISCLOSURE, size),
            width: Some(at(DISCLOSURE, size)),
            corners: Corners::Radius(at(BEZEL_RADIUS, size.min_regular())),
            label_size,
            inset: 0.0,
        },
    }
}

impl ControlSize {
    /// This size, or regular for a larger one.
    fn min_regular(self) -> ControlSize {
        match self {
            ControlSize::Large | ControlSize::ExtraLarge => ControlSize::Regular,
            other => other,
        }
    }
}

/// The parts of a switch at one size: the track, the thumb and the room between them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwitchGeometry {
    pub track: (f64, f64),
    /// A capsule wider than it is tall.
    pub thumb: (f64, f64),
    pub inset: f64,
}

/// The switch's parts with the normal or the compact metrics.
pub fn switch_geometry(compact: bool) -> SwitchGeometry {
    if compact {
        SwitchGeometry {
            track: SWITCH_COMPACT,
            thumb: (22.0, 16.0),
            inset: 2.0,
        }
    } else {
        SwitchGeometry {
            track: SWITCH,
            thumb: (32.0, 20.0),
            inset: 2.0,
        }
    }
}

/// The parts of a slider at one size: the track's thickness and the knob, a capsule wider than it is tall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderGeometry {
    pub track: f64,
    pub knob: (f64, f64),
}

/// The track's thickness at each size.
const SLIDER_TRACK: BySize = [4.0, 5.0, 6.0, 7.0, 7.0];
/// The knob's width over its height.
const KNOB_ASPECT: f64 = 1.25;

/// The slider's parts at `size`.
pub fn slider_geometry(size: ControlSize) -> SliderGeometry {
    let height = at(SLIDER, size);
    SliderGeometry {
        track: at(SLIDER_TRACK, size),
        knob: (height * KNOB_ASPECT, height),
    }
}

/// The side of a check box's box or a radio button's circle at `size`: the family's whole height, the label's line
/// centred beside it.
pub fn choice_mark(size: ControlSize, compact: bool) -> f64 {
    control_geometry(ControlFamily::CheckBox, size, compact).height
}
