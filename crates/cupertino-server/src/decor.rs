//! Server-side window decorations: the glue between `cupertino-decorations` (layout, hit
//! testing, rasterisation) and the compositor (textures, pointer interaction).
//!
//! Title bars and rims are rasterised on the CPU into premultiplied RGBA buffers, wrapped in
//! `MemoryRenderBuffer`s and cached per window; a buffer is rendered again only when its state key
//! (size, scale, active, hover, pressed, title, appearance) changes.

use std::collections::{hash_map::DefaultHasher, HashMap};
use std::hash::{Hash, Hasher};

use cupertino_assets::{AssetError, Assets};
use cupertino_decorations::{
    button_presence, ButtonKind, Capabilities, DecorationConfig, DecorationState, Decorations,
    DoubleClickAction, Edge, FrameKind, Hit, ResizeEdge, ShadowParams, TitlebarStyle,
};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_theme::{Appearance, Theme};
use cupertino_wm::WindowId;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::input::pointer::CursorIcon;
use smithay::utils::Transform;

use crate::config::Insets;

/// Seconds within which a second press on the title bar is a double click (default
/// 0.5 s).
pub const DOUBLE_CLICK_INTERVAL: f64 = cupertino_input::DEFAULT_DOUBLE_CLICK_TIME;

struct Cached<T> {
    key: u64,
    buffer: T,
    used: bool,
}

#[derive(Default)]
struct WindowBuffers {
    titlebar: Option<Cached<MemoryRenderBuffer>>,
    rim: Option<Cached<MemoryRenderBuffer>>,
}

/// What the scene needs to draw one window's decorations.
pub struct DecorationBuffers {
    pub titlebar: MemoryRenderBuffer,
    pub titlebar_px: (i32, i32),
    pub rim: MemoryRenderBuffer,
    pub rim_px: (i32, i32),
    pub titlebar_pt: f64,
}

/// Input of [`Decor::buffers`].
pub struct DecorInput<'a> {
    pub id: WindowId,
    pub style: TitlebarStyle,
    /// Natural frame size in points (surface geometry plus insets).
    pub frame: (f64, f64),
    pub scale: f64,
    pub active: bool,
    pub title: &'a str,
    pub resizable: bool,
    /// The minimize button is enabled: its application did not disable minimize.
    pub can_minimize: bool,
    /// The zoom button is enabled: the window is resizable and its application did not disable zoom.
    pub can_zoom: bool,
    /// A standard title bar edge to edge across an output, without a window around it: square corners, no rim.
    pub flush: bool,
}

pub struct Decor {
    /// Standard title bars; its text renderer draws the titles.
    deco: Decorations,
    /// The styles whose row the client draws (the buttons only): unified, compact unified and sidebar, in the order of
    /// [`TitlebarStyle::ALL`]; an instance each, because the config carries the style.
    styled: Vec<Decorations>,
    cache: HashMap<WindowId, WindowBuffers>,
    /// Window whose button group is hovered.
    pub hover_group: Option<WindowId>,
    pub pressed: Option<(WindowId, ButtonKind)>,
    last_click: Option<(WindowId, f64)>,
    pub cursor: Option<CursorIcon>,
    /// Rasterisations performed (diagnostics and tests).
    pub renders: u64,
}

fn hash_of(parts: &[u64], title: &str) -> u64 {
    let mut h = DefaultHasher::new();
    parts.hash(&mut h);
    title.hash(&mut h);
    h.finish()
}

/// Decorations of `appearance` and `style`; a high-contrast appearance also selects the
/// increased-contrast half of the shadow table, whose denser rim is the window border.
fn config_for(appearance: Appearance, style: TitlebarStyle) -> DecorationConfig {
    let mut cfg = match Theme::load() {
        Ok(t) => DecorationConfig::from_theme(&t, appearance, style),
        Err(e) => {
            tracing::warn!(error = %e, "theme not loaded, built-in decoration palette");
            DecorationConfig::builtin(appearance, style)
        }
    };
    cfg.increased_contrast = appearance.is_high_contrast();
    cfg
}

/// Which way a resize can change the frame along one axis from its size now (`Window::size_limits`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AxisFreedom {
    pub grow: bool,
    pub shrink: bool,
}

