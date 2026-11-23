use super::*;

impl GlassParams {
    /// The recipe of a shape in `mode` whose shorter side is `short_side` points.
    pub fn new(mode: GlassMode, short_side: f32) -> Self {
        let s = if short_side.is_finite() {
            short_side.max(0.0)
        } else {
            0.0
        };
        let dark = mode.dark;
        // module tiles look the same in both appearances, with every option
        let face_dark = dark || mode.style == GlassStyle::Module;
        let mut p = match mode.style {
            GlassStyle::Regular => Self::regular(dark, s),
            GlassStyle::Clear => Self::clear(dark, s),
            GlassStyle::LargePanel => Self::large_panel(dark, s),
            GlassStyle::Bar => Self::bar(dark, s),
            GlassStyle::Dense => Self::dense(dark, s),
            GlassStyle::Module => Self::module(dark, s),
            GlassStyle::Card => Self::card(dark, s),
            GlassStyle::Track => Self::track(dark, s),
            GlassStyle::Popover => Self::popover(dark),
            GlassStyle::Switcher => Self::switcher(dark, s),
            GlassStyle::Sidebar => Self::sidebar(dark, s),
            GlassStyle::SubduedModule => Self::subdued_module(dark, s),
            GlassStyle::Snippet => Self::snippet(dark, s),
            GlassStyle::Veiled => Self::veiled(dark, s),
            GlassStyle::Toolbar => Self::toolbar(dark, s),
            GlassStyle::Menu => Self::menu(dark),
        };
        if mode.increase_contrast {
            if let Some(face) = increased_contrast_face(face_dark, mode.style) {
                p.face = face;
            }
        }
        if mode.reduce_motion {
            p.refraction.height = 0.0;
            p.blur.radius = p.blur.radius.max(REDUCED_MOTION_BLUR);
            p.blur.level_opacity = REDUCED_MOTION_LEVELS;
        }
        if mode.reduce_transparency {
            p.refraction.amount = 0.0;
            p.refraction.height = 0.0;
            p.shadow.ring_blur = 0.0;
            p.shadow.ring_stroke = 0.0;
            p.shadow.ring_offset = 0.0;
            match mode.style {
                GlassStyle::Regular
                | GlassStyle::LargePanel
                | GlassStyle::Bar
                | GlassStyle::Dense
                | GlassStyle::Module
                | GlassStyle::Card
                | GlassStyle::Track
                | GlassStyle::Popover
                | GlassStyle::Switcher
                | GlassStyle::Sidebar
                | GlassStyle::SubduedModule
                | GlassStyle::Snippet
                | GlassStyle::Veiled
                | GlassStyle::Toolbar
                | GlassStyle::Menu => {
                    // the large panel's wider blur gives way to regular glass's whole blur recipe
                    p.blur = Self::regular(dark, s).blur;
                    p.blur.radius = REDUCED_TRANSPARENCY_BLUR;
                    p.blur.level_opacity = REDUCED_TRANSPARENCY_LEVELS;
                }
                GlassStyle::Clear => p.blur.radius = REDUCED_TRANSPARENCY_BLUR_CLEAR,
            }
            // both options: these optics with the contrast colours (clear glass has none of its own)
            let contrast = mode
                .increase_contrast
                .then(|| increased_contrast_face(face_dark, mode.style))
                .flatten();
            p.face = contrast.unwrap_or_else(|| reduced_transparency_face(face_dark, mode.style));
            if contrast.is_none() && dark && mode.style == GlassStyle::LargePanel {
                // a launcher-sized sheet is near black with only a faint trace of the colours behind it
                p.face.saturation = LARGE_PANEL_REDUCED_SATURATION_DARK;
            }
        } else if mode.density > 0 {
            p.face.fill = denser_fill(
                p.face.fill,
                face_dark,
                f32::from(mode.density.min(100)) / 100.0,
            );
        }
        if mode.style == GlassStyle::Menu && (mode.reduce_transparency || mode.increase_contrast) {
            // the popup's own rim, drawn apart, marks the edge of the opaque menu
            p.highlight.glow_amount = 0.0;
        }
        p
    }

