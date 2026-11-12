use crate::accessibility::{
    AccessibleStates, AccessibleValue, Action, Live, Relation, Role, SortOrder,
};
use crate::paint::NodeMaterial;
use crate::route::Placement;
use crate::widget::Change;
use crate::{Environment, Key, NodeId, Phase, WindowKey};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_layout::LayoutDirection;
use cupertino_ui_platform::{CursorShape, GlassRegion, GlassStyle, Timestamp};
use cupertino_ui_render::{
    Effect, Group, ImageResource, PaintOp, ResourceId, ResourceRef, Sampling,
};
use cupertino_ui_text::TextInputState;
use cupertino_ui_theme::Surface;
use std::collections::BTreeSet;

pub struct MeasureCx<'a> {
    pub(crate) node: NodeId,
    pub(crate) env: &'a Environment,
}

impl MeasureCx<'_> {
    pub fn node(&self) -> NodeId {
        self.node
    }

    pub fn env(&self) -> &Environment {
        self.env
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureRequest {
    Capture(NodeId),
    Release,
}

/// Where [`EventCx::focus_item`] moves the focus among a roving group's items, in reading order.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FocusTarget {
    /// The next item, the first after the last.
    Next,
    /// The previous item, the last before the first.
    Previous,
    First,
    Last,
    /// The item at this position; past the last, nothing moves.
    Index(usize),
    /// The item with this key once the view is rebuilt, for an item the old view did not build.
    Key(Key),
}

/// What a widget asked the window system to do with its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowGesture {
    /// An interactive move, answering the pointer press being handled.
    Move,
    /// Maximize, or restore a maximized window.
    Zoom,
}

/// What widgets asked for while one event was routed.
pub(crate) struct Effects<M> {
    pub(crate) messages: Vec<M>,
    pub(crate) focus: Option<NodeId>,
    /// An item of a roving group asked to move the focus within its group.
    pub(crate) focus_item: Option<(NodeId, FocusTarget)>,
    pub(crate) capture: Option<CaptureRequest>,
    pub(crate) changes: Vec<(NodeId, Change)>,
    pub(crate) cursor: Option<Option<CursorShape>>,
    pub(crate) animate: BTreeSet<NodeId>,
    pub(crate) wakes: Vec<(NodeId, Timestamp)>,
    pub(crate) text_input: Option<(NodeId, Option<TextInputState>)>,
    pub(crate) clipboard: Option<String>,
    /// Moves and zooms of the window, in the order asked.
    pub(crate) window: Vec<WindowGesture>,
    /// A part of a node, in its coordinates, for its scroll containers to show.
    pub(crate) reveal: Option<(NodeId, Rect)>,
    /// The window's overlays draw again on the next frame ([`EventCx::request_overlay`]).
    pub(crate) overlay: bool,
}

impl<M> Default for Effects<M> {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            focus: None,
            focus_item: None,
            capture: None,
            changes: Vec::new(),
            cursor: None,
            animate: BTreeSet::new(),
            wakes: Vec::new(),
            text_input: None,
            clipboard: None,
            window: Vec::new(),
            reveal: None,
            overlay: false,
        }
    }
}

impl<M> Effects<M> {
    /// Takes on what a later route asked for; the later request wins where only one can.
    pub(crate) fn absorb(&mut self, other: Self) {
        self.messages.extend(other.messages);
        self.focus = other.focus.or(self.focus);
        self.focus_item = other.focus_item.or(self.focus_item.take());
        self.capture = other.capture.or(self.capture.take());
        self.changes.extend(other.changes);
        self.cursor = other.cursor.or(self.cursor);
        self.animate.extend(other.animate);
        self.wakes.extend(other.wakes);
        if other.text_input.is_some() {
            self.text_input = other.text_input;
        }
        if other.clipboard.is_some() {
            self.clipboard = other.clipboard;
        }
        self.window.extend(other.window);
        self.reveal = other.reveal.or(self.reveal);
        self.overlay |= other.overlay;
    }
}

/// The parts of the runtime's state an event handler reads.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Status {
    pub(crate) focus: Option<NodeId>,
    pub(crate) focus_visible: bool,
    pub(crate) capture: Option<NodeId>,
    pub(crate) window_active: bool,
    pub(crate) now: Timestamp,
    /// When the pointer press that opened this popup went down, if one did.
    pub(crate) opening_press: Option<Timestamp>,
}

pub struct EventCx<'a, M> {
    pub(crate) node: NodeId,
    pub(crate) target: NodeId,
    pub(crate) phase: Phase,
    pub(crate) placement: Option<Placement>,
    pub(crate) env: &'a Environment,
    pub(crate) status: Status,
    /// Where the compositor placed each open popup of the window.
    pub(crate) popups: &'a [(WindowKey, Rect)],
    pub(crate) effects: &'a mut Effects<M>,
}