impl AxisFreedom {
    pub fn of(size: f64, min: f64, max: f64) -> Self {
        Self {
            grow: size < max,
            shrink: size > min,
        }
    }

    fn any(self) -> bool {
        self.grow || self.shrink
    }
}

/// The resize cursor of `edge` for a frame that can change as `x` and `y` say (logic,: where the edge
/// can both grow and shrink, the two-way cursor; where only one, the single arrow that way; where neither, none).
/// Corners (the logic is read for edge masks): an axis that cannot change drops out and the corner shows the
/// other axis's edge cursor; with both axes free the diagonal is two-way when both can grow and shrink, single when they
/// agree on one way, and none when they disagree.
pub fn resize_cursor(edge: ResizeEdge, x: AxisFreedom, y: AxisFreedom) -> Option<CursorIcon> {
    use CursorIcon::*;
    // (two-way, outward, inward) for each side
    let side = |f: AxisFreedom, two: CursorIcon, out: CursorIcon, inward: CursorIcon| match (
        f.grow, f.shrink,
    ) {
        (true, true) => Some(two),
        (true, false) => Some(out),
        (false, true) => Some(inward),
        (false, false) => None,
    };
    let east = |f| side(f, EwResize, EResize, WResize);
    let west = |f| side(f, EwResize, WResize, EResize);
    let south = |f| side(f, NsResize, SResize, NResize);
    let north = |f| side(f, NsResize, NResize, SResize);
    let corner = |hx: Option<CursorIcon>,
                  vy: Option<CursorIcon>,
                  two: CursorIcon,
                  out: CursorIcon,
                  inward: CursorIcon| {
        if !x.any() {
            return vy;
        }
        if !y.any() {
            return hx;
        }
        side(
            AxisFreedom {
                grow: x.grow && y.grow,
                shrink: x.shrink && y.shrink,
            },
            two,
            out,
            inward,
        )
    };
    match edge {
        ResizeEdge::Right => east(x),
        ResizeEdge::Left => west(x),
        ResizeEdge::Bottom => south(y),
        ResizeEdge::Top => north(y),
        ResizeEdge::BottomRight => corner(east(x), south(y), NwseResize, SeResize, NwResize),
        ResizeEdge::TopLeft => corner(west(x), north(y), NwseResize, NwResize, SeResize),
        ResizeEdge::TopRight => corner(east(x), north(y), NeswResize, NeResize, SwResize),
        ResizeEdge::BottomLeft => corner(west(x), south(y), NeswResize, SwResize, NeResize),
    }
}

/// Which of close, minimize and zoom take clicks, from the window's capabilities: a window can always be closed, so its
/// group always exists and keeps a button it cannot use in place, dimmed ([`button_presence`]).
pub fn button_enabled(can_minimize: bool, can_zoom: bool) -> [bool; 3] {
    button_presence(
        FrameKind::Titled,
        Capabilities {
            closable: true,
            minimizable: can_minimize,
            resizable: can_zoom,
        },
    )
    .map(|p| p.enabled())
}

/// Blur sigma of the shadow of menus, popovers and panels, in points.
const POPUP_SHADOW_SIGMA: f64 = 17.0;
/// How far below its popup the shadow falls, in points.
const POPUP_SHADOW_OFFSET: f64 = 9.25;
/// Opacity of a popup's shadow in the dark appearance.
const POPUP_SHADOW_DENSITY_DARK: f64 = 0.265;
/// Opacity of a popup's shadow in the light appearance.
const POPUP_SHADOW_DENSITY_LIGHT: f64 = 0.23;

impl Decor {
    pub fn new(appearance: Appearance, assets: &Assets) -> Result<Self, AssetError> {
        Ok(Self {
            deco: Decorations::new(config_for(appearance, TitlebarStyle::Standard), assets)?,
            styled: TitlebarStyle::ALL
                .into_iter()
                .filter(|style| style.client_drawn())
                .map(|style| Decorations::new(config_for(appearance, style), assets))
                .collect::<Result<_, _>>()?,
            cache: HashMap::new(),
            hover_group: None,
            pressed: None,
            last_click: None,
            cursor: None,
            renders: 0,
        })
    }

    pub fn set_appearance(&mut self, appearance: Appearance) {
        if self.deco.config.appearance != appearance {
            self.deco
                .set_config(config_for(appearance, TitlebarStyle::Standard));
            for deco in &mut self.styled {
                let style = deco.config.style;
                deco.set_config(config_for(appearance, style));
            }
            self.cache.clear();
        }
    }

