//! The decoration façade: configuration, state, layout, hit-testing and rasterisation of the
//! titlebar and rim into premultiplied RGBA buffers.

use crate::buttons::{button_draws, ButtonState};
use crate::coverage::{Outline, Shade};
use crate::draws::{Draw, DrawList};
use crate::edge::{edge, Edge, EdgeKind};
use crate::hit::{double_click, hit_test, in_button_group, input_region, Hit};
use crate::layout::{title_slot, FrameLayout};
use crate::metrics::{
    ButtonKind, DecorationMetrics, DoubleClickAction, TitleAlignment, TitlebarStyle,
};
use crate::palette::Palette;
use crate::shadow::ShadowParams;
use crate::text::TextRenderer;
use cupertino_assets::{AssetError, Assets, FontRole};
use cupertino_geometry::{CornerStyle, Point, Radii, Rect, Size};
use cupertino_theme::window_shadow::ShadowOption;
use cupertino_theme::{Appearance, Theme, Typography, WindowCornerSize};
use tiny_skia::Pixmap;

#[derive(Clone, Debug)]
pub struct DecorationConfig {
    pub appearance: Appearance,
    pub style: TitlebarStyle,
    /// Regular ([`DecorationMetrics::corner_radius`], 16 pt) or small (10 pt) corner radius.
    pub corner: WindowCornerSize,
    /// Continuous (default) or circular corners; the exact continuous curve is approximate.
    pub corner_style: CornerStyle,
    pub metrics: DecorationMetrics,
    pub palette: Palette,
    pub typography: Typography,
    pub title_alignment: TitleAlignment,
    pub double_click: DoubleClickAction,
    /// Window class selecting the shadow row (table, usage of the classes unknown).
    pub shadow_option: ShadowOption,
    pub increased_contrast: bool,
}

impl DecorationConfig {
    /// Configuration without specification files: built-in palette and typography.
    pub fn builtin(appearance: Appearance, style: TitlebarStyle) -> Self {
        Self::with_palette(
            Palette::builtin(appearance.is_dark()),
            Typography::default(),
            appearance,
            style,
        )
    }

    /// Configuration with colours and typography resolved from the theme.
    pub fn from_theme(theme: &Theme, appearance: Appearance, style: TitlebarStyle) -> Self {
        Self::with_palette(
            Palette::from_theme(theme, appearance),
            theme.tokens.typography().clone(),
            appearance,
            style,
        )
    }

    fn with_palette(
        palette: Palette,
        typography: Typography,
        appearance: Appearance,
        style: TitlebarStyle,
    ) -> Self {
        Self {
            appearance,
            style,
            corner: WindowCornerSize::Regular,
            corner_style: CornerStyle::continuous(),
            metrics: DecorationMetrics::default(),
            palette,
            typography,
            title_alignment: if style.client_drawn() {
                TitleAlignment::Leading
            } else {
                TitleAlignment::Center
            },
            double_click: DoubleClickAction::Zoom,
            shadow_option: ShadowOption::Standard,
            increased_contrast: false,
        }
    }

    /// The window's corner radius: the metrics' radius for the regular size, the theme's small radius otherwise.
    pub fn corner_radius(&self) -> f64 {
        match self.corner {
            WindowCornerSize::Regular => self.metrics.corner_radius,
            small => small.radius_pt(),
        }
    }
}

/// Dynamic state of one window's decorations.
#[derive(Clone, Debug, PartialEq)]
pub struct DecorationState {
    pub active: bool,
    /// Pointer over the button group.
    pub group_hovered: bool,
    pub pressed: Option<ButtonKind>,
    /// Enabled flags of close, minimize, zoom.
    pub enabled: [bool; 3],
    pub title: String,
    pub resizable: bool,
}

impl Default for DecorationState {
    fn default() -> Self {
        Self {
            active: true,
            group_hovered: false,
            pressed: None,
            enabled: [true; 3],
            title: String::new(),
            resizable: true,
        }
    }
}

impl DecorationState {
    pub fn button_state(&self, kind: ButtonKind) -> ButtonState {
        ButtonState {
            window_active: self.active,
            group_hovered: self.group_hovered,
            pressed: self.pressed == Some(kind),
            enabled: self.enabled[kind.index()],
        }
    }
}

pub struct Decorations {
    pub config: DecorationConfig,
    text: TextRenderer,
}

impl Decorations {
    /// Decorations of a configuration, with the `ui` font of `assets`. A style whose row the client draws has no
    /// title here, so it loads no fonts.
    pub fn new(config: DecorationConfig, assets: &Assets) -> Result<Self, AssetError> {
        let text = if config.style.client_drawn() {
            TextRenderer::without_fonts()
        } else {
            TextRenderer::new(assets, FontRole::Ui)?
        };
        Ok(Self { config, text })
    }

    /// Replace the configuration (a new appearance, say) and keep the loaded fonts. The titlebar style must
    /// stay the same: the text renderer was built for it.
    pub fn set_config(&mut self, config: DecorationConfig) {
        assert_eq!(
            config.style, self.config.style,
            "the titlebar style of a Decorations never changes"
        );
        self.config = config;
    }

    pub fn text_renderer(&mut self) -> &mut TextRenderer {
        &mut self.text
    }

    /// Layout for a client content size.
    pub fn layout(&self, content: Size) -> FrameLayout {
        FrameLayout::new(
            content,
            self.config.style,
            &self.config.metrics,
            self.config.corner_radius(),
        )
    }