impl<M> EventCx<'_, M> {
    pub fn node(&self) -> NodeId {
        self.node
    }

    /// The node the event was aimed at; it differs from [`node`](Self::node) during capture and bubble.
    pub fn target(&self) -> NodeId {
        self.target
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn env(&self) -> &Environment {
        self.env
    }

    /// Whether a pointer or touch position, in window coordinates as events carry them, lands on this node and inside
    /// every clip above it; false before the node's first layout.
    pub fn hit(&self, position: Point) -> bool {
        self.placement.is_some_and(|p| p.hit(position))
    }

    /// The node's rect in window coordinates; `None` before its first layout.
    pub fn bounds(&self) -> Option<Rect> {
        self.placement.map(|p| p.rect)
    }

    pub fn now(&self) -> Timestamp {
        self.status.now
    }

    /// When an input event carrying `at` happened, on the clock of [`now`](Self::now): its own time, so events
    /// delivered together keep the spacing they happened with, as a speed measured from them needs. An event without
    /// a time of its own (`at` zero, as one a program makes up) or with one later than now happened now.
    pub fn input_time(&self, at: Timestamp) -> Timestamp {
        let now = self.status.now;
        if at.0 == 0 || at > now {
            now
        } else {
            at
        }
    }

    /// When the pointer press or the touch that opened this popup went down, on the clock of [`now`](Self::now): a
    /// popup opened while a button or a finger was held, such as a menu opened by a press, times press-and-hold from it
    /// rather than from the first event it hears. `None` in a toplevel and in a popup no press or touch opened.
    pub fn opening_press(&self) -> Option<Timestamp> {
        self.status.opening_press
    }

    /// Where the compositor placed the open popup `key` of this window, in window coordinates as events carry them:
    /// beside its anchor, or flipped or slid where there was no room. `None` until the compositor has placed it, and
    /// again from when the popup is asked to move until it is placed anew.
    pub fn popup_placement(&self, key: &WindowKey) -> Option<Rect> {
        self.popups
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, rect)| *rect)
    }

    pub fn is_focused(&self) -> bool {
        self.status.focus == Some(self.node)
    }

    pub fn focus_visible(&self) -> bool {
        self.is_focused() && self.status.focus_visible
    }

    pub fn has_pointer_capture(&self) -> bool {
        self.status.capture == Some(self.node)
    }

    pub fn window_active(&self) -> bool {
        self.status.window_active
    }

    /// Sends `message` to the application's `update`.
    pub fn emit(&mut self, message: M) {
        self.effects.messages.push(message);
    }

    /// Moves the keyboard focus to `target` among the items of this node's roving group (see
    /// [`SemanticsCx::roving_group`]). It moves once the view after this event is built, and only if that item is then a
    /// Tab stop; otherwise the focus stays where it is.
    pub fn focus_item(&mut self, target: FocusTarget) {
        self.effects.focus_item = Some((self.node, target));
    }

    pub fn request_focus(&mut self) {
        self.effects.focus = Some(self.node);
    }

    /// Sends every pointer event to this node until it releases the pointer or the capture is revoked.
    pub fn capture_pointer(&mut self) {
        self.effects.capture = Some(CaptureRequest::Capture(self.node));
    }

    pub fn release_pointer(&mut self) {
        if self.has_pointer_capture() {
            self.effects.capture = Some(CaptureRequest::Release);
        }
    }

    pub fn request_paint(&mut self) {
        self.effects.changes.push((self.node, Change::Paint));
    }

    pub fn request_layout(&mut self) {
        self.effects.changes.push((self.node, Change::Layout));
    }

    /// Draws the window's overlays ([`Widget::overlay`](crate::Widget::overlay)) again on the next frame without
    /// painting any node: what changes only there, such as a caret's blink.
    pub fn request_overlay(&mut self) {
        self.effects.overlay = true;
    }

    /// `None` hides the pointer.
    pub fn set_cursor(&mut self, cursor: Option<CursorShape>) {
        self.effects.cursor = Some(cursor);
    }

    /// Turns the input method on for this node with `state`, or off with `None`. The runtime turns it off when the
    /// node loses the focus.
    pub fn set_text_input(&mut self, state: Option<TextInputState>) {
        self.effects.text_input = Some((self.node, state));
    }

    /// Scrolls the scroll containers around this node so `rect`, in its coordinates, shows after the next layout, as an
    /// editor shows its caret.
    pub fn reveal(&mut self, rect: Rect) {
        self.effects.reveal = Some((self.node, rect));
    }

    /// Puts `text` on the clipboard, as a field's copy or cut does.
    pub fn set_clipboard(&mut self, text: String) {
        self.effects.clipboard = Some(text);
    }

    /// Starts an interactive move of the window, in answer to the pointer press being handled: call it while that press
    /// is held, from its press or a motion after it. The window follows the pointer until the button is released; the
    /// node then gets a pointer leave, which revokes its capture. A compositor that does not hold the press any more,
    /// or a platform without moves, leaves the window where it is, and so does a fullscreen window.
    pub fn move_window(&mut self) {
        self.effects.window.push(WindowGesture::Move);
    }

    /// Maximizes the window, or restores it when the window system last said it is maximized. A fullscreen window stays
    /// as it is.
    pub fn zoom_window(&mut self) {
        self.effects.window.push(WindowGesture::Zoom);
    }

    /// Delivers [`Event::AnimationFrame`](crate::Event::AnimationFrame) to this node on the next frame.
    pub fn request_animation_frame(&mut self) {
        self.effects.animate.insert(self.node);
    }

    /// Delivers [`Event::Wake`](crate::Event::Wake) to this node once the clock reaches `at`, in place of any wake it
    /// asked for before.
    pub fn wake_at(&mut self, at: Timestamp) {
        self.effects.wakes.push((self.node, at));
    }
}