    pub fn decorations(&self) -> &Decorations {
        &self.deco
    }

    /// The decorations of a title bar style.
    pub fn for_style(&self, style: TitlebarStyle) -> &Decorations {
        match style {
            TitlebarStyle::Standard => &self.deco,
            other => &self.styled[other.index() - 1],
        }
    }

    /// How far below the top of the frame of a window of `style` its sheets hang, when not under a title bar: a
    /// window whose client draws its toolbar row has them hang below that row.
    pub fn sheet_top(&self, style: TitlebarStyle) -> Option<f64> {
        style
            .client_drawn()
            .then(|| self.for_style(style).config.metrics.titlebar_height(style))
    }

    fn for_style_mut(&mut self, style: TitlebarStyle) -> &mut Decorations {
        match style {
            TitlebarStyle::Standard => &mut self.deco,
            other => &mut self.styled[other.index() - 1],
        }
    }

    /// Space the decorations of `style` reserve around the client surface.
    pub fn insets(&self, style: TitlebarStyle) -> Insets {
        Insets {
            top: self.deco.config.metrics.titlebar_reserved(style),
            left: 0.0,
            right: 0.0,
            bottom: 0.0,
        }
    }

    /// Distance the input region reaches outside the frame (resize zones).
    pub fn resize_outset(&self) -> f64 {
        self.deco.config.metrics.resize_outset
    }

    /// Hit test at `local` (frame-local points) for a client surface of `content` points.
    pub fn hit_test(
        &self,
        style: TitlebarStyle,
        content: (f64, f64),
        local: (f64, f64),
        resizable: bool,
    ) -> Hit {
        self.for_style(style).hit_test(
            Size::new(content.0, content.1),
            Point::new(local.0, local.1),
            resizable,
        )
    }

    /// The resize zone under `local` (frame-local) of a frame of size `frame` (a client-decorated window's window
    /// geometry, an untitled window's frame); `exterior` for a titled window.
    pub fn resize_hit(
        &self,
        frame: (f64, f64),
        local: (f64, f64),
        exterior: bool,
    ) -> Option<ResizeEdge> {
        cupertino_decorations::resize_hit(
            Rect::new(0.0, 0.0, frame.0, frame.1),
            &self.deco.config.metrics,
            Point::new(local.0, local.1),
            exterior,
        )
    }

    pub fn group_hovered(
        &self,
        style: TitlebarStyle,
        content: (f64, f64),
        local: (f64, f64),
    ) -> bool {
        self.for_style(style).button_group_hovered(
            Size::new(content.0, content.1),
            Point::new(local.0, local.1),
        )
    }

    /// Window shadow parameters from the verified table.
    pub fn shadow_params(&self, active: bool) -> ShadowParams {
        self.deco.shadow_params(active)
    }

    /// Shadow of popups with a material (menus, popovers): the panel row of the table, the class of
    /// panel, utility, popover, combo box, drawer and menu windows (the shadow option
    /// per class is `window-management.md` section 4).
    pub fn popup_shadow_params(&self) -> ShadowParams {
        let dark = self.deco.config.appearance.is_dark();
        let increased_contrast = self.deco.config.increased_contrast;
        let mut params = ShadowParams::lookup(
            cupertino_theme::window_shadow::ShadowOption::Panel,
            true,
            dark,
            increased_contrast,
        );
        if !increased_contrast {
            params.entry.sigma = POPUP_SHADOW_SIGMA;
            params.entry.vertical_offset = POPUP_SHADOW_OFFSET;
            params.entry.density = if dark {
                POPUP_SHADOW_DENSITY_DARK
            } else {
                POPUP_SHADOW_DENSITY_LIGHT
            };
        }
        params
    }

    /// The edge of a window, active or not: the outline the compositor draws just outside its frame (the inner
    /// rim is in the rim buffer of [`Decor::buffers`]).
    pub fn edge(&self, active: bool) -> Edge {
        self.deco.edge(active)
    }

    /// The edge of a popup with a material (a menu, a popover): an outline along its sides.
    pub fn menu_edge(&self) -> Edge {
        self.deco.menu_edge()
    }