    /// Frame size (titlebar included) for a client content size.
    pub fn frame_size(&self, content: Size) -> Size {
        self.layout(content).frame.size
    }

    pub fn hit_test(&self, content: Size, p: Point, resizable: bool) -> Hit {
        hit_test(&self.layout(content), &self.config.metrics, p, resizable)
    }

    pub fn button_group_hovered(&self, content: Size, p: Point) -> bool {
        in_button_group(&self.layout(content), p)
    }

    /// Region in which pointer input must reach the decorations (frame plus resize outset).
    pub fn input_region(&self, content: Size) -> Rect {
        input_region(&self.layout(content), &self.config.metrics)
    }

    pub fn double_click_action(&self, hit: Hit) -> DoubleClickAction {
        double_click(hit, self.config.double_click)
    }

    pub fn shadow_params(&self, active: bool) -> ShadowParams {
        ShadowParams::lookup(
            self.config.shadow_option,
            active,
            self.config.appearance.is_dark(),
            self.config.increased_contrast,
        )
    }

    fn title_px(&self, scale: f64) -> f32 {
        (self.config.metrics.title_size * self.config.typography.substitute_size_scale * scale)
            as f32
    }

    fn letter_spacing_px(&self, scale: f64) -> f32 {
        (self
            .config
            .typography
            .tracking
            .letter_spacing(self.config.metrics.title_size, 0.0)
            * scale) as f32
    }

    /// Placement of the title in logical points: `(left edge, text width, text)` after truncation,
    /// or `None` when the title is empty, there is no room, or the client draws the row (and that
    /// title).
    pub fn title_placement(
        &mut self,
        frame_width: f64,
        title: &str,
        scale: f64,
    ) -> Option<(f64, f64, String)> {
        if title.is_empty() || self.config.style.client_drawn() {
            return None;
        }
        self.text.set_scale(scale as f32);
        let layout = self.layout(Size::new(frame_width, 0.0));
        let (px, ls, weight) = (
            self.title_px(scale),
            self.letter_spacing_px(scale),
            self.config.metrics.title_weight,
        );
        let full = self.text.measure(title, px, weight, ls) as f64 / scale;
        let align = self.config.title_alignment;
        let (_, max) = title_slot(
            frame_width,
            layout.group_reach(),
            full,
            align,
            &self.config.metrics,
        )?;
        let (text, w_px) = self.text.fit(title, px, weight, ls, (max * scale) as f32);
        if text.is_empty() {
            return None;
        }
        let w = w_px as f64 / scale;
        let (left, _) = title_slot(
            frame_width,
            layout.group_reach(),
            w,
            align,
            &self.config.metrics,
        )?;
        Some((left, w, text))
    }

    /// Render the titlebar strip: fill, rim along the top corners and sides, separator, buttons and
    /// title. The pixmap is `round(width·scale) × round(titlebar_height·scale)` premultiplied RGBA.
    /// A style whose row the client draws gets the buttons only: the rest of the strip stays
    /// transparent over the client's content, and [`Decorations::render_rim`] draws the rim of the
    /// whole frame.
    pub fn render_titlebar(
        &mut self,
        frame_width: f64,
        scale: f64,
        state: &DecorationState,
    ) -> Pixmap {
        self.titlebar_draws(frame_width, scale, state, false)
            .paint()
    }

    /// The standard titlebar strip of [`Decorations::render_titlebar`] edge to edge across an output: square corners
    /// and no rim.
    pub fn render_flush_titlebar(
        &mut self,
        frame_width: f64,
        scale: f64,
        state: &DecorationState,
    ) -> Pixmap {
        self.titlebar_draws(frame_width, scale, state, true).paint()
    }

    /// The draws of the title bar strip [`Decorations::render_titlebar`] paints (`flush`: of
    /// [`Decorations::render_flush_titlebar`]), in device pixels of the strip: the bar, its separator, the rim along
    /// its top, the buttons and the title.
    pub fn titlebar_draws(
        &mut self,
        frame_width: f64,
        scale: f64,
        state: &DecorationState,
        flush: bool,
    ) -> DrawList {
        self.text.set_scale(scale as f32);
        let cfg = &self.config;
        let layout = self.layout(Size::new(frame_width, 0.0));
        let tb = layout.titlebar;
        let (w, h) = (
            (tb.width() * scale).round().max(1.0) as u32,
            (tb.height() * scale).round().max(1.0) as u32,
        );
        let mut list = DrawList::new(w, h);
        if cfg.style.client_drawn() {
            for b in &layout.buttons {
                button_draws(
                    &mut list.draws,
                    b.kind,
                    b.center,
                    scale,
                    &cfg.metrics,
                    &cfg.palette,
                    state.button_state(b.kind),
                );
            }
            return list;
        }
        let r = if flush { 0.0 } else { cfg.corner_radius() };
        let fill = cfg.palette.titlebar(cfg.style, state.active);
        let (pw, ph) = (w as f64 / scale, h as f64 / scale);
        let bar = Outline::rounded(
            Rect::new(0.0, 0.0, pw, ph),
            Radii::top(r),
            cfg.corner_style,
            scale,
        );
        list.draws.push(Draw::Shape {
            outline: bar,
            shade: Shade::Solid(fill),
            stroke: None,
        });
        if cfg.style == TitlebarStyle::Standard {
            let sep = (scale.round().max(1.0)) as f32;
            list.draws.push(Draw::Rect {
                x: 0.0,
                y: h as f32 - sep,
                width: w as f32,
                height: sep,
                color: cfg.palette.separator,
            });
        }
        if !flush {
            list.draws
                .extend(self.rim_stroke(Size::new(pw, ph + 4.0 * r), scale));
        }
        let cfg = &self.config;
        for b in &layout.buttons {
            button_draws(
                &mut list.draws,
                b.kind,
                b.center,
                scale,
                &cfg.metrics,
                &cfg.palette,
                state.button_state(b.kind),
            );
        }
        if let Some((left, _, text)) = self.title_placement(frame_width, &state.title, scale) {
            let cfg = &self.config;
            let px = self.title_px(scale);
            let ls = self.letter_spacing_px(scale);
            let cap = cfg.typography.metrics.cap_height / cfg.typography.metrics.units_per_em;
            let baseline = ((tb.height() / 2.0) * scale) as f32 + (cap as f32 * px) / 2.0;
            let color = if state.active {
                cfg.palette.title_active
            } else {
                cfg.palette.title_inactive
            };
            let weight = cfg.metrics.title_weight;
            let glyphs = self.text.place(
                &text,
                (left * scale) as f32,
                baseline,
                px,
                weight,
                ls,
                color,
            );
            list.draws.push(Draw::Glyphs { glyphs, color });
        }
        list
    }