pub struct PaintCx<'a> {
    pub(crate) node: NodeId,
    pub(crate) size: Size,
    /// What the node paints with: on glass, the theme of text on glass ([`PaintCx::glass`]).
    pub(crate) env: &'a Environment,
    /// The window's environment, for what the node paints over a material that is not glass.
    pub(crate) plain_env: &'a Environment,
    /// The environment of text on the glass the node would show, for what it paints once it asked for it.
    pub(crate) glass_env: &'a Environment,
    pub(crate) focused: bool,
    pub(crate) focus_visible: bool,
    pub(crate) window_active: bool,
    /// The node lies on glass shown by an ancestor, an earlier node that holds it, or the window's background.
    pub(crate) on_glass: bool,
    /// The window system shows the material of the window's [`WindowBackground::Surface`](crate::WindowBackground).
    pub(crate) backdrop: bool,
    pub(crate) now: Timestamp,
    pub(crate) ops: &'a mut Vec<PaintOp>,
    pub(crate) group: &'a mut Option<Group>,
    pub(crate) animate: &'a mut bool,
    pub(crate) images: &'a mut Vec<(ResourceId, ImageResource)>,
    pub(crate) visible: Rect,
    pub(crate) reads_visible: &'a mut bool,
    /// The window can show one more material.
    pub(crate) materials: bool,
    pub(crate) material: &'a mut Option<NodeMaterial>,
    /// The node called [`PaintCx::material`].
    pub(crate) asked_material: &'a mut bool,
    /// The node called [`PaintCx::overlay`].
    pub(crate) overlay: &'a mut bool,
    /// The node lies on a material ([`PaintCx::on_material`]).
    pub(crate) on_material: bool,
    /// The node asked for glass or a translucent surface's material under itself, which its subtree lies on.
    pub(crate) lays_material: &'a mut bool,
    /// The ops [`PaintCx::fill_pixels`] and [`PaintCx::clip_pixels`] pushed, by index.
    pub(crate) pixels: &'a mut Vec<usize>,
    /// How far raised nodes cover the node's edges ([`PaintCx::obscured`]).
    pub(crate) obscured: Insets,
    pub(crate) reads_obscured: &'a mut bool,
    /// The window system draws raised nodes in a layer of their own, with their glass ([`PaintCx::raised_layer`]).
    pub(crate) layer: bool,
    /// What nodes of the window told each other so far ([`PaintCx::page_scrolled`]).
    pub(crate) marks: crate::paint::WindowMarks,
    /// The appearance the node asked its subtree to draw in ([`PaintCx::pin_appearance`]).
    pub(crate) pin: &'a mut Option<bool>,
    pub(crate) reads_marks: &'a mut bool,
    pub(crate) set_page_scrolled: &'a mut Option<bool>,
    pub(crate) set_band_shown: &'a mut Option<bool>,
}

