//! The decoration façade: configuration, state, layout, hit-testing and rasterisation of the
//! titlebar and rim into premultiplied RGBA buffers.

use crate::decorations::buttons::{button_draws, ButtonState};
use crate::decorations::coverage::{Outline, Shade};
use crate::decorations::draws::{Draw, DrawList};
use crate::decorations::edge::{edge, Edge, EdgeKind};
use crate::decorations::hit::{double_click, hit_test, in_button_group, input_region, Hit};
use crate::decorations::layout::{title_slot, FrameLayout};
use crate::decorations::metrics::{
    ButtonKind, DecorationMetrics, DoubleClickAction, TitleAlignment, TitlebarStyle,
};
use crate::decorations::palette::Palette;
use crate::decorations::shadow::ShadowParams;
use crate::decorations::text::TextRenderer;
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
        // the bar is the top of the window's outline, the one the rim follows: a strip only as tall as the bar would
        // have its top corners shrunk to half its height (a continuous corner reaches further down than its radius),
        // so the fill would stand out past the rim and the outline at the corners
        let frame = Size::new(pw, ph + 4.0 * r);
        let bar = if flush {
            Outline::rounded(
                Rect::new(0.0, 0.0, pw, ph),
                Radii::uniform(0.0),
                cfg.corner_style,
                scale,
            )
        } else {
            self.frame_outline(frame, scale)
        };
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
            list.draws.extend(self.rim_stroke(frame, scale));
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
        let inner = self.edge(true).inner?;
        let (px, color) = inner.device(scale);
        Some(Draw::Shape {
            outline: self.frame_outline(size, scale),
            shade: Shade::Solid(color),
            stroke: Some(px as f32),
        })
    }

    /// The window's outline for a frame of `size` logical points at `scale`, its four corners rounded; `size` holds
    /// at least twice a corner's extent in each direction, or the corners shrink.
    fn frame_outline(&self, size: Size, scale: f64) -> Outline {
        let cfg = &self.config;
        Outline::rounded(
            Rect::new(0.0, 0.0, size.width, size.height),
            Radii::uniform(cfg.corner_radius()),
            cfg.corner_style,
            scale,
        )
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