    /// The edge of the window, active or not: the outline the compositor draws just outside the frame and the inner
    /// rim [`Decorations::render_rim`] draws just inside it.
    pub fn edge(&self, active: bool) -> Edge {
        self.edge_of(EdgeKind::Window, active)
    }

    /// The edge of a menu or popover in this appearance: an outline along its sides and a light rim inside, brightest
    /// at the top and bottom.
    pub fn menu_edge(&self) -> Edge {
        self.edge_of(EdgeKind::Menu, true)
    }

    fn edge_of(&self, kind: EdgeKind, active: bool) -> Edge {
        let cfg = &self.config;
        edge(
            kind,
            cfg.appearance.is_dark(),
            active,
            cfg.increased_contrast,
            // the high-contrast rim colour is the same at every scale
            cfg.palette.rim(1.0),
        )
    }

    /// The stroke of the inner rim of the edge ([`Decorations::edge`]) just inside the outline of a frame of `size`
    /// logical points: whole device pixels, at least one.
    fn rim_stroke(&self, size: Size, scale: f64) -> Option<Draw> {
        let cfg = &self.config;
        let inner = self.edge(true).inner?;
        let (px, color) = inner.device(scale);
        let frame = Outline::rounded(
            Rect::new(0.0, 0.0, size.width, size.height),
            Radii::uniform(cfg.corner_radius()),
            cfg.corner_style,
            scale,
        );
        Some(Draw::Shape {
            outline: frame,
            shade: Shade::Solid(color),
            stroke: Some(px as f32),
        })
    }

    /// The inner rim of the edge around the whole frame (premultiplied RGBA, transparent inside), whole device pixels
    /// wide. Meant to be drawn over the client content; the outline outside the frame is the compositor's.
    pub fn render_rim(&self, frame_size: Size, scale: f64) -> Pixmap {
        self.rim_draws(frame_size, scale).paint()
    }

    /// The draws of the rim [`Decorations::render_rim`] paints, in device pixels of the frame.
    pub fn rim_draws(&self, frame_size: Size, scale: f64) -> DrawList {
        let (w, h) = (
            (frame_size.width * scale).round().max(1.0) as u32,
            (frame_size.height * scale).round().max(1.0) as u32,
        );
        let mut list = DrawList::new(w, h);
        list.draws
            .extend(self.rim_stroke(Size::new(w as f64 / scale, h as f64 / scale), scale));
        list
    }