    pub fn double_click_action(&self, hit: Hit) -> DoubleClickAction {
        self.deco.double_click_action(hit)
    }

    /// Register a title bar press; true when it completes a double click on the same window.
    pub fn register_click(&mut self, id: WindowId, now: f64) -> bool {
        let double =
            matches!(self.last_click, Some((w, t)) if w == id && now - t <= DOUBLE_CLICK_INTERVAL);
        self.last_click = if double { None } else { Some((id, now)) };
        double
    }

    fn state_for(&self, input: &DecorInput<'_>) -> DecorationState {
        DecorationState {
            active: input.active,
            group_hovered: self.hover_group == Some(input.id),
            pressed: self.pressed.filter(|(w, _)| *w == input.id).map(|(_, k)| k),
            enabled: button_enabled(input.can_minimize, input.can_zoom),
            title: input.title.to_string(),
            resizable: input.resizable,
        }
    }

    /// Title bar and rim buffers of a window, rendered when their state changed.
    pub fn buffers(&mut self, input: &DecorInput<'_>) -> DecorationBuffers {
        let state = self.state_for(input);
        let (fw, fh) = input.frame;
        let scale = input.scale;
        let appearance = self.deco.config.appearance as u64;
        let pressed = state.pressed.map_or(9, |k| k.index() as u64);
        let tb_key = hash_of(
            &[
                fw.to_bits(),
                scale.to_bits(),
                state.active as u64,
                state.group_hovered as u64,
                pressed,
                state.enabled[0] as u64,
                state.enabled[1] as u64,
                state.enabled[2] as u64,
                appearance,
                input.style as u64,
                input.flush as u64,
            ],
            &state.title,
        );
        let rim_key = hash_of(
            &[
                fw.to_bits(),
                fh.to_bits(),
                scale.to_bits(),
                appearance,
                input.style as u64,
            ],
            "",
        );
        let entry = self.cache.entry(input.id).or_default();
        let mut render_tb = false;
        match &mut entry.titlebar {
            Some(c) if c.key == tb_key => c.used = true,
            _ => render_tb = true,
        }
        let mut render_rim = false;
        match &mut entry.rim {
            Some(c) if c.key == rim_key => c.used = true,
            _ => render_rim = true,
        }
        if render_tb {
            self.renders += 1;
            let pm = match input.style {
                TitlebarStyle::Standard if input.flush => {
                    self.deco.render_flush_titlebar(fw, scale, &state)
                }
                style => self.for_style_mut(style).render_titlebar(fw, scale, &state),
            };
            let size = (pm.width() as i32, pm.height() as i32);
            let buf = MemoryRenderBuffer::from_slice(
                pm.data(),
                Fourcc::Abgr8888,
                size,
                1,
                Transform::Normal,
                None,
            );
            self.cache.get_mut(&input.id).unwrap().titlebar = Some(Cached {
                key: tb_key,
                buffer: buf,
                used: true,
            });
        }
        if render_rim {
            self.renders += 1;
            let pm = self
                .for_style(input.style)
                .render_rim(Size::new(fw, fh), scale);
            let size = (pm.width() as i32, pm.height() as i32);
            let buf = MemoryRenderBuffer::from_slice(
                pm.data(),
                Fourcc::Abgr8888,
                size,
                1,
                Transform::Normal,
                None,
            );
            self.cache.get_mut(&input.id).unwrap().rim = Some(Cached {
                key: rim_key,
                buffer: buf,
                used: true,
            });
        }
        let entry = &self.cache[&input.id];
        let tb = entry.titlebar.as_ref().unwrap();
        let rim = entry.rim.as_ref().unwrap();
        let tb_h = self.deco.config.metrics.titlebar_height(input.style);
        let tb_px = (
            (fw * scale).round() as i32,
            (tb_h * scale).round().max(1.0) as i32,
        );
        let rim_px = (
            (fw * scale).round().max(1.0) as i32,
            (fh * scale).round().max(1.0) as i32,
        );
        DecorationBuffers {
            titlebar: tb.buffer.clone(),
            titlebar_px: tb_px,
            rim: rim.buffer.clone(),
            rim_px,
            titlebar_pt: tb_h,
        }
    }