    fn regular(dark: bool, s: f32) -> Self {
        let narrow = by_side(&NARROW_LEVEL_OPACITY_BY_SIDE, s);
        let bleed = BLEED_PER_POINT * s.min(BLEED_SIDE_MAX);
        let mut face = regular_face(dark);
        if dark {
            face.max_luma = by_side(&DARK_MAX_LUMA_BY_SIDE, s);
        }
        Self {
            blur: GlassBlur {
                radius: 5.0,
                level_opacity: [narrow, narrow, 1.0, 1.0],
                fill_radius: 8.0,
                fill_darken: if dark { 0.882 } else { 0.0 },
                fill_normal: 0.46,
                fill_lighten: if dark { 0.0 } else { 0.882 },
            },
            face,
            refraction: GlassRefraction {
                amount: -(REFRACTION_PER_POINT_REGULAR * s).min(REFRACTION_AMOUNT_MAX),
                height: (REFRACTION_HEIGHT_PER_POINT_REGULAR * s).min(REFRACTION_HEIGHT_MAX),
                outer_amount: 0.0,
                outer_height: 0.0,
                outer_opacity: 0.0,
            },
            bleed: GlassBleed {
                amount: bleed,
                height: bleed,
                blur: bleed.min(BLEED_BLUR_MAX),
                white: if dark { 0.5 } else { 1.0 },
                black: if dark { 0.125 } else { 0.9 },
                saturation: if dark { 1.0 } else { 1.2 },
                darken: !dark,
                opacity: 0.0,
            },
            highlight: GlassHighlight {
                amount: 0.5,
                angle: HIGHLIGHT_ANGLE,
                spread: 2.0 * std::f32::consts::FRAC_PI_3,
                height: 0.5,
                offset: -0.5,
                color_bias: -0.1875,
                curvature: 0.75,
                glow_amount: 0.0,
                glow_height: 0.0,
                glow_offset: 0.0,
                glow_fill: GLOW_FILL_REGULAR,
                diffuse: GlowDiffuse::NONE,
                matrix: HIGHLIGHT_MATRIX_REGULAR,
            },
            shadow: GlassShadow {
                offset: [0.0, 8.0],
                amount: 0.0,
                height: 0.0,
                fill: [0.0, 0.0, 0.0, 0.3],
                saturation: 1.0,
                opacity: 0.0,
                ring_blur: 5.0,
                ring_stroke: 4.0,
                ring_offset: 8.0,
                ring_opacity: 0.0,
                outline_opacity: 0.0,
                outline_width: 0.0,
                edge_line: if dark {
                    REGULAR_EDGE_LINE_DARK
                } else {
                    REGULAR_EDGE_LINE_LIGHT
                },
                edge_line_sides: 0.0,
                edge_line_even: false,
            },
            capture_scale: GLASS_BACKDROP_SCALE,
            veil: [0.0; 2],
            specular: GlassSpecular::NONE,
            border: GlassBorder::NONE,
            tint: [0.0; 4],
        }
    }

    /// The large panel's recipe: the regular one of its size with its own face (see [`large_panel_face`]), a
    /// refraction band no deeper than [`LARGE_PANEL_REFRACTION_HEIGHT_MAX`], in the light appearance a bright inner line,
    /// and the panel's drop shadow shown.
    fn large_panel(dark: bool, s: f32) -> Self {
        let mut p = Self::regular(dark, s);
        p.blur.radius *= LARGE_PANEL_BLUR_SCALE;
        p.blur.fill_radius *= LARGE_PANEL_BLUR_SCALE;
        p.face = large_panel_face(dark);
        p.refraction.height = p.refraction.height.min(LARGE_PANEL_REFRACTION_HEIGHT_MAX);
        let h = &mut p.highlight;
        if dark {
            (h.glow_amount, h.glow_height, h.glow_offset) = LARGE_PANEL_RIM_DARK;
            h.spread = LARGE_PANEL_RIM_SPREAD_DARK;
            h.curvature = LARGE_PANEL_RIM_CURVATURE_DARK;
            h.glow_fill = 1.0;
            h.diffuse = LARGE_PANEL_GLOW_DARK;
            h.matrix = TOWARDS_WHITE_MATRIX;
        } else {
            h.glow_amount = LARGE_PANEL_INNER_LINE_LIGHT;
            h.glow_height = LARGE_PANEL_INNER_LINE_WIDTH;
            h.glow_offset = LARGE_PANEL_INNER_LINE_DEPTH;
        }
        p.shadow.offset = [0.0, LARGE_PANEL_SHADOW_OFFSET];
        p.shadow.amount = 100.0;
        p.shadow.height = LARGE_PANEL_SHADOW_BLUR;
        p.shadow.fill = [
            0.0,
            0.0,
            0.0,
            if dark {
                LARGE_PANEL_SHADOW_DARK
            } else {
                LARGE_PANEL_SHADOW_LIGHT
            },
        ];
        p.shadow.opacity = 1.0;
        p
    }

    /// The dense glass: the regular recipe of its size with a blur that grows with the size at full opacity, a bending
    /// edge of its own, the dense face ([`dense_face`]) and a faint ring shadow inside its outline, captured coarsely.
    fn dense(dark: bool, s: f32) -> Self {
        let mut p = Self::regular(dark, s);
        let max_luma = p.face.max_luma;
        p.face = dense_face(dark);
        if dark {
            p.face.max_luma = max_luma;
        }
        p.blur.radius = by_side(&DENSE_BLUR_BY_SIDE, s);
        p.blur.level_opacity = [1.0; BLUR_LEVELS];
        p.refraction.amount = -(DENSE_REFRACTION_PER_POINT * s).min(REFRACTION_AMOUNT_MAX);
        p.refraction.height = (DENSE_REFRACTION_HEIGHT_PER_POINT * s).min(REFRACTION_HEIGHT_MAX);
        p.shadow.ring_opacity = DENSE_RING_OPACITY;
        p.shadow.edge_line = if dark {
            DENSE_EDGE_LINE_DARK
        } else {
            DENSE_EDGE_LINE_LIGHT
        };
        let widest = DENSE_BLUR_BY_SIDE[DENSE_BLUR_BY_SIDE.len() - 1].1;
        p.capture_scale = if p.blur.radius >= widest {
            DENSE_CAPTURE_SCALE_WIDE
        } else {
            DENSE_CAPTURE_SCALE
        };
        p
    }