impl PaintCx<'_> {
    pub fn node(&self) -> NodeId {
        self.node
    }

    /// The node's size; painting is in its local coordinates, with the origin at its top-left corner.
    pub fn size(&self) -> Size {
        self.size
    }

    pub fn env(&self) -> &Environment {
        self.env
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn focus_visible(&self) -> bool {
        self.focus_visible
    }

    pub fn window_active(&self) -> bool {
        self.window_active
    }

    /// Whether the node lies on glass the window shows: an ancestor's, the glass of an earlier node that holds it
    /// whole, or the window's own glass background. A control that would draw glass of its own there draws none, so
    /// glass is never laid on glass.
    pub fn on_glass(&self) -> bool {
        self.on_glass
    }

    /// Whether the node lies on a material rather than on the window's plain content: glass ([`on_glass`]), the
    /// material of a surface that shows what is behind the window (a sidebar, a popover) an ancestor asked for, or such
    /// a window background. It holds whether the window shows the material or paints the surface's opaque fill in its
    /// place (without the desktop's effects, with Reduce Transparency), so a control keeps one look in one place. The
    /// window's and a form's own backgrounds drawn by the compositor are not materials here: what lies on them is the
    /// window's content. A control drawn one way on content and another on a material (a search field) reads it.
    ///
    /// [`on_glass`]: Self::on_glass
    pub fn on_material(&self) -> bool {
        self.on_material
    }

    /// Whether the window system shows the material of the window's
    /// [`WindowBackground::Surface`](crate::WindowBackground) and draws its edge. Without it the window paints the
    /// surface's opaque fill instead, and a panel draws its own edge over it.
    pub fn window_backdrop(&self) -> bool {
        self.backdrop
    }

    pub fn now(&self) -> Timestamp {
        self.now
    }

    pub fn push(&mut self, op: PaintOp) {
        self.ops.push(op);
    }

    /// The part of the node that shows inside the window and its scroll containers, in its coordinates. A node that
    /// asks paints again whenever that part changes, so it can paint only what shows, as an editor paints its visible
    /// lines.
    pub fn visible(&mut self) -> Rect {
        *self.reads_visible = true;
        self.visible
    }

    /// Whether the window system draws the window's raised nodes ([`Widget::raised`](crate::Widget::raised)) in a layer
    /// over the rest of it, with the glass they ask for between the two, so what lies under them shows through their
    /// glass (where the desktop draws materials; else the nodes paint their own fills there). Without it they draw in
    /// place, and what scrolls under them is clipped away.
    pub fn raised_layer(&self) -> bool {
        self.layer
    }

    /// A scroll view tells the window whether its page is scrolled under the window's raised nodes. A node that does
    /// not tell again when it paints has told nothing.
    pub fn set_page_scrolled(&mut self, scrolled: bool) {
        *self.set_page_scrolled = Some(scrolled);
    }

    /// Whether a scroll view of this window told its page is scrolled. The node paints again when that changes.
    pub fn page_scrolled(&mut self) -> bool {
        *self.reads_marks = true;
        self.marks.page_scrolled
    }

    /// A raised node tells the window whether it shows a band over the page, so scroll views leave their own veil
    /// out ([`OverlayCx::band_shown`]). A node that does not tell again when it paints has told nothing.
    pub fn set_band_shown(&mut self, shown: bool) {
        *self.set_band_shown = Some(shown);
    }

    /// How far the window's raised nodes ([`Widget::raised`](crate::Widget::raised)), such as a toolbar row over a page
    /// that scrolls under it, cover the node's edges: what the node draws there lies under them. Zero for a raised node
    /// itself and its subtree. The node paints again when it changes.
    pub fn obscured(&mut self) -> Insets {
        *self.reads_obscured = true;
        self.obscured
    }

    /// Delivers [`Event::AnimationFrame`](crate::Event::AnimationFrame) to this node on the next frame, for what
    /// moves by itself from its first paint, as an indeterminate progress bar does.
    pub fn request_animation_frame(&mut self) {
        *self.animate = true;
    }

    /// Draws this node's overlay ([`Widget::overlay`](crate::Widget::overlay)) on every frame until the node paints
    /// again without asking.
    pub fn overlay(&mut self) {
        *self.overlay = true;
    }

    /// Fills `rect` (node coordinates) with `color`, each edge on the nearest device pixel edge of the window wherever
    /// a frame draws the node, at least one device pixel each way, so no pixel at its edge blends with what lies under
    /// it: for a mark that meets the caret or text, such as a selection.
    pub fn fill_pixels(&mut self, rect: Rect, color: cupertino_ui_theme::Color) {
        self.pixels.push(self.ops.len());
        self.ops.push(PaintOp::Fill {
            shape: cupertino_ui_render::Shape::Rect(rect),
            paint: cupertino_ui_render::Paint::Solid(color),
        });
    }

    /// Clips what the node paints next, until a `PaintOp::PopClip`, to `rect` (node coordinates) widened to the device
    /// pixels of the window it touches wherever a frame draws the node, so a [`fill_pixels`](Self::fill_pixels) at its
    /// edge stays whole.
    pub fn clip_pixels(&mut self, rect: Rect) {
        self.pixels.push(self.ops.len());
        self.ops
            .push(PaintOp::PushClip(cupertino_ui_render::Shape::Rect(rect)));
    }

    /// Draws all of `image` into `dst` (node coordinates). `id` names these pixels for good: new pixels take a new id
    /// ([`next_resource_id`](crate::next_resource_id)).
    pub fn image(&mut self, id: ResourceId, image: &ImageResource, dst: Rect, sampling: Sampling) {
        self.ops.push(PaintOp::Image {
            image: ResourceRef {
                id,
                revision: image.revision,
            },
            src: Rect::new(0.0, 0.0, f64::from(image.width), f64::from(image.height)),
            dst,
            sampling,
        });
        if !self.images.iter().any(|(drawn, _)| *drawn == id) {
            self.images.push((id, image.clone()));
        }
    }

    /// Paints this node and its whole subtree as one layer, with `opacity` applied once and `effects` (a fade, or a
    /// shadow or blur of the subtree) on the result.
    pub fn set_group(&mut self, opacity: f32, effects: Vec<Effect>) {
        *self.group = Some(Group { opacity, effects });
    }

    /// Asks the compositor for the material of `surface` under this node: its whole size, cut by the clips around it,
    /// with its corners rounded by `radius` points, at `alpha` from 0 (hidden) to 1 (opaque). A group's opacity does
    /// not reach it, so a node that fades passes its opacity here too. Returns whether the window shows the material.
    /// It does not without the desktop's effects, with Reduce Transparency or Increase Contrast, or past
    /// [`MAX_MATERIALS`](crate::MAX_MATERIALS) in one window; the node then paints the surface's opaque fill
    /// ([`ThemeTokens::surface`](cupertino_ui_theme::ThemeTokens::surface)) itself. The material lies under everything
    /// the window paints, so a node over other content of its window paints its own fill instead.
    pub fn material(&mut self, surface: Surface, radius: f64, alpha: f64) -> bool {
        *self.asked_material = true;
        *self.lays_material |= translucent(surface);
        let name = self.env.theme.surface(surface).material;
        *self.material = name.filter(|_| self.materials).map(|name| NodeMaterial {
            name,
            radius: if radius.is_finite() {
                radius.max(0.0)
            } else {
                0.0
            },
            capsule: false,
            alpha: if alpha.is_finite() {
                alpha.clamp(0.0, 1.0)
            } else {
                1.0
            },
            // a glass surface is glass on its own
            glass: glass_style(name).map(|style| GlassRegion {
                style,
                container: 0,
                spacing: 0.0,
            }),
        });
        self.take_foreground();
        self.material.is_some()
    }

    /// Draws the node's subtree in the dark appearance (`dark`) or the light one, whatever the window's: content that
    /// always stands on a picture of one tone, such as a lock screen over its darkened wallpaper. The theme keeps the
    /// session's other display options (contrast, transparency, motion). The node itself paints in the appearance it
    /// was given. Without a way to make the theme of another appearance (a runtime that does not follow the session's
    /// appearance) the subtree keeps the window's.
    pub fn pin_appearance(&mut self, dark: bool) {
        *self.pin = Some(dark);
    }

    /// What the node paints after it asked for a material lies on it: text and symbols on glass take the colours of
    /// the glass's face, and over any other material the window's.
    fn take_foreground(&mut self) {
        match self.material {
            Some(NodeMaterial { glass: Some(_), .. }) => self.env = self.glass_env,
            Some(_) => self.env = self.plain_env,
            None => {}
        }
    }

    /// Makes this node glass: the compositor draws `glass` under its whole size, cut by the clips around it. Nodes of
    /// one window in the same container join into one shape ([`Glass::container`]). A group's opacity does not reach
    /// it, so a node that fades passes its opacity in [`Glass::alpha`]. Returns whether the window shows the glass: it
    /// does not without the desktop's effects or past [`MAX_MATERIALS`](crate::MAX_MATERIALS) in one window, and the
    /// node then paints a fill itself. Reduce Transparency and Increase Contrast keep the glass, with the compositor's
    /// recipes for them. The glass lies under everything the window paints.
    ///
    /// What the node paints after it asked, and its whole subtree, lies on the glass: [`PaintCx::env`] then has the
    /// theme of text on glass ([`ThemeTokens::on_glass`](cupertino_ui_theme::ThemeTokens::on_glass)), whose labels take
    /// the colour of the glass's face, light or dark on a small shape as the compositor measured it. A node reads its
    /// colours after this call. [`PaintCx::material`] with a glass surface does the same.
    pub fn glass(&mut self, glass: Glass) -> bool {
        *self.asked_material = true;
        *self.lays_material = true;
        let finite = |v: f64, otherwise: f64| if v.is_finite() { v } else { otherwise };
        let (name, style) = (glass.style.material(), glass.style);
        *self.material = self.materials.then(|| NodeMaterial {
            name,
            radius: match glass.corners {
                GlassCorners::Radius(r) => finite(r, 0.0).max(0.0),
                GlassCorners::Capsule => 0.0,
            },
            capsule: glass.corners == GlassCorners::Capsule,
            alpha: finite(glass.alpha, 1.0).clamp(0.0, 1.0),
            glass: Some(GlassRegion {
                style,
                container: glass.container,
                spacing: finite(glass.spacing, 0.0).max(0.0),
            }),
        });
        self.take_foreground();
        self.material.is_some()
    }
}

