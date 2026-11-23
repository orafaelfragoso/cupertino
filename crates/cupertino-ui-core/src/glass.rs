//! What a node asks the compositor to draw under it as glass ([`PaintCx::glass`](crate::PaintCx::glass)): a legacy
//! style or a system variant, its corners, the container it joins, its tint and whether it responds to a press.

use crate::platform::{Color, GlassModifiers, GlassRegion, GlassSize, GlassStyle, GlassVariant};

/// The corners of a glass node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GlassCorners {
    /// Corners of this radius in points.
    Radius(f64),
    /// Round ends: the radius is half the node's shorter side, whatever its size.
    Capsule,
}

/// What [`PaintCx::glass`](crate::PaintCx::glass) puts under a node: the style or the system variant, the corners, the
/// container it joins, its opacity, its tint and whether it responds to a press.
///
/// ```
/// use cupertino_ui_core::{Glass, GlassCorners, GlassStyle, GlassVariant};
///
/// // two capsule buttons of a toolbar that merge where they are less than 8 points apart
/// let button = Glass::regular().capsule().container(1, 8.0);
/// assert_eq!(button.corners, GlassCorners::Capsule);
/// // a clear panel on its own, fading in
/// let panel = Glass::clear().radius(16.0).alpha(0.5);
/// assert_eq!((panel.style, panel.container), (GlassStyle::Clear, 0));
/// // a button of the system's regular glass that grows a little under a press
/// let pressable = Glass::regular().variant(GlassVariant::Regular).capsule().interactive(true);
/// assert!(pressable.interactive);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glass {
    /// The legacy style; what a window system without variants draws when a variant is named.
    pub style: GlassStyle,
    pub corners: GlassCorners,
    /// Nodes of a window with the same nonzero container join into one shape; 0 keeps the node on its own.
    pub container: u32,
    /// How far the outlines of a container blend into each other, in points; 0 joins them with a hard edge.
    pub spacing: f64,
    /// From 0 (hidden) to 1.
    pub alpha: f64,
    /// The system variant drawn in place of the style; `None` draws the style.
    pub variant: Option<GlassVariant>,
    /// The size a variant's size-dependent values resolve at; [`GlassSize::Dynamic`], the node's own.
    pub size: GlassSize,
    /// The glass grows a little about its centre while the pointer presses it and lets go on the release, on the
    /// compositor's clock; not under Reduce Motion.
    pub interactive: bool,
    /// A colour laid over the glass's face at its alpha; `None` lays none.
    pub tint: Option<Color>,
    /// The variant steps back behind what is in front of it ([`GlassModifiers::subdued`]).
    pub subdued: bool,
    /// The variant draws no drop shadow of its own: the surface draws its shadow apart
    /// ([`GlassModifiers::exclude_shadow`]).
    pub exclude_shadow: bool,
    /// The glass is a document-modal sheet: the glass window variant casts a sheet's shallower drop shadow in place of
    /// an alert's; no other variant changes.
    pub document_modal: bool,
}

impl Glass {
    /// Regular glass with square corners, on its own, opaque.
    pub const fn regular() -> Self {
        Self {
            style: GlassStyle::Regular,
            corners: GlassCorners::Radius(0.0),
            container: 0,
            spacing: 0.0,
            alpha: 1.0,
            variant: None,
            size: GlassSize::Dynamic,
            interactive: false,
            tint: None,
            subdued: false,
            exclude_shadow: false,
            document_modal: false,
        }
    }

    /// This glass drawn as the system's `variant` in place of its style; the style
    /// stays as what a window system without variants draws.
    pub const fn variant(self, variant: GlassVariant) -> Self {
        Self {
            variant: Some(variant),
            ..self
        }
    }

    /// The variant's size-dependent values resolved at `size` rather than the node's own.
    pub const fn size_class(self, size: GlassSize) -> Self {
        Self { size, ..self }
    }

    /// The straight sRGB `color` laid over the glass's face at its alpha.
    pub const fn tint(self, color: Color) -> Self {
        Self {
            tint: Some(color),
            ..self
        }
    }

    /// Whether the glass grows a little about its centre under a press, on the compositor's clock.
    pub const fn interactive(self, interactive: bool) -> Self {
        Self {
            interactive,
            ..self
        }
    }

    /// Whether the variant steps back behind what is in front of it (a card behind another).
    pub const fn subdued(self, subdued: bool) -> Self {
        Self { subdued, ..self }
    }

    /// Whether the variant draws no drop shadow of its own, as the surface draws its shadow apart.
    pub const fn exclude_shadow(self, exclude_shadow: bool) -> Self {
        Self {
            exclude_shadow,
            ..self
        }
    }

    /// Whether the glass is a document-modal sheet: the glass window variant then casts a sheet's drop shadow, shallower
    /// than an alert's.
    pub const fn document_modal(self, document_modal: bool) -> Self {
        Self {
            document_modal,
            ..self
        }
    }