    /// Forget buffers of windows that were not requested since the previous call.
    pub fn end_frame(&mut self) {
        self.cache.retain(|_, w| {
            let mut used = false;
            for c in [&mut w.titlebar, &mut w.rim].into_iter().flatten() {
                used |= std::mem::replace(&mut c.used, false);
            }
            used
        });
    }

    pub fn forget(&mut self, id: WindowId) {
        self.cache.remove(&id);
        if self.hover_group == Some(id) {
            self.hover_group = None;
        }
        if matches!(self.pressed, Some((w, _)) if w == id) {
            self.pressed = None;
        }
    }

    pub fn cached_windows(&self) -> usize {
        self.cache.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_cursors_follow_what_the_frame_can_do() {
        use cupertino_decorations::ResizeEdge::*;
        let free = AxisFreedom {
            grow: true,
            shrink: true,
        };
        let grow = AxisFreedom {
            grow: true,
            shrink: false,
        };
        let shrink = AxisFreedom {
            grow: false,
            shrink: true,
        };
        let fixed = AxisFreedom {
            grow: false,
            shrink: false,
        };
        assert_eq!(resize_cursor(Right, free, free), Some(CursorIcon::EwResize));
        assert_eq!(
            resize_cursor(Right, grow, free),
            Some(CursorIcon::EResize),
            "at the minimum width the edge moves out only"
        );
        assert_eq!(resize_cursor(Left, grow, free), Some(CursorIcon::WResize));
        assert_eq!(
            resize_cursor(Right, shrink, free),
            Some(CursorIcon::WResize),
            "at the maximum it moves in only"
        );
        assert_eq!(resize_cursor(Top, free, grow), Some(CursorIcon::NResize));
        assert_eq!(resize_cursor(Right, fixed, free), None);
        assert_eq!(
            resize_cursor(BottomRight, free, free),
            Some(CursorIcon::NwseResize)
        );
        assert_eq!(
            resize_cursor(TopRight, free, free),
            Some(CursorIcon::NeswResize)
        );
        assert_eq!(
            resize_cursor(BottomRight, grow, grow),
            Some(CursorIcon::SeResize)
        );
        assert_eq!(
            resize_cursor(TopLeft, shrink, shrink),
            Some(CursorIcon::SeResize),
            "inward from the top-left"
        );
        assert_eq!(
            resize_cursor(BottomRight, fixed, free),
            Some(CursorIcon::NsResize),
            "a fixed width leaves the bottom edge's cursor"
        );
        assert_eq!(
            resize_cursor(BottomRight, grow, shrink),
            None,
            "the axes disagree"
        );
        assert_eq!(resize_cursor(BottomLeft, fixed, fixed), None);
        assert_eq!(AxisFreedom::of(400.0, 400.0, f64::INFINITY), grow);
        assert_eq!(AxisFreedom::of(500.0, 400.0, 500.0), shrink);
    }

    /// Needs the asset directory (`CUPERTINO_ASSET_DIR`); fails with the build instructions when it is missing.
    fn decor(appearance: Appearance) -> Decor {
        let assets = Assets::builtin();
        Decor::new(appearance, &assets).unwrap_or_else(|e| panic!("{e}"))
    }

    fn input(id: u64, w: f64) -> DecorInput<'static> {
        DecorInput {
            id: WindowId(id),
            style: TitlebarStyle::Standard,
            frame: (w, 328.0),
            scale: 1.0,
            active: true,
            title: "Title",
            resizable: true,
            can_minimize: true,
            can_zoom: true,
            flush: false,
        }
    }

    #[test]
    fn the_minimize_and_zoom_buttons_follow_the_window_controls() {
        let d = decor(Appearance::Light);
        assert_eq!(d.state_for(&input(1, 400.0)).enabled, [true, true, true]);
        let disabled = DecorInput {
            can_minimize: false,
            can_zoom: false,
            ..input(1, 400.0)
        };
        assert_eq!(
            d.state_for(&disabled).enabled,
            [true, false, false],
            "close always stays enabled"
        );
    }

    #[test]
    fn a_button_that_changes_state_draws_the_title_bar_again() {
        let mut d = decor(Appearance::Light);
        d.buffers(&input(1, 400.0));
        for (can_minimize, can_zoom) in [(false, true), (false, false), (true, false), (true, true)]
        {
            let before = d.renders;
            d.buffers(&DecorInput {
                can_minimize,
                can_zoom,
                ..input(1, 400.0)
            });
            assert_eq!(
                d.renders,
                before + 1,
                "minimize {can_minimize}, zoom {can_zoom}"
            );
        }
    }