/// Whether `surface`'s material shows what lies behind the window, so what lies on it lies on a material
/// ([`PaintCx::on_material`]); the window's and a form's own backgrounds are the window's content.
pub(crate) fn translucent(surface: Surface) -> bool {
    !matches!(surface, Surface::WindowBackground | Surface::TextBackground)
}

/// The glass style a material name of the theme stands for.
pub(crate) fn glass_style(material: &str) -> Option<GlassStyle> {
    GlassStyle::of_material(material)
}

/// The corners of a glass node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GlassCorners {
    /// Corners of this radius in points.
    Radius(f64),
    /// Round ends: the radius is half the node's shorter side, whatever its size.
    Capsule,
}

/// What [`PaintCx::glass`] puts under a node: the style, the corners, the container it joins and its opacity.
///
/// ```
/// use cupertino_ui_core::{Glass, GlassCorners, GlassStyle};
///
/// // two capsule buttons of a toolbar that merge where they are less than 8 points apart
/// let button = Glass::regular().capsule().container(1, 8.0);
/// assert_eq!(button.corners, GlassCorners::Capsule);
/// // a clear panel on its own, fading in
/// let panel = Glass::clear().radius(16.0).alpha(0.5);
/// assert_eq!((panel.style, panel.container), (GlassStyle::Clear, 0));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glass {
    pub style: GlassStyle,
    pub corners: GlassCorners,
    /// Nodes of a window with the same nonzero container join into one shape; 0 keeps the node on its own.
    pub container: u32,
    /// How far the outlines of a container blend into each other, in points; 0 joins them with a hard edge.
    pub spacing: f64,
    /// From 0 (hidden) to 1.
    pub alpha: f64,
}