    /// The module glass, the same in both appearances: the dark clear glass of its size with the module's face,
    /// refraction and rim, a line of light inside its top and bottom edges and a dark line outside its sides.
    fn module(_dark: bool, s: f32) -> Self {
        let mut p = Self::clear(true, s);
        p.blur.radius = by_side(&MODULE_BLUR_BY_SIDE, s);
        p.face = GlassFace {
            saturation: MODULE_SATURATION,
            white: MODULE_WHITE,
            black: MODULE_BLACK,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [0.0; 4],
        };
        p.refraction.amount = MODULE_REFRACTION;
        p.refraction.height =
            (MODULE_REFRACTION_HEIGHT_PER_POINT * s).min(MODULE_REFRACTION_HEIGHT_MAX);
        p.highlight.color_bias = MODULE_RIM_BIAS;
        let (amount, height, offset) = MODULE_LINE;
        p.highlight.glow_amount = amount;
        p.highlight.glow_height = height;
        p.highlight.glow_offset = offset;
        p.highlight.glow_fill = MODULE_LINE_BOTTOM;
        p.highlight.spread = MENU_LINE_SPREAD;
        p.highlight.curvature = MODULE_LINE_CURVATURE;
        p.highlight.diffuse = GlowDiffuse::NONE;
        p.highlight.matrix = MODULE_LINE_MATRIX;
        p.shadow.edge_line = MODULE_EDGE_LINE;
        p.shadow.edge_line_sides = MODULE_EDGE_LINE_SIDES;
        p.shadow.edge_line_even = true;
        p
    }

    /// The card glass: the clear face over a wide blur and the regular glass's blur fill, with the card's refraction,
    /// rim and ring shadow, captured at [`CARD_CAPTURE_SCALE`].
    fn card(dark: bool, s: f32) -> Self {
        let mut p = Self::clear(dark, s);
        if dark {
            p.face = GlassFace {
                saturation: CARD_SATURATION_DARK,
                white: CARD_WHITE_DARK,
                black: CARD_BLACK_DARK,
                clamp: 1.0,
                max_luma: CARD_MAX_LUMA_DARK,
                fill: [0.0; 4],
            };
        }
        p.blur = GlassBlur {
            radius: CARD_BLUR,
            level_opacity: [1.0; BLUR_LEVELS],
            fill_radius: CARD_FILL_RADIUS,
            fill_darken: if dark { CARD_FILL_BLEND } else { 0.0 },
            fill_normal: CARD_FILL_NORMAL,
            fill_lighten: if dark { 0.0 } else { CARD_FILL_BLEND },
        };
        p.refraction.amount = CARD_REFRACTION;
        p.refraction.height = CARD_REFRACTION_HEIGHT;
        p.highlight.amount = CARD_RIM;
        p.highlight.color_bias = CARD_RIM_BIAS;
        p.highlight.spread = if dark {
            CARD_SPREAD_DARK
        } else {
            CARD_SPREAD_LIGHT
        };
        p.highlight.matrix = HIGHLIGHT_MATRIX_REGULAR;
        p.shadow = GlassShadow {
            amount: 0.0,
            height: 0.0,
            fill: [0.0, 0.0, 0.0, 0.3],
            saturation: 1.0,
            ring_blur: 5.0,
            ring_stroke: 4.0,
            ring_offset: 8.0,
            ring_opacity: CARD_RING_OPACITY,
            edge_line: if dark {
                CARD_EDGE_LINE_DARK
            } else {
                CARD_EDGE_LINE_LIGHT
            },
            edge_line_sides: if dark { MODULE_EDGE_LINE_SIDES } else { 0.0 },
            edge_line_even: dark,
            ..p.shadow
        };
        if dark {
            let h = &mut p.highlight;
            (h.glow_amount, h.glow_height, h.glow_offset) = CARD_LINE_DARK;
            h.glow_fill = MODULE_LINE_BOTTOM;
            h.matrix = MODULE_LINE_MATRIX;
        }
        p.capture_scale = CARD_CAPTURE_SCALE;
        p
    }