    /// The glass as a region of the window's material, in `container` blending over `spacing` points: named by its
    /// variant when it has one, or by its style's own variant when it is interactive, so the compositor can grow it.
    pub(crate) fn region(&self, spacing: f64) -> GlassRegion {
        let variant = match self.variant {
            Some(v) => Some((v, GlassModifiers::DEFAULT)),
            None if self.interactive => Some(self.style.variant()),
            None => None,
        };
        let finite = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        GlassRegion {
            variant: variant.map(|(v, m)| {
                let size = if self.variant.is_some() {
                    self.size
                } else {
                    m.size
                };
                (
                    v,
                    GlassModifiers {
                        size,
                        interactive: self.interactive,
                        subdued: m.subdued || self.subdued,
                        exclude_shadow: m.exclude_shadow || self.exclude_shadow,
                        ..m
                    },
                )
            }),
            tint: self.tint.map_or([0.0; 4], |c| {
                [finite(c.r), finite(c.g), finite(c.b), finite(c.a)]
            }),
            document_modal: self.variant.is_some() && self.document_modal,
            ..GlassRegion::of_style(self.style, self.container, spacing)
        }
    }

    /// The glass as the background of a whole window ([`crate::WindowBackground::Surface`]): on its own, as the
    /// window's shape and alpha stand for its corners and its alpha.
    pub(crate) fn window_region(&self) -> GlassRegion {
        Self {
            container: 0,
            spacing: 0.0,
            ..*self
        }
        .region(0.0)
    }

    /// Clear glass with square corners, on its own, opaque.
    pub const fn clear() -> Self {
        Self {
            style: GlassStyle::Clear,
            ..Self::regular()
        }
    }

    /// The glass of a large floating panel with square corners, on its own, opaque: regular glass toned further
    /// towards the middle, with a soft drop shadow the compositor draws below it.
    pub const fn large_panel() -> Self {
        Self {
            style: GlassStyle::LargePanel,
            ..Self::regular()
        }
    }

    /// The glass of a dense floating surface with square corners, on its own, opaque: regular glass with a wider blur
    /// over a washed-out face.
    pub const fn dense() -> Self {
        Self {
            style: GlassStyle::Dense,
            ..Self::regular()
        }
    }

    /// The glass of the tiles of a panel of modules with square corners, on its own, opaque: the clear face under a
    /// wide blur and a strong bending edge.
    pub const fn module() -> Self {
        Self {
            style: GlassStyle::Module,
            ..Self::regular()
        }
    }

    /// The glass of a notification card with square corners, on its own, opaque: the clear face over a wide blur and a
    /// blur fill, with a faint ring shadow inside its outline.
    pub const fn card() -> Self {
        Self {
            style: GlassStyle::Card,
            ..Self::regular()
        }
    }

    /// The glass of a level's track with square corners, on its own, opaque: no blur, a bending edge that follows the
    /// track's height and a ring shadow inside its outline.
    pub const fn track() -> Self {
        Self {
            style: GlassStyle::Track,
            ..Self::regular()
        }
    }

    /// The glass of a small popover over a floating bar with square corners, on its own, opaque: a wide soft blur fill,
    /// a strong bending edge and a thin line of light along its top and bottom edges.
    pub const fn popover() -> Self {
        Self {
            style: GlassStyle::Popover,
            ..Self::regular()
        }
    }

    /// The glass of a wide floating panel of large icons (the application switcher) with square corners, on its own,
    /// opaque: a blur of about a point, a strong bending edge and no shadow.
    pub const fn switcher() -> Self {
        Self {
            style: GlassStyle::Switcher,
            ..Self::regular()
        }
    }

    /// The glass of a window's sidebar with square corners, on its own, opaque: a wide soft blur fill over a light pane
    /// in the light appearance and a dark, nearly grey one in the dark.
    pub const fn sidebar() -> Self {
        Self {
            style: GlassStyle::Sidebar,
            ..Self::regular()
        }
    }

    /// The module glass of a tile that steps back, with square corners, on its own, opaque: twice the module's blur and
    /// a quieter face.
    pub const fn subdued_module() -> Self {
        Self {
            style: GlassStyle::SubduedModule,
            ..Self::regular()
        }
    }

    /// The glass of a small card of content over the desktop with square corners, on its own, opaque: a faint blur under
    /// a wide soft blur fill and a veiled face.
    pub const fn snippet() -> Self {
        Self {
            style: GlassStyle::Snippet,
            ..Self::regular()
        }
    }

    /// Clear glass veiled black from a fifth at its top to fully at its bottom, with square corners, on its own, opaque.
    pub const fn veiled() -> Self {
        Self {
            style: GlassStyle::Veiled,
            ..Self::regular()
        }
    }

    /// The glass of a window's toolbar controls with square corners, on its own, opaque: regular glass with a face
    /// lifted a little further and a thin line of light along its top and bottom edges.
    pub const fn toolbar() -> Self {
        Self {
            style: GlassStyle::Toolbar,
            ..Self::regular()
        }
    }

    /// The glass of a menu with square corners, on its own, opaque: a wide blur under a face that tones the backdrop
    /// into a narrow band, with a thin line of light along its top and bottom edges.
    pub const fn menu() -> Self {
        Self {
            style: GlassStyle::Menu,
            ..Self::regular()
        }
    }

    /// Corners of `radius` points.
    pub const fn radius(self, radius: f64) -> Self {
        Self {
            corners: GlassCorners::Radius(radius),
            ..self
        }
    }

    /// Round ends.
    pub const fn capsule(self) -> Self {
        Self {
            corners: GlassCorners::Capsule,
            ..self
        }
    }

    /// Join container `id` (nonzero), blending into its other shapes over `spacing` points.
    pub const fn container(self, id: u32, spacing: f64) -> Self {
        Self {
            container: id,
            spacing,
            ..self
        }
    }

    /// Draw at `alpha`, from 0 (hidden) to 1.
    pub const fn alpha(self, alpha: f64) -> Self {
        Self { alpha, ..self }
    }
}