    /// Mask path of the window shape (all four corners rounded) for clipping client content.
    pub fn shape(&self, frame_size: Size) -> cupertino_geometry::RoundedRect {
        cupertino_geometry::RoundedRect::new(
            Rect::new(0.0, 0.0, frame_size.width, frame_size.height),
            self.config.corner_radius(),
            self.config.corner_style,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decorations(style: TitlebarStyle, appearance: Appearance) -> Decorations {
        decorations_with(DecorationConfig::builtin(appearance, style))
    }

    /// Uses the built-in assets only, so the result does not depend on the packs installed on the machine.
    fn decorations_with(config: DecorationConfig) -> Decorations {
        let assets = Assets::builtin();
        Decorations::new(config, &assets).unwrap_or_else(|e| panic!("{e}"))
    }

    fn rgba(pm: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let p = pm.pixel(x, y).unwrap();
        [p.red(), p.green(), p.blue(), p.alpha()]
    }

    fn near(a: [u8; 4], b: [f32; 4], tol: f32) -> bool {
        (0..3).all(|i| (a[i] as f32 / 255.0 - b[i]).abs() <= tol)
    }

    #[test]
    fn a_titlebar_is_its_list_of_draws_the_bar_its_separator_rim_buttons_and_title() {
        let mut d = decorations(TitlebarStyle::Standard, Appearance::Light);
        let state = DecorationState {
            title: "Documents".into(),
            group_hovered: true,
            ..DecorationState::default()
        };
        let list = d.titlebar_draws(800.0, 1.5, &state, false);
        assert_eq!((list.width, list.height), (1200, 48));
        let kinds: Vec<&str> = list
            .draws
            .iter()
            .map(|d| match d {
                Draw::Shape { stroke: None, .. } => "fill",
                Draw::Shape { .. } => "stroke",
                Draw::Rect { .. } => "rect",
                Draw::Path { .. } => "path",
                Draw::Glyphs { .. } => "glyphs",
            })
            .collect();
        // the bar, the separator and the rim along the top; each hovered button a face, two rings and its glyph
        // (the zoom glyph two triangles); the title last
        assert_eq!(&kinds[..3], ["fill", "rect", "stroke"]);
        assert_eq!(kinds.iter().filter(|k| **k == "path").count(), 2 + 1 + 2);
        assert_eq!(kinds.iter().filter(|k| **k == "stroke").count(), 1 + 3 * 2);
        assert_eq!(kinds.last(), Some(&"glyphs"));
        // what the list paints is the title bar
        let painted = list.paint();
        assert_eq!(painted.data(), d.render_titlebar(800.0, 1.5, &state).data());
        // the title's glyphs keep their identity from one list to the next, for an atlas to keep them
        let keys = |list: &DrawList| match list.draws.last() {
            Some(Draw::Glyphs { glyphs, .. }) => {
                glyphs.iter().map(|g| (g.key, g.x, g.y)).collect::<Vec<_>>()
            }
            _ => Vec::new(),
        };
        let again = d.titlebar_draws(800.0, 1.5, &state, false);
        assert!(!keys(&list).is_empty());
        assert_eq!(keys(&list), keys(&again));
        // the rim alone is one stroke over the whole frame
        let rim = d.rim_draws(Size::new(800.0, 600.0), 1.5);
        assert_eq!((rim.width, rim.height, rim.draws.len()), (1200, 900, 1));
        assert!(matches!(
            rim.draws[0],
            Draw::Shape {
                stroke: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn titlebar_has_the_expected_size_at_fractional_scales() {
        let mut d = decorations(TitlebarStyle::Standard, Appearance::Light);
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let pm = d.render_titlebar(800.0, scale, &DecorationState::default());
            assert_eq!(pm.width(), (800.0f64 * scale).round() as u32);
            assert_eq!(pm.height(), (32.0f64 * scale).round() as u32);
        }
        let u = decorations(TitlebarStyle::Unified, Appearance::Light).render_titlebar(
            800.0,
            1.0,
            &DecorationState::default(),
        );
        assert_eq!(u.height(), 52);
    }

    /// The centre of the strongly coloured ink (a button's hue, not the grey fill or the title) inside `box_pt`
    /// (x, y, size in points), in points.
    fn ink_centre(pm: &Pixmap, scale: f64, box_pt: (f64, f64, f64)) -> Option<(f64, f64)> {
        let (x0, y0) = (
            (box_pt.0 * scale).floor() as u32,
            (box_pt.1 * scale).floor() as u32,
        );
        let side = (box_pt.2 * scale).ceil() as u32 + 1;
        let (mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0);
        for y in y0..(y0 + side).min(pm.height()) {
            for x in x0..(x0 + side).min(pm.width()) {
                let [r, g, b, _] = rgba(pm, x, y);
                let chroma = f64::from(r.max(g).max(b) - r.min(g).min(b));
                // every strongly coloured pixel counts once: a face is less saturated at its bottom than its top
                if chroma > 60.0 {
                    sx += f64::from(x) + 0.5;
                    sy += f64::from(y) + 0.5;
                    sw += 1.0;
                }
            }
        }
        (sw > 0.0).then(|| (sx / sw / scale, sy / sw / scale))
    }

    #[test]
    fn the_same_titlebar_at_every_scale_is_complete_and_keeps_its_geometry() {
        // Our own criterion: at each scale the strip is fully painted inside, every button is drawn with its centre
        // at the same point within half a device pixel, the title has ink, and the inner rim covers whole device
        // pixels. Going through other scales and back draws the same pixels again (nothing stale is reused), and the
        // dark appearance changes colours without moving a control.
        let state = DecorationState {
            title: "Document".into(),
            ..Default::default()
        };
        let width = 400.0;
        let mut d = decorations(TitlebarStyle::Standard, Appearance::Light);
        let first_two = d.render_titlebar(width, 2.0, &state).data().to_vec();
        let layout = d.layout(Size::new(width, 0.0));
        let mut dark = decorations(TitlebarStyle::Standard, Appearance::Dark);
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let pm = d.render_titlebar(width, scale, &state);
            assert_eq!(
                (pm.width(), pm.height()),
                (
                    (width * scale).round() as u32,
                    (32.0 * scale).round() as u32
                ),
                "{scale}"
            );
            // complete: no hole in the strip away from its rounded corners and its edges
            for y in (2.0 * scale).ceil() as u32..(28.0 * scale) as u32 {
                for x in (20.0 * scale) as u32..((width - 20.0) * scale) as u32 {
                    assert_eq!(rgba(&pm, x, y)[3], 255, "{scale}: ({x}, {y}) is painted");
                }
            }
            let dark_pm = dark.render_titlebar(width, scale, &state);
            for b in &layout.buttons {
                let r = 7.0;
                let at = (b.center.x - r - 1.0, b.center.y - r - 1.0, 2.0 * r + 2.0);
                let light = ink_centre(&pm, scale, at).expect("the button is drawn");
                let tolerance = 0.5 / scale;
                assert!(
                    (light.0 - b.center.x).abs() <= tolerance
                        && (light.1 - b.center.y).abs() <= tolerance,
                    "{scale}: {:?} drawn at {light:?}, laid out at {:?}",
                    b.kind,
                    b.center
                );
                let dark_centre =
                    ink_centre(&dark_pm, scale, at).expect("the dark button is drawn");
                assert!(
                    (dark_centre.0 - light.0).abs() <= tolerance
                        && (dark_centre.1 - light.1).abs() <= tolerance,
                    "{scale}: the dark appearance moves {:?}",
                    b.kind
                );
            }
            let (left, w, _) = d.title_placement(width, "Document", scale).unwrap();
            let fill = rgba(&pm, (left * scale) as u32 - 2, (16.0 * scale) as u32);
            let ink = (((left * scale) as u32)..((left + w) * scale) as u32)
                .flat_map(|x| (0..pm.height()).map(move |y| (x, y)))
                .filter(|&(x, y)| rgba(&pm, x, y) != fill)
                .count();
            assert!(
                ink > (20.0 * scale * scale) as usize,
                "{scale}: the title is drawn ({ink} pixels)"
            );
            assert_ne!(
                rgba(&pm, (width * scale) as u32 / 2, 2),
                rgba(&dark_pm, (width * scale) as u32 / 2, 2)
            );

            // the rim lies on whole device pixels at both sides
            let rim = d.render_rim(Size::new(width, 300.0), scale);
            let (px, _) = d.edge(true).inner.unwrap().device(scale);
            let mid = rim.height() / 2;
            for (x, inward) in [(0, 1i64), (rim.width() - 1, -1)] {
                let alpha: Vec<u8> = (0..=px as i64)
                    .map(|i| rgba(&rim, (x as i64 + inward * i) as u32, mid)[3])
                    .collect();
                let (line, inside) = alpha.split_at(px as usize);
                assert!(
                    line.iter().all(|&a| a > 0 && a == line[0]),
                    "{scale}: {alpha:?}"
                );
                assert_eq!(inside, [0], "{scale}: nothing inside the rim");
            }
        }
        for scale in [1.25, 1.75, 1.0] {
            d.render_titlebar(width, scale, &state);
        }
        assert!(
            d.render_titlebar(width, 2.0, &state).data() == first_two.as_slice(),
            "the strip at 2 after other scales is the first one"
        );
    }

    #[test]
    fn a_flush_titlebar_fills_its_top_corners() {
        let mut d = decorations(TitlebarStyle::Standard, Appearance::Light);
        let state = DecorationState::default();
        let rounded = d.render_titlebar(800.0, 2.0, &state);
        let flush = d.render_flush_titlebar(800.0, 2.0, &state);
        assert_eq!(rgba(&rounded, 0, 0)[3], 0, "a window's corner is cut");
        for (x, y) in [(0, 0), (1599, 0), (0, 20)] {
            assert!(
                near(rgba(&flush, x, y), d.config.palette.titlebar_active, 0.02),
                "({x}, {y}) is the fill, no corner and no rim: {:?}",
                rgba(&flush, x, y)
            );
        }
    }

    #[test]
    fn fill_colour_comes_from_the_palette_and_stays_when_the_window_is_not_active() {
        let mut d = decorations(TitlebarStyle::Standard, Appearance::Light);
        let active = d.render_titlebar(800.0, 1.0, &DecorationState::default());
        let inactive = d.render_titlebar(
            800.0,
            1.0,
            &DecorationState {
                active: false,
                ..Default::default()
            },
        );
        assert!(near(
            rgba(&active, 400, 14),
            d.config.palette.titlebar_active,
            0.01
        ));
        assert!(near(
            rgba(&inactive, 400, 14),
            d.config.palette.titlebar_inactive,
            0.01
        ));
        assert_eq!(
            rgba(&active, 400, 14),
            rgba(&inactive, 400, 14),
            "an inactive window's title bar is the active one's"
        );
        let mut dark = decorations(TitlebarStyle::Standard, Appearance::Dark);
        let da = dark.render_titlebar(800.0, 1.0, &DecorationState::default());
        assert!(near(
            rgba(&da, 400, 14),
            dark.config.palette.titlebar_active,
            0.01
        ));
        assert!(rgba(&da, 400, 14)[0] < rgba(&active, 400, 14)[0]);
    }

    #[test]
    fn corners_are_transparent_and_the_rim_is_one_pixel() {
        let mut d = decorations(TitlebarStyle::Standard, Appearance::Light);
        let pm = d.render_titlebar(400.0, 1.0, &DecorationState::default());
        assert_eq!(rgba(&pm, 0, 0)[3], 0, "outside the rounded corner");
        assert_eq!(rgba(&pm, 399, 0)[3], 0);
        assert_eq!(rgba(&pm, 200, 0)[3], 255, "rim over fill is opaque");
        let (_, rim) = d.edge(true).inner.unwrap().device(1.0);
        let fill = d.config.palette.titlebar_active;
        let top = rgba(&pm, 200, 0);
        let expect: Vec<f32> = (0..3)
            .map(|i| rim[i] * rim[3] + fill[i] * (1.0 - rim[3]))
            .collect();
        for i in 0..3 {
            assert!(
                (top[i] as f32 / 255.0 - expect[i]).abs() < 0.02,
                "rim blends on the first row: {top:?} vs {expect:?}"
            );
        }
        assert!(
            near(rgba(&pm, 200, 1), fill, 0.01),
            "second row is plain fill"
        );
    }

    #[test]
    fn rim_is_a_faint_white_ring_inside_the_frame() {
        let d = decorations(TitlebarStyle::Standard, Appearance::Dark);
        let ring = d.render_rim(Size::new(300.0, 200.0), 1.0);
        assert_eq!((ring.width(), ring.height()), (300, 200));
        assert_eq!(rgba(&ring, 150, 100)[3], 0);
        // white at a tenth, premultiplied
        let edge = rgba(&ring, 150, 0);
        assert!((edge[3] as i32 - 26).abs() <= 1, "{edge:?}");
        assert_eq!(edge[0], edge[3], "white");
        assert_eq!(rgba(&ring, 150, 1)[3], 0);
        for (x, y) in [(0, 100), (299, 100), (150, 199)] {
            assert!((rgba(&ring, x, y)[3] as i32 - 26).abs() <= 1);
        }
        let ring2 = d.render_rim(Size::new(300.0, 200.0), 2.0);
        assert!(
            rgba(&ring2, 300, 1)[3] > 20 && rgba(&ring2, 300, 2)[3] == 0,
            "a point wide in the dark appearance: two device pixels at 2x"
        );
        let light = decorations(TitlebarStyle::Standard, Appearance::Light);
        let ring2 = light.render_rim(Size::new(300.0, 200.0), 2.0);
        let edge = rgba(&ring2, 300, 0);
        assert!((edge[3] as i32 - 13).abs() <= 1, "{edge:?}");
        assert_eq!(
            rgba(&ring2, 300, 1)[3],
            0,
            "half a point in the light appearance: one device pixel at 2x"
        );
    }

    #[test]
    fn the_edge_follows_the_appearance_and_increase_contrast() {
        let dark = decorations(TitlebarStyle::Sidebar, Appearance::Dark);
        assert_eq!(
            dark.edge(true).outline.color[3],
            crate::edge::WINDOW_OUTLINE_DARK
        );
        assert_eq!(
            dark.menu_edge().outline.color[3],
            crate::edge::MENU_OUTLINE_DARK
        );
        let mut cfg =
            DecorationConfig::builtin(Appearance::DarkHighContrast, TitlebarStyle::Standard);
        cfg.increased_contrast = true;
        let contrast = decorations_with(cfg);
        let e = contrast.edge(false);
        assert_eq!(e.outline.color, contrast.config.palette.rim(1.0));
        assert_eq!(e.outline.width_pt, 1.0);
    }

    /// Each coloured button's face, sampled down its centre line at 2x: its upper colour over the top, its own colour
    /// at the centre and its lower colour near the bottom, in each appearance; the dark discs are deeper and hold no
    /// blue in their upper half.
    #[test]
    fn button_faces_follow_the_appearance() {
        let want: [(Appearance, [[[u8; 3]; 3]; 3]); 2] = [
            (
                Appearance::Light,
                [
                    [[252, 110, 101], [248, 122, 114], [246, 136, 129]],
                    [[252, 188, 43], [253, 198, 56], [253, 209, 71]],
                    [[104, 208, 56], [127, 212, 88], [150, 217, 119]],
                ],
            ),
            (
                Appearance::Dark,
                [
                    [[246, 85, 72], [243, 90, 80], [240, 103, 95]],
                    [[251, 178, 0], [253, 190, 0], [253, 204, 43]],
                    [[13, 191, 0], [35, 192, 0], [79, 195, 55]],
                ],
            ),
        ];
        for (appearance, faces) in want {
            let mut d = decorations(TitlebarStyle::Standard, appearance);
            let layout = d.layout(Size::new(400.0, 100.0));
            let r = d.config.metrics.button_diameter / 2.0;
            let pm = d.render_titlebar(400.0, 2.0, &DecorationState::default());
            for (k, face) in ButtonKind::ALL.into_iter().zip(faces) {
                let b = layout.button(k);
                for (at, want) in [(-0.7, face[0]), (0.0, face[1]), (0.7, face[2])] {
                    let (x, y) = (b.center.x * 2.0, (b.center.y + at * r) * 2.0);
                    let got = rgba(&pm, x as u32, y as u32);
                    assert!(
                        (0..3).all(|i| (got[i] as i32 - want[i] as i32).abs() <= 6),
                        "{appearance:?} {k:?} at {at}: {got:?} against {want:?}"
                    );
                }
            }
        }
        let (light, dark) = (Palette::builtin(false), Palette::builtin(true));
        assert!(dark.zoom[2] < light.zoom[2] && dark.minimize[2] < light.minimize[2]);
    }

    #[test]
    fn button_centre_colours_per_state() {
        let mut d = decorations(TitlebarStyle::Standard, Appearance::Light);
        let layout = d.layout(Size::new(400.0, 100.0));
        let c = |k: ButtonKind| layout.button(k).center;
        let px = |pm: &Pixmap, k: ButtonKind| rgba(pm, c(k).x as u32, c(k).y as u32);
        // the face's centre is the button's colour; the sampled pixel lies half a point below it, where the face
        // already lightens
        let face = |fill: [f32; 4]| fill;

        let normal = d.render_titlebar(400.0, 1.0, &DecorationState::default());
        assert!(
            near(
                px(&normal, ButtonKind::Close),
                face(d.config.palette.close),
                0.03
            ),
            "close fill at the centre"
        );
        assert!(near(
            px(&normal, ButtonKind::Minimize),
            face(d.config.palette.minimize),
            0.03
        ));
        assert!(near(
            px(&normal, ButtonKind::Zoom),
            face(d.config.palette.zoom),
            0.03
        ));

        let hover = d.render_titlebar(
            400.0,
            1.0,
            &DecorationState {
                group_hovered: true,
                ..Default::default()
            },
        );
        for k in ButtonKind::ALL {
            let (n, h) = (px(&normal, k), px(&hover, k));
            assert!(
                h[0] < n[0] || h[1] < n[1],
                "{k:?}: glyph darkens the centre pixel ({n:?} → {h:?})"
            );
        }

        let pressed = d.render_titlebar(
            400.0,
            1.0,
            &DecorationState {
                pressed: Some(ButtonKind::Close),
                ..Default::default()
            },
        );
        let pc = px(&pressed, ButtonKind::Close);
        assert!(
            pc[0] < px(&normal, ButtonKind::Close)[0] && pc[1] < px(&normal, ButtonKind::Close)[1]
        );
        assert!(
            near(
                px(&pressed, ButtonKind::Zoom),
                face(d.config.palette.zoom),
                0.03
            ),
            "other buttons unaffected"
        );

        let inactive = d.render_titlebar(
            400.0,
            1.0,
            &DecorationState {
                active: false,
                ..Default::default()
            },
        );
        for k in ButtonKind::ALL {
            let p = px(&inactive, k);
            assert!(
                (p[0] as i32 - p[1] as i32).abs() <= 2 && (p[1] as i32 - p[2] as i32).abs() <= 2,
                "{k:?} grey: {p:?}"
            );
        }
        assert_eq!(
            px(&inactive, ButtonKind::Close),
            px(&inactive, ButtonKind::Zoom)
        );

        let disabled = d.render_titlebar(
            400.0,
            1.0,
            &DecorationState {
                enabled: [true, true, false],
                group_hovered: true,
                ..Default::default()
            },
        );
        let z = px(&disabled, ButtonKind::Zoom);
        assert!(
            (z[0] as i32 - z[1] as i32).abs() <= 2,
            "disabled zoom is grey even while hovered"
        );
        let close_c = c(ButtonKind::Close);
        let off_glyph = rgba(&disabled, close_c.x as u32, close_c.y as u32 + 4);
        assert!(
            off_glyph[0] > 150 && off_glyph[1] < 150,
            "close stays red: {off_glyph:?}"
        );
    }

    #[test]
    fn glyphs_appear_on_all_three_buttons_when_the_group_is_hovered() {
        let mut d = decorations(TitlebarStyle::Standard, Appearance::Light);
        let layout = d.layout(Size::new(400.0, 100.0));
        let normal = d.render_titlebar(400.0, 2.0, &DecorationState::default());
        let hover = d.render_titlebar(
            400.0,
            2.0,
            &DecorationState {
                group_hovered: true,
                ..Default::default()
            },
        );
        for b in &layout.buttons {
            let (x0, y0) = (
                ((b.center.x - 6.0) * 2.0) as u32,
                ((b.center.y - 6.0) * 2.0) as u32,
            );
            let mut changed = 0;
            for y in y0..y0 + 24 {
                for x in x0..x0 + 24 {
                    if rgba(&normal, x, y) != rgba(&hover, x, y) {
                        changed += 1;
                    }
                }
            }
            assert!(changed > 15, "{:?}: {changed} pixels changed", b.kind);
        }
    }

    #[test]
    fn title_is_drawn_centred() {
        let mut d = decorations(TitlebarStyle::Standard, Appearance::Light);
        let state = DecorationState {
            title: "Document".into(),
            ..Default::default()
        };
        let pm = d.render_titlebar(400.0, 1.0, &state);
        let fill = d.config.palette.titlebar_active;
        let (mut min_x, mut max_x, mut n) = (u32::MAX, 0, 0);
        for y in 4..24 {
            for x in 80..320 {
                let p = rgba(&pm, x, y);
                if (p[0] as f32 / 255.0) < fill[0] - 0.4 {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                    n += 1;
                }
            }
        }
        assert!(n > 60, "title pixels {n}");
        let centre = (min_x + max_x) as f64 / 2.0;
        assert!((centre - 200.0).abs() < 4.0, "title centre {centre}");
        let placement = d.title_placement(400.0, "Document", 1.0).unwrap();
        assert!((placement.0 + placement.1 / 2.0 - 200.0).abs() < 0.01);
    }

    #[test]
    fn the_title_has_the_same_width_in_points_at_every_scale() {
        // the optical size follows the point size, not the pixel size: the title at scale 2 is laid out like at scale 1
        let mut d = decorations(TitlebarStyle::Standard, Appearance::Light);
        let (_, one, text_one) = d.title_placement(400.0, "Document Title", 1.0).unwrap();
        let (_, two, text_two) = d.title_placement(400.0, "Document Title", 2.0).unwrap();
        assert_eq!(text_one, text_two);
        assert!(
            (one - two).abs() < 0.5,
            "{one} points at scale 1, {two} at scale 2"
        );
        // and the width is the one of the title's point size: a renderer told the scale measures the same
        let mut fresh = decorations(TitlebarStyle::Standard, Appearance::Light);
        fresh.text.set_scale(1.0);
        let measured = f64::from(fresh.text.measure(
            "Document Title",
            fresh.title_px(1.0),
            fresh.config.metrics.title_weight,
            fresh.letter_spacing_px(1.0),
        ));
        assert!(
            (one - measured).abs() < 0.01,
            "{one} points placed, {measured} measured at the point size"
        );
    }

    #[test]
    fn long_titles_are_truncated_and_never_touch_the_buttons() {
        let mut d = decorations(TitlebarStyle::Standard, Appearance::Light);
        let title =
            "An extremely long window title that has to be cut somewhere before the buttons";
        let (left, w, text) = d.title_placement(300.0, title, 1.0).unwrap();
        assert!(text.ends_with('…'));
        let group_right = d.layout(Size::new(300.0, 0.0)).group_right();
        assert!(left >= group_right + d.config.metrics.title_gap - 1e-6);
        assert!(
            left + w <= 300.0 - (group_right + d.config.metrics.title_gap) + 1e-6,
            "symmetric limit"
        );
        assert!(
            d.title_placement(60.0, title, 1.0).is_none(),
            "no room next to the buttons"
        );
        assert!(d.title_placement(300.0, "", 1.0).is_none());
    }

    #[test]
    fn a_unified_bar_draws_only_its_buttons_and_leaves_the_strip_to_the_client() {
        for style in [
            TitlebarStyle::Unified,
            TitlebarStyle::UnifiedCompact,
            TitlebarStyle::Sidebar,
        ] {
            only_buttons(style);
        }
    }

    fn only_buttons(style: TitlebarStyle) {
        let mut d = decorations(style, Appearance::Light);
        let state = DecorationState {
            title: "Files".into(),
            ..Default::default()
        };
        let pm = d.render_titlebar(600.0, 1.0, &state);
        let close = d
            .layout(Size::new(600.0, 100.0))
            .button(ButtonKind::Close)
            .center;
        assert_eq!(
            rgba(&pm, close.x as u32, close.y as u32)[3],
            255,
            "the close button is opaque"
        );
        assert!(
            d.title_placement(600.0, "Files", 1.0).is_none(),
            "the client draws the title"
        );
        let opaque = pm.pixels().iter().filter(|p| p.alpha() > 0).count();
        let group = d.layout(Size::new(600.0, 100.0)).button_group;
        assert!(
            opaque as f64 <= group.width() * group.height(),
            "nothing is drawn outside the button group: {opaque} pixels"
        );
    }

    #[test]
    fn a_unified_frame_has_no_titlebar_hit_and_keeps_the_resize_zones() {
        for style in [
            TitlebarStyle::Unified,
            TitlebarStyle::UnifiedCompact,
            TitlebarStyle::Sidebar,
        ] {
            client_moves(style);
        }
    }

    fn client_moves(style: TitlebarStyle) {
        let d = decorations(style, Appearance::Light);
        let content = Size::new(800.0, 600.0);
        let l = d.layout(content);
        assert_eq!(
            d.hit_test(content, l.buttons[2].center, true),
            Hit::Button(ButtonKind::Zoom)
        );
        assert_eq!(
            d.hit_test(content, Point::new(400.0, 10.0), true),
            Hit::Content,
            "the strip is the client's"
        );
        assert_eq!(
            d.hit_test(content, Point::new(-2.0, 300.0), true),
            Hit::Resize(crate::hit::ResizeEdge::Left)
        );
        assert_eq!(d.frame_size(content), content);
    }

    #[test]
    fn facade_hit_testing_and_actions() {
        let d = decorations(TitlebarStyle::Standard, Appearance::Light);
        let content = Size::new(800.0, 600.0);
        let l = d.layout(content);
        assert_eq!(
            d.hit_test(content, l.buttons[0].center, true),
            Hit::Button(ButtonKind::Close)
        );
        let h = d.hit_test(content, Point::new(400.0, 10.0), true);
        assert_eq!(h, Hit::Titlebar);
        assert_eq!(d.double_click_action(h), DoubleClickAction::Zoom);
        assert!(d.button_group_hovered(content, l.buttons[1].center));
        assert_eq!(d.frame_size(content), Size::new(800.0, 632.0));
        assert_eq!(d.input_region(content), Rect::new(-4.0, -4.0, 808.0, 640.0));
    }

    #[test]
    fn corner_radius_follows_the_window_class() {
        let mut cfg = DecorationConfig::builtin(Appearance::Light, TitlebarStyle::Standard);
        assert_eq!(cfg.corner_radius(), 16.0);
        cfg.corner = WindowCornerSize::Small;
        assert_eq!(
            decorations_with(cfg)
                .layout(Size::new(100.0, 100.0))
                .corner_radius,
            10.0
        );
    }

    #[test]
    fn shadow_selection_follows_appearance_and_activity() {
        let d = decorations(TitlebarStyle::Standard, Appearance::Dark);
        assert_eq!(d.shadow_params(true).density(), 0.70);
        assert_eq!(d.shadow_params(false).density(), 0.45);
        let l = decorations(TitlebarStyle::Standard, Appearance::Light);
        assert_eq!(l.shadow_params(true).density(), 0.40);
    }

    #[test]
    fn theme_configuration_matches_the_builtin_one() {
        let theme = Theme::load().unwrap();
        let a = DecorationConfig::from_theme(&theme, Appearance::Light, TitlebarStyle::Standard);
        let b = DecorationConfig::builtin(Appearance::Light, TitlebarStyle::Standard);
        assert!((a.palette.titlebar_active[0] - b.palette.titlebar_active[0]).abs() < 0.01);
        assert_eq!(a.typography.family, "Inter");
    }
}