    /// The track glass: no blur, read at [`TRACK_CAPTURE_SCALE`], a refraction that follows the track's height, and
    /// a ring shadow inside the outline.
    fn track(dark: bool, s: f32) -> Self {
        let mut p = Self::clear(dark, s);
        p.blur = GlassBlur {
            radius: 0.0,
            level_opacity: [0.0; BLUR_LEVELS],
            fill_radius: 0.0,
            fill_darken: 0.0,
            fill_normal: 0.0,
            fill_lighten: 0.0,
        };
        p.face = GlassFace {
            saturation: 1.0,
            white: 1.0,
            black: TRACK_BLACK,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [1.0, 1.0, 1.0, 0.0],
        };
        p.refraction.amount = -TRACK_REFRACTION_PER_POINT * s;
        p.refraction.height = TRACK_REFRACTION_HEIGHT_PER_POINT * s;
        p.highlight = GlassHighlight {
            amount: TRACK_RIM,
            spread: 2.0 * std::f32::consts::FRAC_PI_3,
            height: 1.0,
            offset: -1.0,
            color_bias: TRACK_RIM_BIAS,
            matrix: HIGHLIGHT_MATRIX_REGULAR,
            ..p.highlight
        };
        p.shadow = GlassShadow {
            offset: [0.0, TRACK_SHADOW_OFFSET],
            amount: TRACK_SHADOW_AMOUNT,
            height: TRACK_SHADOW_BLUR,
            fill: TRACK_SHADOW_COLOR,
            saturation: 1.0,
            opacity: TRACK_SHADOW_OPACITY,
            ring_blur: TRACK_RING_BLUR,
            ring_stroke: 4.0,
            ring_offset: 8.0,
            ring_opacity: if dark {
                TRACK_RING_DARK
            } else {
                TRACK_RING_LIGHT
            },
            outline_opacity: 0.0,
            outline_width: 0.0,
            edge_line: if dark {
                TRACK_EDGE_LINE_DARK
            } else {
                TRACK_EDGE_LINE_LIGHT
            },
            edge_line_sides: 0.0,
            edge_line_even: false,
        };
        p.capture_scale = TRACK_CAPTURE_SCALE;
        p
    }

    /// The popover glass: no blur of its own, only the wide soft blur fill, the popover's face, a strong bending edge,
    /// the rim and the thin line of light along the top and bottom edges over a wide faint glow; a faint ring shadow
    /// inside its outline. Nothing in it follows the size.
    fn popover(dark: bool) -> Self {
        let mut p = Self::regular(dark, POPOVER_SIDE);
        p.blur = GlassBlur {
            radius: POPOVER_BLUR,
            level_opacity: POPOVER_LEVELS,
            fill_radius: POPOVER_FILL_RADIUS,
            fill_darken: if dark { POPOVER_FILL_OVER } else { 0.0 },
            fill_normal: POPOVER_FILL_NORMAL,
            fill_lighten: if dark { 0.0 } else { POPOVER_FILL_OVER },
        };
        p.face = popover_face(dark);
        p.refraction = GlassRefraction {
            amount: POPOVER_REFRACTION,
            height: POPOVER_REFRACTION_HEIGHT,
            outer_amount: 0.0,
            outer_height: 0.0,
            outer_opacity: 0.0,
        };
        p.bleed.amount = POPOVER_BLEED;
        p.bleed.height = POPOVER_BLEED;
        p.highlight = GlassHighlight {
            amount: BAR_RIM,
            spread: BAR_GLOW_SPREAD,
            color_bias: BAR_RIM_BIAS,
            glow_amount: BAR_GLOW,
            glow_height: BAR_GLOW_HEIGHT,
            glow_offset: BAR_GLOW_OFFSET,
            glow_fill: BAR_GLOW_FILL,
            diffuse: BAR_GLOW_DIFFUSE,
            ..p.highlight
        };
        p.shadow.ring_opacity = POPOVER_RING_OPACITY;
        p.shadow.edge_line = if dark { 0.0 } else { POPOVER_EDGE_LINE_LIGHT };
        p
    }

    /// The switcher glass: regular glass of its size with a blur of about a point at full opacity and no blur fill, the
    /// switcher's face, a strong bending edge of a fixed depth, its rim, no ring shadow, and the dark line just outside
    /// its outline.
    fn switcher(dark: bool, s: f32) -> Self {
        let mut p = Self::regular(dark, s);
        p.blur = GlassBlur {
            radius: SWITCHER_BLUR,
            level_opacity: [1.0; BLUR_LEVELS],
            fill_radius: 0.0,
            fill_darken: 0.0,
            fill_normal: 0.0,
            fill_lighten: 0.0,
        };
        p.face = switcher_face(dark);
        p.refraction = GlassRefraction {
            amount: SWITCHER_REFRACTION,
            height: SWITCHER_REFRACTION_HEIGHT,
            outer_amount: 0.0,
            outer_height: 0.0,
            outer_opacity: 0.0,
        };
        p.highlight.amount = SWITCHER_RIM;
        p.highlight.color_bias = SWITCHER_RIM_BIAS;
        p.highlight.glow_amount = 0.0;
        p.shadow.ring_opacity = 0.0;
        if dark {
            p.shadow.edge_line = SWITCHER_EDGE_LINE_DARK;
        }
        p
    }

