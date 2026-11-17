//! Server-side window decorations: the glue between `cupertino-decorations` (layout, hit
//! testing, rasterisation) and the compositor (textures, pointer interaction).
//!
//! Title bars and rims are recorded as lists of draws ([`cupertino_decorations::DrawList`]) in images cached per
//! window ([`ChromeImage`]), which the renderer draws on its GPU (or paints on the CPU when it cannot); a list is
//! recorded again only when its state key (size, scale, active, hover, pressed, title, appearance) changes.

use std::collections::{hash_map::DefaultHasher, HashMap};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use cupertino_assets::{AssetError, Assets};
use cupertino_decorations::{
    button_presence, ButtonKind, Capabilities, DecorationConfig, DecorationState, Decorations,
    DoubleClickAction, DrawList, Edge, FrameKind, Hit, ResizeEdge, ShadowParams, TitlebarStyle,
};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_theme::{Appearance, Theme};
use cupertino_wm::WindowId;
use smithay::input::pointer::CursorIcon;

use crate::config::Insets;
use crate::render::chrome::ChromeImage;

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
    titlebar: Option<Cached<Rc<ChromeImage>>>,
    rim: Option<Cached<Rc<ChromeImage>>>,
}

/// What the scene needs to draw one window's decorations.
pub struct DecorationBuffers {
    pub titlebar: Rc<ChromeImage>,
    pub titlebar_px: (i32, i32),
    pub rim: Rc<ChromeImage>,
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
    /// What a double click on a title bar does; kept across a change of appearance, which rebuilds the configuration.
    double_click: DoubleClickAction,
    pub cursor: Option<CursorIcon>,
    /// Lists of draws recorded: a title bar or rim whose state changed (diagnostics and tests).
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
            double_click: DoubleClickAction::Zoom,
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
            self.apply_double_click();
            self.cache.clear();
        }
    }

    /// What a double click on a title bar does from now on.
    pub fn set_double_click(&mut self, action: DoubleClickAction) {
        self.double_click = action;
        self.apply_double_click();
    }

    fn apply_double_click(&mut self) {
        self.deco.config.double_click = self.double_click;
        for deco in &mut self.styled {
            deco.config.double_click = self.double_click;
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
            let list = match input.style {
                TitlebarStyle::Standard if input.flush => {
                    self.deco.titlebar_draws(fw, scale, &state, true)
                }
                style => self
                    .for_style_mut(style)
                    .titlebar_draws(fw, scale, &state, false),
            };
            let slot = &mut self.cache.get_mut(&input.id).unwrap().titlebar;
            Self::keep(slot, tb_key, list);
        }
        if render_rim {
            self.renders += 1;
            let list = self
                .for_style(input.style)
                .rim_draws(Size::new(fw, fh), scale);
            let slot = &mut self.cache.get_mut(&input.id).unwrap().rim;
            Self::keep(slot, rim_key, list);
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

    /// `list` as the image of `slot` under `key`: the image already there takes it (and keeps the textures it was
    /// drawn into), or a new one.
    fn keep(slot: &mut Option<Cached<Rc<ChromeImage>>>, key: u64, list: DrawList) {
        match slot {
            Some(c) => {
                c.buffer.set(list);
                c.key = key;
                c.used = true;
            }
            None => {
                *slot = Some(Cached {
                    key,
                    buffer: ChromeImage::new(list, crate::render::uploads::Drawer::Decoration),
                    used: true,
                })
            }
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
