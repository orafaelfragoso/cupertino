//! The windows the model shows: the bar and its popups (the hovered tile's name, a folder's stack, the context menu),
//! the held tile, the edge strips, the trash's alerts, the Force Quit dialog and the switcher's panels.

use cupertino_kit::core::{Element, Key, Window, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_wm::geometry::{Point, Rect};

use super::{
    fan_basis, kit_point, message, tile_description, FanBasis, IconKey, Message, Model,
    FORCE_QUIT_ROW_ICON, REDUCED_BOUNCE_DIM,
};
use crate::dock::DockLayout;
use crate::fan::{self, FanLayout};
use crate::force_quit;
use crate::stack::{self, StackLayout, StackPlacement};
use crate::tiles::TileKind;
use crate::view::{self, IconSet, Input};

/// The sides the switcher's panel draws its icons at.
fn icon_sides(switcher: &crate::switcher_kit::SwitcherSurface) -> Vec<f64> {
    switcher.icon_side().into_iter().collect()
}

/// The switcher's panel's window on the output with key `output`: a key and an `app_id` (the layer surface's
/// namespace and output, `<namespace>@<output>`) of its own, and the compositor closing it reported as lost.
fn on_output(mut window: Window<Message>, output: u32) -> Window<Message> {
    if let Key::Str(name) = &window.key.0 {
        window.key = WindowKey(Key::from(format!("{name}@{output}")));
    }
    window.spec.app_id = format!("{}@{output}", window.spec.app_id);
    window.on_close = Some(message(Input::SwitcherLost(output)));
    window
}

/// What the held tile's window draws, as a number that changes whenever any of it does: its window repaints then, and
/// only then.
fn drag_revision(held: Option<&view::drag::Held>, puff: Option<&view::drag::Puff>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut rect = |r: cupertino_kit::geometry::Rect| {
        for v in [r.x(), r.y(), r.width(), r.height()] {
            v.to_bits().hash(&mut h);
        }
    };
    if let Some(held) = held {
        rect(held.rect);
        for v in [held.opacity, held.label, held.indicator] {
            v.to_bits().hash(&mut h);
        }
        for source in &held.icon {
            source.id.0.hash(&mut h);
        }
    }
    if let Some(puff) = puff {
        for v in [puff.centre.x, puff.centre.y, puff.size, puff.progress] {
            v.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

/// Every window the model shows.
pub fn view(model: &Model) -> Vec<Window<Message>> {
    let mut windows = Vec::new();
    let orientation = model.session.dock.cfg.orientation;
    for out in &model.outputs {
        let strip = Some(out.key) != model.output && Some(out.key) != model.left;
        if strip && !model.locked {
            windows.push(view::edge::window(
                out.key,
                Size::new(out.width, out.height),
                orientation,
                message,
            ));
        }
    }
    if !model.handoff {
        bar_windows(model, &mut windows);
    }
    if let (Some(alert), Some(output)) = (&model.confirm, model.output) {
        windows.push(view::confirm::window(alert, output, message));
    }
    windows.extend(force_quit_window(model));
    let now = model.now();
    for (output, switcher) in model.switchers() {
        if model.output_info(output).is_none() {
            continue;
        }
        let icon = |app: &str| model.icon(&IconKey::App(app.to_owned()), &icon_sides(switcher));
        let panel = switcher.window_with_frames(
            now,
            &icon,
            move |input| message(Input::Switcher(output, input)),
            message(Input::Frame),
        );
        windows.extend(panel.map(|w| on_output(w, output)));
    }
    windows
}

/// The tiles of `layout` as the bar shows them.
fn tile_views(model: &Model, layout: &DockLayout, now: f64) -> Vec<view::bar::TileView> {
    let orientation = model.session.dock.cfg.orientation;
    let sides = model.drawn_sides();
    layout
        .tiles
        .iter()
        .filter(|t| t.presence > 0.01)
        .map(|t| view::bar::TileView {
            id: t.id.0,
            kind: t.kind,
            rect: view::kit_rect(&t.visible_rect(orientation)),
            rest: view::kit_rect(&t.rect),
            // under Reduce Motion a bounce dims the tile in place instead of lifting it
            opacity: t.presence * (1.0 - REDUCED_BOUNCE_DIM * t.pulse),
            indicator: if t.indicator { t.indicator_alpha } else { 0.0 },
            windowless: t.indicator && model.windowless(t.id),
            chevron: model
                .stack
                .as_ref()
                .is_some_and(|s| s.tile == t.id && s.entries.is_some()),
            icon: match model.pressed_icon(t.id, now) {
                Some(dark) => dark.clone(),
                None => model
                    .tile_icon(t.id)
                    .map(|k| model.icon(&k, &sides))
                    .unwrap_or_default(),
            },
            name: model.tile_name(t.id),
            badge: model.badge(t.id).map(crate::badge::text),
            picture: model
                .session
                .dock
                .tile(t.id)
                .filter(|tile| tile.kind == TileKind::MinimizedWindow && tile.window.is_some())
                .map(|_| view::bar::picture_layer(t.id.0)),
            description: tile_description(model, t.id, t.lifecycle),
        })
        .collect()
}

/// Where the popups over the bar stand, in the bar's window: from the bar's top (or a tile's middle) down to the
/// bottom of the window, so that a popup, which has no room below, stands above them.
struct Anchors {
    label: Option<cupertino_kit::geometry::Rect>,
    fan: Option<cupertino_kit::geometry::Rect>,
    menu: Option<cupertino_kit::geometry::Rect>,
}

/// The bar's window and its popups, and the held tile's window.
fn bar_windows(model: &Model, windows: &mut Vec<Window<Message>>) {
    let (Some(output), Some(screen), Some(key)) = (model.output, model.screen(), model.bar_key())
    else {
        return;
    };
    let now = model.now();
    let orientation = model.session.dock.cfg.orientation;
    let layout = model.session.dock.layout(screen, now);
    let bar = layout.bar.intersection(&screen).map_or_else(
        || cupertino_kit::geometry::Rect::new(0.0, 0.0, 0.0, 0.0),
        |r| view::kit_rect(&r),
    );
    let input = model
        .session
        .dock
        .input_rects(screen, now)
        .iter()
        .map(view::kit_rect)
        .collect();
    let to_bottom = |x: f64, y: f64, width: f64| {
        let height = screen.bottom() - y;
        (height > 0.0).then(|| cupertino_kit::geometry::Rect::new(x, y, width, height))
    };
    // the name fades as the bar hides, and one that showed as the pointer left stays while the bar goes down
    let hidden = if model.session.dock.cfg.autohide {
        model.session.dock.autohide.hidden_fraction(now)
    } else {
        0.0
    };
    let label = model.label(&layout).or_else(|| {
        model
            .leaving_label
            .clone()
            .filter(|(id, ..)| hidden < 1.0 && layout.tiles.iter().any(|t| t.id == *id))
    });
    let fan_basis = model
        .stack
        .as_ref()
        .filter(|s| s.shape == fan::Shape::Fan && s.entries.is_some())
        .and_then(|s| fan_basis(&layout, screen, s.tile));
    let grid = grid_placement(model, &layout, screen);
    let anchors = Anchors {
        // the name rides its tile's bounce, and stands over a tile that is higher than the bar
        label: label.as_ref().and_then(|(id, ..)| {
            let t = layout.tiles.iter().find(|t| t.id == *id)?;
            let top = view::label::stands_on(layout.bar.y, t.rect.y, t.lift);
            to_bottom(t.rect.x, top, t.rect.w)
        }),
        fan: fan_basis
            .as_ref()
            .and_then(|b| to_bottom(b.tile_cx - 0.5, b.tile_cy, 1.0)),
        menu: model.menu.as_ref().and_then(|m| {
            let t = layout.tiles.iter().find(|t| t.id == m.tile)?;
            to_bottom(t.rect.x + t.rect.w / 2.0 - 0.5, layout.bar.y, 1.0)
        }),
    };
    let mut bar_window = view::bar::window(
        view::bar::BarView {
            key: key.clone(),
            output,
            size: Size::new(screen.w, screen.h),
            orientation,
            tile_size: model.session.dock.cfg.tile_size,
            bar,
            tiles: tile_views(model, &layout, now),
            label_anchor: anchors.label,
            fan_anchor: anchors.fan,
            stack_anchor: grid.map(|g| view::kit_rect(&g.anchor)),
            menu_anchor: anchors.menu,
            input,
            initial: model.tile_order.first().map(|t| t.0),
            ticking: model.bar_moving(now),
            surface_changes: model.surface_changes,
        },
        message,
    );
    bar_window.content = app_drops(bar_window.content);
    windows.push(bar_window);
    if let Some((id, name, status)) = label {
        windows.push(view::label::window(
            view::label::LabelView {
                parent: key.clone(),
                tile: id.0,
                name: &name,
                status,
                orientation,
                opacity: 1.0 - hidden,
            },
            message,
        ));
    }
    windows.extend(stack_window(model, &key, fan_basis.as_ref(), grid));
    if let Some(menu) = &model.menu {
        if let Some(anchor) = layout.tiles.iter().find(|t| t.id == menu.tile) {
            windows.extend(view::menu::windows(
                view::menu::MenuView {
                    parent: key.clone(),
                    items: &menu.items,
                    label: menu.label.clone(),
                    tile: menu.tile.0,
                    anchor: view::kit_rect(&anchor.visible_rect(orientation)),
                    orientation,
                },
                message,
            ));
        }
    }
    windows.extend(drag_window(model, &layout, output));
}

/// Where the grid of an open stack's popup stands by its tile, as the bar is laid out now.
fn grid_placement(model: &Model, layout: &DockLayout, screen: Rect) -> Option<StackPlacement> {
    let orientation = model.session.dock.cfg.orientation;
    let s = model
        .stack
        .as_ref()
        .filter(|s| s.shape == fan::Shape::Grid)?;
    let entries = s.entries.as_ref()?;
    let t = layout.tiles.iter().find(|t| t.id == s.tile)?;
    let panel = StackLayout::new(entries.len());
    Some(StackPlacement::new(
        orientation,
        (panel.width, panel.height),
        layout.bar,
        t.visible_rect(orientation),
        screen,
    ))
}

/// The open stack's popup of the bar `key`: its fan over `basis`, or its grid at `grid`.
fn stack_window(
    model: &Model,
    key: &WindowKey,
    basis: Option<&FanBasis>,
    grid: Option<StackPlacement>,
) -> Option<Window<Message>> {
    let now = model.now();
    let open = model.stack.as_ref()?;
    let entries = open.entries.as_ref()?;
    if let (fan::Shape::Fan, Some(basis)) = (open.shape, basis) {
        let lookup =
            |name: &str| -> IconSet { model.icon(&IconKey::Named(name.to_owned()), &[fan::ICON]) };
        let closing = open.closing.map(|c| (c.start, c.from));
        return Some(view::fan::window(
            view::fan::FanView {
                parent: key.clone(),
                name: &open.name,
                entries,
                icons: &lookup,
                layout: FanLayout::new(&fan::names(entries), stack::OPEN_FOLDER, basis.lowest),
                progress: fan::progress(now, open.opened, closing, model.reduce_motion),
                highlighted: open.hover.or(open.selected),
                moving: fan::moving(now, open.opened, open.closing.map(|c| c.start)),
            },
            message,
        ));
    }
    let placement = grid?;
    let lookup = |name: &str| -> IconSet {
        model.icon(&IconKey::Named(name.to_owned()), &[StackLayout::ICON])
    };
    Some(view::stack::window(
        view::stack::StackView {
            parent: key.clone(),
            placement,
            orientation: model.session.dock.cfg.orientation,
            name: &open.name,
            entries,
            icons: &lookup,
            since: now - open.opened,
            reduce_motion: model.reduce_motion,
        },
        message,
    ))
}

/// The window over the whole output of the bar that draws the tile the pointer holds, and the removal puff.
fn drag_window(model: &Model, layout: &DockLayout, output: u32) -> Option<Window<Message>> {
    let now = model.now();
    let poof = model
        .poof
        .and_then(|p| p.progress(now).map(|progress| (p.centre, progress)));
    if layout.dragging.is_none() && poof.is_none() {
        return None;
    }
    let (out, in_bar) = (model.output_info(output)?, model.output_in_bar()?);
    let sides = model.drawn_sides();
    // the bar's window sits at (-x, -y) of the output
    let (ox, oy) = (-in_bar.x, -in_bar.y);
    let held = layout.dragging.as_ref().map(|d| view::drag::Held {
        rect: view::kit_rect(&d.tile.rect.translate(ox, oy)),
        icon: match d.tile.kind {
            TileKind::Trash => model.icon(&IconKey::Trash(model.trash_full), &sides),
            _ => model.icon(&IconKey::App(d.key.clone()), &sides),
        },
        opacity: d.opacity,
        label: d.label,
        indicator: d.indicator,
    });
    let puff = poof.map(|(centre, progress)| view::drag::Puff {
        centre: kit_point(Point::new(centre.x + ox, centre.y + oy)),
        size: model.session.dock.cfg.tile_size,
        progress,
    });
    Some(view::drag::window(
        view::drag::DragView {
            output,
            size: Size::new(out.width, out.height),
            orientation: model.session.dock.cfg.orientation,
            revision: drag_revision(held.as_ref(), puff.as_ref()),
            held,
            puff,
            ticking: false,
        },
        message,
    ))
}

/// The Force Quit dialog's window, while it is open.
fn force_quit_window(model: &Model) -> Option<Window<Message>> {
    let open = model.force_quit.as_ref()?;
    let set = |key: IconKey, sides: &[f64]| model.icon(&key, sides);
    let rows = match &open.layout {
        force_quit::Layout::List(list) => list
            .rows
            .iter()
            .filter_map(|&(i, rect)| {
                let e = open.dialog.entries().get(i)?;
                Some(view::force_quit::RowView {
                    index: i,
                    rect: view::kit_rect(&rect),
                    name: e.name.clone(),
                    unresponsive: e.unresponsive,
                    selected: open.dialog.is_selected(i),
                    hovered: open.hover_row == Some(i),
                    icon: set(IconKey::App(e.app_id.clone()), &[FORCE_QUIT_ROW_ICON]),
                })
            })
            .collect(),
        force_quit::Layout::Question(_) => Vec::new(),
    };
    let confirming = open.dialog.confirming();
    let question = confirming
        .as_ref()
        .map_or_else(String::new, |entries| force_quit::question(entries));
    let question_icon = confirming
        .as_ref()
        .and_then(|entries| entries.first())
        .map(|e| set(IconKey::App(e.app_id.clone()), &[force_quit::ICON_SIZE]))
        .unwrap_or_default();
    Some(view::force_quit::window(
        view::force_quit::ForceQuitView {
            output: open.output,
            layout: open.layout.clone(),
            rows,
            entries: open.dialog.entries().len(),
            empty: open.dialog.entries().is_empty(),
            can_force_quit: open.dialog.can_force_quit(),
            question,
            question_icon,
            hover_button: open.hover_button,
        },
        message,
    ))
}

/// The type an application is dropped on the bar in: a list of `file://` URIs, its desktop file among them.
const URI_LIST: &str = "text/uri-list";

/// The bar's `content` taking drops of applications' desktop files ([`Message::Dropped`]).
fn app_drops(content: Element<Message>) -> Element<Message> {
    use cupertino_kit::core::{DropPlace, DropTarget};
    use cupertino_kit::platform::DropAction;
    Element::from(DropTarget {
        content,
        mimes: vec![URI_LIST.into()],
        actions: [DropAction::Copy].into(),
        place: Box::new(|over| {
            over.actions
                .contains(&DropAction::Copy)
                .then_some(DropPlace {
                    key: None,
                    action: DropAction::Copy,
                })
        }),
        on_hover: None,
        on_drop: Box::new(Message::Dropped),
        on_leave: None,
    })
}

/// The key of the application whose desktop file is the first `file://` URI of a `text/uri-list` ending in
/// `.desktop`: the file's name without the suffix, as the dock keeps its pins. `None` when the list holds none.
pub fn dropped_app(list: &str) -> Option<String> {
    list.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.strip_prefix("file://"))
        .map(percent_decoded)
        .find_map(|path| {
            let name = path.rsplit('/').next()?;
            let key = name.strip_suffix(".desktop")?;
            (!key.is_empty()).then(|| key.to_owned())
        })
}

/// `text` with its `%XX` escapes decoded; an escape that is not one is kept as it is.
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let hex = |b: u8| (b as char).to_digit(16);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escape = match (bytes[i], bytes.get(i + 1), bytes.get(i + 2)) {
            (b'%', Some(&h), Some(&l)) => hex(h).zip(hex(l)),
            _ => None,
        };
        match escape {
            Some((h, l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