    #[test]
    fn insets_follow_the_titlebar_height() {
        let d = decor(Appearance::Light);
        assert_eq!(d.insets(TitlebarStyle::Standard).top, 32.0);
        assert_eq!(d.insets(TitlebarStyle::Standard).vertical(), 32.0);
        assert_eq!(d.insets(TitlebarStyle::Standard).horizontal(), 0.0);
        for style in [
            TitlebarStyle::Unified,
            TitlebarStyle::UnifiedCompact,
            TitlebarStyle::Sidebar,
        ] {
            assert_eq!(
                d.insets(style).vertical(),
                0.0,
                "{style:?}: the row lies over the surface"
            );
        }
    }

    #[test]
    fn sheets_hang_below_the_row_the_client_draws() {
        let d = decor(Appearance::Dark);
        assert_eq!(
            d.sheet_top(TitlebarStyle::Standard),
            None,
            "under the title bar"
        );
        assert_eq!(d.sheet_top(TitlebarStyle::Unified), Some(52.0));
        assert_eq!(d.sheet_top(TitlebarStyle::UnifiedCompact), Some(40.0));
        assert_eq!(d.sheet_top(TitlebarStyle::Sidebar), Some(52.0));
    }

    #[test]
    fn a_sidebar_window_buffers_its_buttons_at_the_top_of_the_sidebar_and_hits_only_them() {
        let mut d = decor(Appearance::Dark);
        for (style, row) in [
            (TitlebarStyle::Sidebar, 52),
            (TitlebarStyle::UnifiedCompact, 40),
        ] {
            let input = DecorInput {
                style,
                ..input(1, 400.0)
            };
            assert_eq!(d.buffers(&input).titlebar_px, (400, row), "{style:?}");
            let content = (400.0, 300.0);
            let zoom = d
                .for_style(style)
                .layout(Size::new(content.0, content.1))
                .button(ButtonKind::Zoom)
                .center;
            // the compact row's group starts 12 pt in, a point nearer the edge than its middle
            let first = if style == TitlebarStyle::UnifiedCompact {
                19.0
            } else {
                f64::from(row) / 2.0
            };
            assert_eq!((zoom.x, zoom.y), (first + 46.0, f64::from(row) / 2.0));
            assert_eq!(
                d.hit_test(style, content, (zoom.x, zoom.y), true),
                Hit::Button(ButtonKind::Zoom)
            );
            assert_eq!(
                d.hit_test(style, content, (200.0, 10.0), true),
                Hit::Content,
                "{style:?}: the client moves the window"
            );
        }
    }

    #[test]
    fn a_unified_window_buffers_the_52_pt_strip_and_hits_only_its_buttons() {
        let mut d = decor(Appearance::Light);
        let unified = DecorInput {
            style: TitlebarStyle::Unified,
            ..input(1, 400.0)
        };
        assert_eq!(d.buffers(&unified).titlebar_px, (400, 52));
        let content = (400.0, 300.0);
        let close = d
            .for_style(TitlebarStyle::Unified)
            .layout(Size::new(content.0, content.1))
            .button(ButtonKind::Close)
            .center;
        let strip = (200.0, 10.0);
        assert_eq!(
            d.hit_test(TitlebarStyle::Unified, content, (close.x, 26.0), true),
            Hit::Button(ButtonKind::Close)
        );
        assert_eq!(
            d.hit_test(TitlebarStyle::Unified, content, strip, true),
            Hit::Content
        );
        assert_eq!(
            d.hit_test(TitlebarStyle::Standard, content, strip, true),
            Hit::Titlebar
        );
    }

    #[test]
    fn buffers_are_cached_until_the_state_changes() {
        let mut d = decor(Appearance::Light);
        let a = d.buffers(&input(1, 400.0));
        d.end_frame();
        let b = d.buffers(&input(1, 400.0));
        let _ = (a, b);
        assert_eq!(d.renders, 2, "title bar and rim rendered once each");
        assert_eq!(d.cached_windows(), 1);
        let mut inactive = input(1, 400.0);
        inactive.active = false;
        let c = d.buffers(&inactive);
        assert_eq!(c.titlebar_px, (400, 32));
        d.end_frame();
        assert_eq!(d.cached_windows(), 1);
        d.end_frame();
        assert_eq!(d.cached_windows(), 0, "unused windows are dropped");
    }