    /// The sidebar glass: regular glass of its size with the sidebar's blur, blur fill, face, bending edge, rim and
    /// ring shadow.
    fn sidebar(dark: bool, s: f32) -> Self {
        let mut p = Self::regular(dark, s);
        p.blur = GlassBlur {
            radius: SIDEBAR_BLUR,
            level_opacity: [1.0; BLUR_LEVELS],
            fill_radius: SIDEBAR_FILL_RADIUS,
            fill_darken: if dark { SIDEBAR_FILL_BLEND } else { 0.0 },
            fill_normal: SIDEBAR_FILL_NORMAL,
            fill_lighten: if dark { 0.0 } else { SIDEBAR_FILL_BLEND },
        };
        p.face = sidebar_face(dark);
        p.refraction = GlassRefraction {
            amount: SIDEBAR_REFRACTION,
            height: SIDEBAR_REFRACTION_HEIGHT,
            outer_amount: 0.0,
            outer_height: 0.0,
            outer_opacity: 0.0,
        };
        p.highlight.amount = SIDEBAR_RIM;
        p.highlight.color_bias = SIDEBAR_RIM_BIAS;
        p.highlight.glow_amount = 0.0;
        p.shadow.ring_opacity = SIDEBAR_RING_OPACITY;
        // no line outside the outline: the sidebar meets the window's content there, which a line would mark
        p.shadow.edge_line = 0.0;
        p
    }

    /// The subdued module glass: the module glass of its size with twice its blur and the subdued face.
    fn subdued_module(dark: bool, s: f32) -> Self {
        let mut p = Self::module(dark, s);
        p.blur.radius *= SUBDUED_MODULE_BLUR_SCALE;
        if dark {
            p.face.black = SUBDUED_MODULE_BLACK_DARK;
        } else {
            p.face.white = SUBDUED_MODULE_WHITE_LIGHT;
        }
        p
    }

    /// The snippet glass: regular glass of its size with a faint blur under the regular blur fill, the snippet's face,
    /// bending edge, rim and ring shadow.
    fn snippet(dark: bool, s: f32) -> Self {
        let mut p = Self::regular(dark, s);
        p.blur = GlassBlur {
            radius: SNIPPET_BLUR,
            level_opacity: [SNIPPET_LEVEL_OPACITY, 1.0, 1.0, 1.0],
            fill_radius: SNIPPET_FILL_RADIUS,
            fill_darken: if dark { SNIPPET_FILL_BLEND } else { 0.0 },
            fill_normal: SNIPPET_FILL_NORMAL,
            fill_lighten: if dark { 0.0 } else { SNIPPET_FILL_BLEND },
        };
        p.face = snippet_face(dark);
        p.refraction = GlassRefraction {
            amount: SNIPPET_REFRACTION,
            height: SNIPPET_REFRACTION_HEIGHT,
            outer_amount: 0.0,
            outer_height: 0.0,
            outer_opacity: 0.0,
        };
        p.highlight.amount = SNIPPET_RIM;
        p.highlight.color_bias = SNIPPET_RIM_BIAS;
        p.highlight.glow_amount = 0.0;
        p.shadow.ring_opacity = SNIPPET_RING_OPACITY;
        p.shadow.edge_line = if dark { 0.0 } else { SNIPPET_EDGE_LINE_LIGHT };
        p
    }

    /// The veiled glass: clear glass of its size under the soft blur fill, a bending edge and rim of its own, and the
    /// black veil from its top to its bottom.
    fn veiled(dark: bool, s: f32) -> Self {
        let mut p = Self::clear(dark, s);
        p.blur = GlassBlur {
            radius: VEILED_BLUR,
            level_opacity: [1.0; BLUR_LEVELS],
            fill_radius: VEILED_FILL_RADIUS,
            fill_darken: VEILED_FILL_DARKEN,
            fill_normal: VEILED_FILL_NORMAL,
            fill_lighten: if dark { 0.0 } else { VEILED_FILL_LIGHTEN },
        };
        p.refraction = GlassRefraction {
            amount: VEILED_REFRACTION,
            height: VEILED_REFRACTION_HEIGHT,
            outer_amount: 0.0,
            outer_height: 0.0,
            outer_opacity: 0.0,
        };
        p.highlight.amount = VEILED_RIM;
        p.highlight.color_bias = VEILED_RIM_BIAS;
        p.veil = VEILED_VEIL;
        p
    }

    /// The menu glass: one Gaussian of [`MENU_BLUR`] at full opacity and no blur fill, the menu's face
    /// ([`menu_face`]), no bending edge, bleed or rim, and a thin line of light along its top and bottom edges; captured
    /// coarsely, as the wide blur leaves no fine detail. Nothing in it follows the size. The outline and the drop shadow
    /// are the popup's own.
    fn menu(dark: bool) -> Self {
        let mut p = Self::regular(dark, 0.0);
        p.blur = GlassBlur {
            radius: MENU_BLUR,
            level_opacity: [1.0; BLUR_LEVELS],
            fill_radius: 0.0,
            fill_darken: 0.0,
            fill_normal: 0.0,
            fill_lighten: 0.0,
        };
        p.face = menu_face(dark);
        p.refraction = GlassRefraction {
            amount: 0.0,
            height: 0.0,
            outer_amount: 0.0,
            outer_height: 0.0,
            outer_opacity: 0.0,
        };
        p.bleed.amount = 0.0;
        p.bleed.height = 0.0;
        let (amount, height, offset) = if dark {
            MENU_LINE_DARK
        } else {
            MENU_LINE_LIGHT
        };
        p.highlight = GlassHighlight {
            amount: 0.0,
            spread: MENU_LINE_SPREAD,
            curvature: 1.0,
            glow_amount: amount,
            glow_height: height,
            glow_offset: offset,
            glow_fill: 1.0,
            diffuse: GlowDiffuse::NONE,
            matrix: MENU_LINE_MATRIX,
            ..p.highlight
        };
        p.shadow.ring_opacity = 0.0;
        p.shadow.edge_line = 0.0;
        p.capture_scale = MENU_CAPTURE_SCALE;
        p
    }