impl Glass {
    /// Regular glass with square corners, on its own, opaque.
    pub fn regular() -> Self {
        Self {
            style: GlassStyle::Regular,
            corners: GlassCorners::Radius(0.0),
            container: 0,
            spacing: 0.0,
            alpha: 1.0,
        }
    }

    /// Clear glass with square corners, on its own, opaque.
    pub fn clear() -> Self {
        Self {
            style: GlassStyle::Clear,
            ..Self::regular()
        }
    }

    /// The glass of a large floating panel with square corners, on its own, opaque: regular glass toned further
    /// towards the middle, with a soft drop shadow the compositor draws below it.
    pub fn large_panel() -> Self {
        Self {
            style: GlassStyle::LargePanel,
            ..Self::regular()
        }
    }

    /// The glass of a dense floating surface with square corners, on its own, opaque: regular glass over a mid grey
    /// veil.
    pub fn dense() -> Self {
        Self {
            style: GlassStyle::Dense,
            ..Self::regular()
        }
    }

    /// The glass of the tiles of a panel of modules with square corners, on its own, opaque: regular optics over a face
    /// that lets the scene through darker.
    pub fn module() -> Self {
        Self {
            style: GlassStyle::Module,
            ..Self::regular()
        }
    }

    /// Corners of `radius` points.
    pub fn radius(self, radius: f64) -> Self {
        Self {
            corners: GlassCorners::Radius(radius),
            ..self
        }
    }

    /// Round ends.
    pub fn capsule(self) -> Self {
        Self {
            corners: GlassCorners::Capsule,
            ..self
        }
    }

    /// Join container `id` (nonzero), blending into its other shapes over `spacing` points.
    pub fn container(self, id: u32, spacing: f64) -> Self {
        Self {
            container: id,
            spacing,
            ..self
        }
    }

    /// Draw at `alpha`, from 0 (hidden) to 1.
    pub fn alpha(self, alpha: f64) -> Self {
        Self { alpha, ..self }
    }
}

pub(crate) type KeyedRelation = (fn(NodeId) -> Relation, Key);

/// What a node declared about itself for accessibility and keyboard focus.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Semantics {
    pub(crate) role: Option<Role>,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) value: Option<AccessibleValue>,
    pub(crate) states: AccessibleStates,
    pub(crate) actions: Vec<Action>,
    pub(crate) relations: Vec<Relation>,
    pub(crate) keyed_relations: Vec<KeyedRelation>,
    pub(crate) focusable: bool,
    pub(crate) focus_scope: bool,
    pub(crate) initial_focus: bool,
    pub(crate) hidden: bool,
    pub(crate) protected: bool,
    pub(crate) live: Option<Live>,
    pub(crate) item_count: Option<usize>,
    pub(crate) level: Option<usize>,
    pub(crate) item_index: Option<usize>,
    pub(crate) column_index: Option<usize>,
    pub(crate) sort: Option<SortOrder>,
    pub(crate) roving_group: bool,
    pub(crate) roving_item: bool,
    pub(crate) focus_fallback: bool,
    pub(crate) keeps_editing: bool,
    pub(crate) dismissible: bool,
    pub(crate) composing: bool,
    pub(crate) bounds: Option<Rect>,
    pub(crate) content_description: Option<String>,
}

/// A node without a role is left out of the accessibility tree and its children take its place.
pub struct SemanticsCx<'a> {
    pub(crate) node: NodeId,
    pub(crate) size: Size,
    pub(crate) direction: LayoutDirection,
    pub(crate) can_scroll: bool,
    pub(crate) semantics: &'a mut Semantics,
}