    #[test]
    fn every_change_the_rasters_depend_on_renders_them_again() {
        let mut d = decor(Appearance::Light);
        let mut renders = d.renders;
        let mut changed = |d: &mut Decor, input: &DecorInput<'_>, what: &str| {
            let b = d.buffers(input);
            let rendered = d.renders - renders;
            renders = d.renders;
            assert!(rendered > 0, "{what} renders again");
            b
        };
        changed(&mut d, &input(1, 400.0), "the first frame");
        for scale in [1.25, 1.5, 1.75, 2.0, 1.0] {
            let b = changed(
                &mut d,
                &DecorInput {
                    scale,
                    ..input(1, 400.0)
                },
                "a scale",
            );
            let px = |pt: f64| (pt * scale).round() as i32;
            assert_eq!(b.titlebar_px, (px(400.0), px(32.0)), "{scale}");
            assert_eq!(b.rim_px, (px(400.0), px(328.0)), "{scale}");
        }
        let unchanged = d.renders;
        d.buffers(&input(1, 400.0));
        assert_eq!(d.renders, unchanged, "the same state reuses its rasters");
        changed(
            &mut d,
            &DecorInput {
                can_minimize: false,
                ..input(1, 400.0)
            },
            "a disabled minimize button",
        );
        changed(
            &mut d,
            &DecorInput {
                can_zoom: false,
                ..input(1, 400.0)
            },
            "a disabled zoom button",
        );
        d.set_appearance(Appearance::Dark);
        assert_eq!(d.cached_windows(), 0, "a new appearance drops every raster");
        changed(&mut d, &input(1, 400.0), "the dark appearance");
    }

    #[test]
    fn double_click_needs_the_same_window_within_the_interval() {
        let mut d = decor(Appearance::Light);
        assert!(!d.register_click(WindowId(1), 1.0));
        assert!(d.register_click(WindowId(1), 1.2));
        assert!(
            !d.register_click(WindowId(1), 1.3),
            "a third press starts over"
        );
        assert!(!d.register_click(WindowId(2), 1.4));
        assert!(!d.register_click(WindowId(2), 2.5));
    }

    #[test]
    fn hit_testing_reports_buttons_titlebar_and_resize_zones() {
        let d = decor(Appearance::Light);
        let content = (400.0, 300.0);
        let layout = d.decorations().layout(Size::new(content.0, content.1));
        let close = layout.button(ButtonKind::Close).center;
        assert_eq!(
            d.hit_test(TitlebarStyle::Standard, content, (close.x, close.y), true),
            Hit::Button(ButtonKind::Close)
        );
        assert_eq!(
            d.hit_test(TitlebarStyle::Standard, content, (200.0, 10.0), true),
            Hit::Titlebar
        );
        assert_eq!(
            d.hit_test(TitlebarStyle::Standard, content, (200.0, 200.0), true),
            Hit::Content
        );
        assert!(matches!(
            d.hit_test(TitlebarStyle::Standard, content, (-2.0, 150.0), true),
            Hit::Resize(ResizeEdge::Left)
        ));
        assert!(d.group_hovered(TitlebarStyle::Standard, content, (close.x, close.y)));
    }

    #[test]
    fn increase_contrast_selects_the_contrast_half_of_the_shadow_table() {
        let mut d = decor(Appearance::Light);
        assert!(!d.shadow_params(true).entry.increased_contrast);
        d.set_appearance(Appearance::LightHighContrast);
        assert!(
            d.shadow_params(true).entry.increased_contrast
                && d.shadow_params(false).entry.increased_contrast
        );
        assert!(d.popup_shadow_params().entry.increased_contrast);
        assert_eq!(
            d.decorations().config.appearance,
            Appearance::LightHighContrast
        );
    }

    #[test]
    fn shadow_parameters_come_from_the_verified_table() {
        let d = decor(Appearance::Light);
        let a = d.shadow_params(true);
        let i = d.shadow_params(false);
        assert!(a.density() > i.density());
        assert!(a.sigma() >= i.sigma());
    }
}