    /// The bar glass: clear glass with the bar's narrow blur, face, thin refraction, rim and a thin line of light along
    /// its top and bottom edges, equally bright, over a wide faint glow.
    fn bar(dark: bool, s: f32) -> Self {
        let mut p = Self::clear(dark, s);
        p.blur.radius = BAR_BLUR;
        p.face = bar_face(dark);
        p.refraction.amount = BAR_REFRACTION;
        p.refraction.height = BAR_REFRACTION_HEIGHT;
        p.highlight = GlassHighlight {
            amount: BAR_RIM,
            angle: HIGHLIGHT_ANGLE,
            spread: BAR_GLOW_SPREAD,
            color_bias: BAR_RIM_BIAS,
            glow_amount: BAR_GLOW,
            glow_height: BAR_GLOW_HEIGHT,
            glow_offset: BAR_GLOW_OFFSET,
            glow_fill: BAR_GLOW_FILL,
            diffuse: BAR_GLOW_DIFFUSE,
            matrix: if dark {
                BAR_GLOW_MATRIX_DARK
            } else {
                BAR_GLOW_MATRIX_LIGHT
            },
            ..p.highlight
        };
        p.shadow.amount = BAR_SHADOW_AMOUNT;
        p.shadow.height = BAR_SHADOW_HEIGHT;
        p
    }

    /// The toolbar glass: the regular recipe of its size with a face lifted further ([`TOOLBAR_BLACK_DARK`],
    /// [`TOOLBAR_WHITE_LIGHT`]), a thin line of light along its top edge and a fainter one along its bottom over a soft
    /// glow, and neither the rim nor the edge line, which the toolkit draws along the control's outline.
    fn toolbar(dark: bool, s: f32) -> Self {
        let mut p = Self::regular(dark, s);
        if dark {
            p.face.black = TOOLBAR_BLACK_DARK;
        } else {
            p.face.white = TOOLBAR_WHITE_LIGHT;
        }
        p.highlight = GlassHighlight {
            amount: 0.0,
            spread: TOOLBAR_GLOW_SPREAD,
            curvature: 1.0,
            glow_amount: if dark {
                TOOLBAR_GLOW_DARK
            } else {
                TOOLBAR_GLOW_LIGHT
            },
            glow_height: TOOLBAR_GLOW_HEIGHT,
            glow_offset: TOOLBAR_GLOW_OFFSET,
            glow_fill: TOOLBAR_GLOW_FILL,
            diffuse: TOOLBAR_GLOW_DIFFUSE,
            ..p.highlight
        };
        p.shadow.edge_line = 0.0;
        p
    }

    fn clear(dark: bool, s: f32) -> Self {
        Self {
            blur: GlassBlur {
                radius: (CLEAR_BLUR_MIN + (s - CLEAR_BLUR_SIDE_MIN) * CLEAR_BLUR_PER_POINT)
                    .clamp(CLEAR_BLUR_MIN, CLEAR_BLUR_MAX),
                level_opacity: [1.0; BLUR_LEVELS],
                fill_radius: 0.0,
                fill_darken: 0.0,
                fill_normal: 0.0,
                fill_lighten: 0.0,
            },
            face: clear_face(dark),
            refraction: GlassRefraction {
                amount: -(REFRACTION_PER_POINT_CLEAR * s).min(REFRACTION_AMOUNT_MAX),
                height: (REFRACTION_HEIGHT_PER_POINT_CLEAR * s).min(REFRACTION_HEIGHT_MAX),
                outer_amount: 0.0,
                outer_height: 0.0,
                outer_opacity: 0.0,
            },
            bleed: GlassBleed {
                amount: 0.0,
                height: 0.0,
                blur: 0.0,
                white: 1.0,
                black: 0.75,
                saturation: 1.2,
                darken: true,
                opacity: 0.0,
            },
            highlight: GlassHighlight {
                amount: 0.4,
                angle: HIGHLIGHT_ANGLE,
                spread: 5.0 * std::f32::consts::PI / 12.0,
                height: 0.5,
                offset: -0.5,
                color_bias: -0.25,
                curvature: 0.75,
                glow_amount: 0.0,
                glow_height: 0.0,
                glow_offset: 0.0,
                glow_fill: GLOW_FILL,
                diffuse: GlowDiffuse::NONE,
                matrix: HIGHLIGHT_MATRIX_CLEAR,
            },
            shadow: GlassShadow {
                offset: [0.0, 8.0],
                amount: CLEAR_SHADOW_AMOUNT_PER_POINT * s.min(CLEAR_SHADOW_AMOUNT_SIDE_MAX),
                height: CLEAR_SHADOW_HEIGHT_PER_POINT * s.min(CLEAR_SHADOW_HEIGHT_SIDE_MAX),
                fill: [0.0, 0.0, 0.0, 0.1],
                saturation: 1.2,
                opacity: 0.0,
                ring_blur: 0.0,
                ring_stroke: 0.0,
                ring_offset: 0.0,
                ring_opacity: 0.0,
                outline_opacity: 0.0,
                outline_width: 0.0,
                edge_line: 0.0,
                edge_line_sides: 0.0,
                edge_line_even: false,
            },
            capture_scale: GLASS_BACKDROP_SCALE,
            veil: [0.0; 2],
            specular: GlassSpecular::NONE,
            border: GlassBorder::NONE,
            tint: [0.0; 4],
        }
    }