impl SemanticsCx<'_> {
    pub fn node(&self) -> NodeId {
        self.node
    }

    /// The node's laid-out size; zero before its first layout.
    pub fn size(&self) -> Size {
        self.size
    }

    pub fn direction(&self) -> LayoutDirection {
        self.direction
    }

    /// Whether the node is a scroll container whose content passes its viewport as laid out; false before its first
    /// layout.
    pub fn can_scroll(&self) -> bool {
        self.can_scroll
    }

    pub fn role(&mut self, role: Role) {
        self.semantics.role = Some(role);
    }

    pub fn name(&mut self, name: impl Into<String>) {
        self.semantics.name = Some(name.into());
    }

    /// On a node without a role, describes its nearest ancestor in the tree instead, after that node's own
    /// description: a badge's "3 unread" on the row it sits in.
    pub fn description(&mut self, description: impl Into<String>) {
        self.semantics.description = Some(description.into());
    }

    pub fn value(&mut self, value: AccessibleValue) {
        self.semantics.value = Some(value);
    }

    pub fn states(&mut self, states: AccessibleStates) {
        self.semantics.states = states;
    }

    pub fn action(&mut self, action: Action) {
        self.semantics.actions.push(action);
    }

    pub fn relation(&mut self, relation: Relation) {
        self.semantics.relations.push(relation);
    }

    /// A relation to the nearest node keyed `key`: the child of that key under the first ancestor, from the parent
    /// up, that has one. It is dropped when no ancestor has one or that node is not in the accessibility tree.
    pub fn relation_to(&mut self, relation: fn(NodeId) -> Relation, key: impl Into<Key>) {
        self.semantics.keyed_relations.push((relation, key.into()));
    }

    /// Makes the node a Tab stop.
    pub fn focusable(&mut self) {
        self.semantics.focusable = true;
    }

    /// Keeps Tab inside this subtree while it holds the focus, and gives the focus back when it unmounts (dialogs,
    /// popovers, menus).
    pub fn focus_scope(&mut self) {
        self.semantics.focus_scope = true;
    }

    /// Makes the node, or its first Tab stop, the one its focus scope gives the focus to when it opens, in place of the
    /// scope's first Tab stop. The first such stop in reading order wins.
    pub fn initial_focus(&mut self) {
        self.semantics.initial_focus = true;
    }

    /// Leaves the node and everything under it out of the accessibility tree and the Tab order, such as the content
    /// behind a modal sheet.
    pub fn hidden(&mut self) {
        self.semantics.hidden = true;
    }

    /// A press on the node or under it leaves a focused text field editing, where a press elsewhere ends it: for
    /// content whose field is where the keys always go, such as a search panel's.
    pub fn keeps_editing(&mut self) {
        self.semantics.keeps_editing = true;
    }

    /// Makes the node a group whose items share one Tab stop and move the focus among themselves with
    /// [`EventCx::focus_item`]: a radio group, a segmented control, a tab list.
    pub fn roving_group(&mut self) {
        self.semantics.roving_group = true;
    }

    /// Makes the node an item of its nearest roving group. Only the item that is the group's Tab stop calls
    /// [`focusable`](Self::focusable).
    pub fn roving_item(&mut self) {
        self.semantics.roving_item = true;
    }

    /// Marks the node's value a secret: whatever value it sets reaches accessibility as
    /// [`AccessibleValue::Protected`].
    pub fn protected(&mut self) {
        self.semantics.protected = true;
    }

    /// Makes the node a live region: assistive technology announces its changes as they happen (a toast, a form
    /// error). A node without a role is left out of the tree, and so is never announced.
    pub fn live(&mut self, live: Live) {
        self.semantics.live = Some(live);
    }

    /// Tells assistive technology how many items a list, table or tree holds when it builds only some of them.
    pub fn item_count(&mut self, count: usize) {
        self.semantics.item_count = Some(count);
    }

    /// Tells assistive technology a tree item's depth, from 1 at the top level.
    pub fn level(&mut self, level: usize) {
        self.semantics.level = Some(level);
    }

    /// Tells assistive technology an item's place among all of its container's items, from 0.
    pub fn item_index(&mut self, index: usize) {
        self.semantics.item_index = Some(index);
    }

    /// Tells assistive technology a table cell's or column header's column, from 0.
    pub fn column_index(&mut self, index: usize) {
        self.semantics.column_index = Some(index);
    }

    /// Tells assistive technology that the table is sorted by this column header's column, and in which order.
    pub fn sort(&mut self, order: SortOrder) {
        self.semantics.sort = Some(order);
    }

    /// Makes the node a Tab stop only when no node under it is one, such as a scroll view around long text.
    pub fn focus_fallback(&mut self) {
        self.semantics.focus_fallback = true;
    }

    /// While declared, Escape comes to this node first, as [`Event::Dismiss`](crate::Event::Dismiss), before the focus
    /// path hears it: a tip that shows while the focus is elsewhere.
    pub fn dismissible(&mut self) {
        self.semantics.dismissible = true;
    }

    /// Marks text an input method or a dead key is still composing here; Escape then goes to this node first, to
    /// cancel it, and dismisses nothing.
    pub fn composing(&mut self) {
        self.semantics.composing = true;
    }

    /// Describes the first node with a role under this one, in reading order, after that node's own description: a
    /// tooltip's text for its anchor.
    pub fn describe_content(&mut self, description: impl Into<String>) {
        self.semantics.content_description = Some(description.into());
    }

    /// The part of the node assistive technology shows as the node, in node coordinates; the whole node by default.
    pub fn bounds(&mut self, bounds: Rect) {
        self.semantics.bounds = Some(bounds);
    }
}

/// What a node draws over everything it painted, on every frame it asked for ([`PaintCx::overlay`]), without painting
/// again: a text caret, which blinks and must sit on whole device pixels of the window wherever the node lies. Rects
/// are in the node's coordinates; the window places them on device pixel edges after layout, so the node's paint never
/// depends on where it lies.
pub struct OverlayCx<'a> {
    pub(crate) size: Size,
    pub(crate) env: &'a Environment,
    pub(crate) now: Timestamp,
    /// The node's top-left corner in the window.
    pub(crate) origin: Point,
    pub(crate) ops: &'a mut Vec<PaintOp>,
    /// What the overlay is clipped to, in window coordinates.
    pub(crate) clip: &'a mut Option<Rect>,
    /// How far raised nodes cover the node's edges.
    pub(crate) obscured: Insets,
    /// The overlay draws under raised nodes too ([`OverlayCx::reach_obscured`]).
    pub(crate) reach_obscured: &'a mut bool,
    /// Whether a raised node of the window shows a band over the page ([`PaintCx::set_band_shown`]).
    pub(crate) band_shown: bool,
    pub(crate) reads_band: &'a mut bool,
}

impl OverlayCx<'_> {
    /// The node's size.
    pub fn size(&self) -> Size {
        self.size
    }

    pub fn env(&self) -> &Environment {
        self.env
    }

    pub fn now(&self) -> Timestamp {
        self.now
    }

    /// Fills `rect` with `color`, each edge moved to the nearest device pixel edge of the window and the rect at least
    /// one device pixel each way, so every pixel is covered whole or not at all.
    pub fn fill_pixels(&mut self, rect: Rect, color: cupertino_ui_theme::Color) {
        let at = rect.translate(self.origin.x, self.origin.y);
        self.ops.push(PaintOp::Fill {
            shape: cupertino_ui_render::Shape::Rect(pixel_fill(at, self.env.scale)),
            paint: cupertino_ui_render::Paint::Solid(color),
        });
    }

    /// How far the window's raised nodes cover the node's edges ([`PaintCx::obscured`]).
    pub fn obscured(&self) -> Insets {
        self.obscured
    }

    /// Whether a raised node of the window shows a band over the page. The overlay draws again when that changes.
    pub fn band_shown(&mut self) -> bool {
        *self.reads_band = true;
        self.band_shown
    }

    /// An overlay is cut from the part of the window raised nodes cover, as a caret is under a toolbar row; this one
    /// draws there too, as the edge of a page that scrolls under the row does.
    pub fn reach_obscured(&mut self) {
        *self.reach_obscured = true;
    }

    /// Fills `rect` (node coordinates) with `paint`, a gradient's points in node coordinates too.
    pub fn fill(&mut self, rect: Rect, paint: cupertino_ui_render::Paint) {
        use cupertino_ui_render::Paint;
        let (dx, dy) = (self.origin.x, self.origin.y);
        let moved = |p: Point| Point::new(p.x + dx, p.y + dy);
        let paint = match paint {
            Paint::Linear { start, end, stops } => Paint::Linear {
                start: moved(start),
                end: moved(end),
                stops,
            },
            Paint::Radial {
                center,
                radius,
                stops,
            } => Paint::Radial {
                center: moved(center),
                radius,
                stops,
            },
            solid => solid,
        };
        self.ops.push(PaintOp::Fill {
            shape: cupertino_ui_render::Shape::Rect(rect.translate(dx, dy)),
            paint,
        });
    }

    /// Clips the whole overlay to `rect`, widened to the device pixels it touches so a fill at its edge stays whole;
    /// the clips around the node apply as well.
    pub fn clip(&mut self, rect: Rect) {
        let at = rect.translate(self.origin.x, self.origin.y);
        *self.clip = Some(pixel_clip(at, self.env.scale));
    }
}

/// Physical pixels per point, a usable number.
pub(crate) fn usable_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// `rect` (window points) with each edge on the nearest device pixel edge at `scale`, at least one device pixel each
/// way, so every pixel is covered whole or not at all.
pub(crate) fn pixel_fill(rect: Rect, scale: f64) -> Rect {
    let scale = usable_scale(scale);
    // in device pixels, so the size is a whole number of them exactly
    let pixel = |v: f64| (v * scale).round();
    let (left, top) = (pixel(rect.min_x()), pixel(rect.min_y()));
    let width = (pixel(rect.max_x()) - left).max(1.0);
    let height = (pixel(rect.max_y()) - top).max(1.0);
    Rect::new(left / scale, top / scale, width / scale, height / scale)
}

/// `rect` (window points) widened to the device pixels at `scale` it touches.
pub(crate) fn pixel_clip(rect: Rect, scale: f64) -> Rect {
    let scale = usable_scale(scale);
    Rect::from_ltrb(
        (rect.min_x() * scale).floor() / scale,
        (rect.min_y() * scale).floor() / scale,
        (rect.max_x() * scale).ceil() / scale,
        (rect.max_y() * scale).ceil() / scale,
    )
}