    /// The Gaussian deviation of the backdrop's blur, in points: the radius scaled by the first level's opacity, twice
    /// over, at most [`BLUR_SIGMA_MAX`].
    pub fn blur_sigma(&self) -> f32 {
        (BLUR_SIGMA_PER_RADIUS * self.blur.radius * self.blur.level_opacity[0].clamp(0.0, 1.0))
            .clamp(0.0, BLUR_SIGMA_MAX)
    }

    /// Whether the blur fill shows.
    pub fn fill_shows(&self) -> bool {
        self.blur.fill_darken + self.blur.fill_normal + self.blur.fill_lighten > 0.0
    }

    /// The Gaussian deviation of the blur fill, in points, at most [`BLUR_SIGMA_MAX`].
    pub fn fill_sigma(&self) -> f32 {
        (FILL_SIGMA_PER_RADIUS * self.blur.fill_radius).clamp(0.0, BLUR_SIGMA_MAX)
    }

    /// How far past the shape the backdrop is read, in points: three deviations of the widest blur it reads (the
    /// backdrop's, or the blur fill's when it shows), or the reach of a shown drop shadow.
    pub fn reach(&self) -> f32 {
        let fill = if self.fill_shows() {
            self.fill_sigma()
        } else {
            0.0
        };
        // a Gaussian is read three deviations out; the refraction reads the face further inside the shape, never past
        // its edge
        let read = 3.0 * self.blur_sigma().max(fill);
        let line = if self.shadow.edge_line > 0.0 {
            EDGE_LINE_REACH
        } else {
            0.0
        };
        read.max(self.shadow_reach()).max(line)
    }

    /// How far past the shape a shown drop shadow reaches, in points: its offset and half its smooth edge of three
    /// blur radii; 0 when it does not show.
    pub fn shadow_reach(&self) -> f32 {
        let s = &self.shadow;
        if s.opacity * s.amount * s.fill[3] <= 0.0 {
            return 0.0;
        }
        s.offset[0].abs().max(s.offset[1].abs()) + 1.5 * s.height
    }
}

/// How far past the shape the edge line ([`GlassShadow::edge_line`]) needs the glass drawn, in points: its pixel and the
/// half pixel its band fades over, at a scale of 1 or more.
const EDGE_LINE_REACH: f32 = 2.0;

/// The refraction's offset at the edge, as a share of its strength.
pub const REFRACTION_EDGE_SHARE: f32 = 0.9;
/// How fast the refraction's offset decays across its band: `e^(−REFRACTION_DECAY · t)`.
pub const REFRACTION_DECAY: f32 = 4.2;

/// The refraction's offset `t` of its band deep (0 at the edge, 1 at the band's inner side), as a share of its strength:
/// `REFRACTION_EDGE_SHARE · e^(−REFRACTION_DECAY · t) · (1 − t⁴)`, nothing from the band's inner side on. A pixel reads
/// the face `strength · refraction_profile(t)` points along the edge's outward normal: a negative strength reads it
/// further inside, so the edge magnifies what lies just inside it.
pub fn refraction_profile(t: f32) -> f32 {
    if !(0.0..1.0).contains(&t) {
        return if t < 0.0 { REFRACTION_EDGE_SHARE } else { 0.0 };
    }
    let t2 = t * t;
    REFRACTION_EDGE_SHARE * (-REFRACTION_DECAY * t).exp() * (1.0 - t2 * t2)
}

/// The foreground colour matrix of text and symbols on glass of `dark` appearance. It is applied to the face colour
/// under the glyph, and the glyph's coverage and alpha mix the result over that face.
pub fn foreground_matrix(dark: bool) -> ColorMatrix {
    if dark {
        FOREGROUND_MATRIX_DARK
    } else {
        FOREGROUND_MATRIX_LIGHT
    }
}

/// Whether a glass shape whose short side is `short_side` points picks its foreground from what is behind it.
pub fn adapts_foreground(short_side: f32) -> bool {
    short_side < SMALL_CONTROL_SIDE
}

/// What a small glass shape's face looks like, which picks its foreground ([`FaceTone::matrix`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FaceTone {
    /// A dark face: light text and symbols.
    Dark,
    /// A light face: dark text and symbols.
    Light,
}

impl FaceTone {
    /// The foreground matrix of a face of this tone: the dark appearance's over a dark face, the light one's over a
    /// light face.
    pub fn matrix(self) -> ColorMatrix {
        foreground_matrix(self == FaceTone::Dark)
    }
}

/// The relative luminance of a face where white and black text read equally well (both about 4.6:1): above it a small
/// shape's face counts as light.
pub const FACE_TONE_THRESHOLD: f32 = 0.179;
/// How far past [`FACE_TONE_THRESHOLD`] the luminance must go before the tone changes, so that a backdrop moving about
/// the threshold (video under a control) does not make the text flicker.
pub const FACE_TONE_HYSTERESIS: f32 = 0.03;

/// The tone of a face of relative luminance `luminance`, which had tone `previous` (`None` when it was not measured
/// yet). The tone changes only once the luminance is [`FACE_TONE_HYSTERESIS`] past the threshold on the other side.
pub fn face_tone(luminance: f32, previous: Option<FaceTone>) -> FaceTone {
    let light_from = match previous {
        None => FACE_TONE_THRESHOLD,
        Some(FaceTone::Dark) => FACE_TONE_THRESHOLD + FACE_TONE_HYSTERESIS,
        Some(FaceTone::Light) => FACE_TONE_THRESHOLD - FACE_TONE_HYSTERESIS,
    };
    if luminance > light_from {
        FaceTone::Light
    } else {
        FaceTone::Dark
    }
}

/// The foreground matrix of text and symbols on a glass shape of `dark` appearance whose face was measured as `tone`:
/// the appearance's matrix until it was measured, or when the shape does not adapt ([`adapts_foreground`]).
pub fn foreground_matrix_for(dark: bool, tone: Option<FaceTone>) -> ColorMatrix {
    tone.map_or_else(|| foreground_matrix(dark), FaceTone::matrix)
}

/// The relative luminance of an encoded sRGB colour, each channel from 0 to 1.
pub fn relative_luminance(rgb: [f32; 3]) -> f32 {
    let lin = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
}

/// The contrast ratio of two encoded sRGB colours, from 1 to 21.
pub fn contrast_ratio(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `m · [rgb, a]`, unclamped.
pub fn apply_matrix(m: &ColorMatrix, c: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|r| {
        let row = &m[r * 5..r * 5 + 5];
        row[0] * c[0] + row[1] * c[1] + row[2] * c[2] + row[3] * c[3] + row[4]
    })
}

/// How strongly a primary label (text, a control's text, a header) is drawn on glass: the face behind it is busier
/// than a window's background, so labels on glass are drawn more opaque than on a plain window.
pub const VIBRANT_PRIMARY: f32 = 1.0;
/// The shift a secondary label's matrix gives every channel of the face under it: down on light glass, up on dark
/// glass (a primary label's goes all the way to black or near white).
pub const SECONDARY_SHIFT_LIGHT: f32 = -0.4;
pub const SECONDARY_SHIFT_DARK: f32 = 0.3;
/// The face a label drawn as a plain colour is taken to lie over, as the window does not know the face: the regular
/// glass's face over a mid grey, in the light and the dark appearance.
pub const LABEL_FACE_LIGHT: f32 = 0.73;
pub const LABEL_FACE_DARK: f32 = 0.43;

/// How strongly a secondary label is drawn on glass whose foreground is the `dark` side's: the opacity at which the
/// side's foreground (black or white) shifts the label face by [`SECONDARY_SHIFT_LIGHT`] or [`SECONDARY_SHIFT_DARK`].
pub fn vibrant_secondary(dark: bool) -> f32 {
    if dark {
        SECONDARY_SHIFT_DARK / (1.0 - LABEL_FACE_DARK)
    } else {
        -SECONDARY_SHIFT_LIGHT / LABEL_FACE_LIGHT
    }
}
/// How strongly a text field's placeholder is drawn on glass.
pub const VIBRANT_PLACEHOLDER: f32 = 0.7;

/// The face the foreground matrix is applied to when the window does not know the face: a mid grey. Both matrices
/// carry any face but the darkest to white or to black, so the exact face hardly matters.
const NOMINAL_FACE: f32 = 0.5;

/// The colour of text and symbols on glass of the `dark` appearance whose face was measured as `tone`
/// ([`foreground_matrix_for`]), as an encoded sRGB colour the window draws at a label's opacity.
pub fn foreground_rgb(dark: bool, tone: Option<FaceTone>) -> [f32; 3] {
    let out = apply_matrix(
        &foreground_matrix_for(dark, tone),
        [NOMINAL_FACE, NOMINAL_FACE, NOMINAL_FACE, 1.0],
    );
    [
        out[0].clamp(0.0, 1.0),
        out[1].clamp(0.0, 1.0),
        out[2].clamp(0.0, 1.0),
    ]
}
