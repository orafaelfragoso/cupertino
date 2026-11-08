//! The panel's windows as CupertinoKit draws them: the card of module toggles, tiles, sliders and the battery over the
//! menu material, faded as one layer with the material, and the transparent catcher under it that takes a press outside
//! the card.

use cupertino_kit::controls::{
    BatteryMeter, Canvas, Grid, ModuleLayout, ModuleToggle, Slider as LevelSlider, SliderStyle,
    Spacer, Stack, Text, TextRole, Tile,
};
use cupertino_kit::core::{
    Change, CommandRegistry, Element, Event, EventCx, Key, MeasureCx, PaintCx, Propagation,
    SemanticsCx, Shortcut, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Insets, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec, Track,
};
use cupertino_kit::platform::{
    DecorationMode, Keysym, Mods, MouseButton, PointerEvent, WindowSpec,
};
use cupertino_kit::text::{AttributedText, FontWeight, TextStyle};
use cupertino_kit::theme::Surface;

use crate::layout::{
    card_size, BATTERY_GAP, CARD_RADIUS, COLUMN, COMPACT_HEIGHT, FIRST_ROW_HEIGHT, GRID_ROWS,
    PADDING, ROW_HEIGHT, ROW_SPACING, SLIDER_BOTTOM_INSET, SLIDER_SIDE_INSET, SLIDER_TOP_INSET,
    SPACING,
};
use crate::model::{speaker_icon, Battery, Controls, Module, Slider, TileState};

/// The `app_id` of the card's window, which gives it its layer surface.
pub const APP_ID: &str = "cupertino-controlcenter";
/// The `app_id` of the catcher's window.
pub const CATCHER_APP_ID: &str = "cupertino-controlcenter.catcher";
/// What assistive technology calls the card.
pub const TITLE: &str = "Control Center";

const ESCAPE: Keysym = Keysym(0xff1b);
/// Title font of a slider tile: system 12 medium.
const TITLE_SIZE: f32 = 12.0;
const TITLE_WEIGHT: u16 = 500;

/// What the user did on the panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    /// A module's switch was pressed.
    Press(Module),
    /// A slider moved to a fraction, 0 to 1.
    Slide(Slider, f64),
    /// The slider's drag or key step ended at a fraction.
    Release(Slider, f64),
    /// Escape, or a press outside the card.
    Dismiss,
    /// The server closed the window: its output went away.
    Closed,
}

/// The symbol of a module's switch or slider.
fn symbol(module: Module) -> &'static str {
    match module {
        Module::WiFi => "symbol.wifi",
        Module::Bluetooth => "symbol.bluetooth",
        Module::FileSharing => "symbol.file-sharing",
        Module::FocusMode => "symbol.moon",
        Module::Stage => "symbol.stage",
        Module::ScreenMirroring => "symbol.screen-mirroring",
        Module::Display => "symbol.sun",
        Module::Sound => "symbol.speaker.high",
        Module::NowPlaying => "symbol.music",
    }
}

fn switch<M: 'static>(
    controls: &Controls,
    module: Module,
    layout: ModuleLayout,
    to: fn(Input) -> M,
) -> Element<M> {
    let state = controls.tile(module);
    Element::new(ModuleToggle {
        label: module.title().to_owned(),
        caption: controls.subtitle(module).to_owned(),
        symbol: symbol(module).to_owned(),
        layout,
        on: state == TileState::On,
        enabled: !matches!(state, TileState::Disabled(_)),
        on_toggle: Some(Box::new(move |_| to(Input::Press(module)))),
    })
}

/// A 40 point switch row in a 62 point tile, inset as far from the leading and trailing edges as from the top.
fn row_tile<M: 'static>(controls: &Controls, module: Module, to: fn(Input) -> M) -> Element<M> {
    let inset = (COMPACT_HEIGHT - ROW_HEIGHT) / 2.0;
    Element::from(Tile {
        label: module.title().to_owned(),
        content: Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::uniform(inset),
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![switch(controls, module, ModuleLayout::Row, to)],
        }),
    })
}

fn compact_tile<M: 'static>(controls: &Controls, module: Module, to: fn(Input) -> M) -> Element<M> {
    Element::from(Tile {
        label: module.title().to_owned(),
        content: switch(controls, module, ModuleLayout::Compact, to),
    })
    .width(COMPACT_HEIGHT)
}

/// The display or sound tile: its title over a level slider with the module's symbol in the track.
fn slider_tile<M: 'static>(controls: &Controls, slider: Slider, to: fn(Input) -> M) -> Element<M> {
    let module = slider.module();
    let fraction = controls.fraction(slider);
    let symbol = match slider {
        Slider::Display => symbol(Module::Display),
        Slider::Sound => controls
            .volume
            .ready()
            .map_or("symbol.speaker.slash", speaker_icon),
    };
    let title = Element::new(Text {
        content: AttributedText::plain(
            module.title(),
            TextStyle {
                weight: FontWeight(TITLE_WEIGHT),
                ..TextStyle::new(TITLE_SIZE)
            },
        ),
        role: TextRole::Body,
        wrap: false,
        secondary: false,
        selected: false,
    });
    Element::from(Tile {
        label: module.title().to_owned(),
        content: Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::new(
                SLIDER_TOP_INSET,
                SLIDER_SIDE_INSET,
                SLIDER_BOTTOM_INSET,
                SLIDER_SIDE_INSET,
            ),
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![
                title,
                Element::new(Spacer { min: 0.0 }),
                Element::new(LevelSlider {
                    label: module.title().to_owned(),
                    value: fraction.unwrap_or(0.0),
                    min: 0.0,
                    max: 1.0,
                    step: None,
                    style: SliderStyle::PanelLevel(symbol),
                    ticks: 0,
                    enabled: fraction.is_some(),
                    on_change: Some(Box::new(move |v| to(Input::Slide(slider, v)))),
                    on_end: Some(Box::new(move |v| to(Input::Release(slider, v)))),
                })
                .key(module.title()),
            ],
        })
        .fill(),
    })
}

fn battery_row<M: 'static>(battery: Battery) -> Element<M> {
    Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets::ZERO,
        spacing: BATTERY_GAP,
        align: Align::Center,
        children: vec![
            Element::new(BatteryMeter {
                label: "Battery".to_owned(),
                level: battery.level(),
                charging: battery.charging,
            }),
            Element::new(Text {
                content: AttributedText::plain(
                    format!("{}%", battery.percent),
                    TextStyle::new(TITLE_SIZE),
                ),
                role: TextRole::Body,
                wrap: false,
                secondary: false,
                selected: false,
            }),
        ],
    })
    .span(2, 1)
}

/// The card's grid: the battery row when there is one, the connectivity rows beside the focus mode, stage and screen
/// mirroring tiles, then the display, sound and now playing tiles across both columns.
fn grid<M: 'static>(controls: &Controls, to: fn(Input) -> M) -> Element<M> {
    let mut rows = Vec::with_capacity(GRID_ROWS + 1);
    let mut children = Vec::with_capacity(8);
    if let Some(battery) = controls.battery {
        rows.push(Track::Fixed(FIRST_ROW_HEIGHT));
        children.push(battery_row(battery).key("battery"));
    }
    rows.extend([Track::Fixed(COMPACT_HEIGHT); GRID_ROWS]);
    children.extend([
        Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: ROW_SPACING,
            align: Align::Stretch,
            children: [Module::WiFi, Module::Bluetooth, Module::FileSharing]
                .map(|module| switch(controls, module, ModuleLayout::Row, to).key(module.title()))
                .into(),
        })
        .span(1, 2)
        .key("connectivity"),
        row_tile(controls, Module::FocusMode, to).key(Module::FocusMode.title()),
        Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets::ZERO,
            spacing: SPACING,
            align: Align::Stretch,
            children: [Module::Stage, Module::ScreenMirroring]
                .map(|module| compact_tile(controls, module, to).key(module.title()))
                .into(),
        })
        .key("compact"),
        slider_tile(controls, Slider::Display, to)
            .span(2, 1)
            .key("display"),
        slider_tile(controls, Slider::Sound, to)
            .span(2, 1)
            .key("sound"),
        row_tile(controls, Module::NowPlaying, to)
            .span(2, 1)
            .key(Module::NowPlaying.title()),
    ]);
    Element::new(Grid {
        columns: vec![Track::Fixed(COLUMN); 2],
        rows,
        padding: Insets::uniform(PADDING),
        spacing: Size::new(SPACING, SPACING),
        children,
    })
}

/// The card at `alpha` over the menu material: the controls, faded as one layer with the material. Escape dismisses
/// it.
pub fn panel<M: Clone + 'static>(controls: &Controls, alpha: f64, to: fn(Input) -> M) -> Window<M> {
    let size = card_size(controls.battery.is_some());
    // the layer surface keeps the size it opened at: a battery that comes or goes opens the card again
    let key = if controls.battery.is_some() {
        "panel-battery"
    } else {
        "panel"
    };
    Window {
        key: WindowKey(Key::from(key)),
        spec: WindowSpec {
            title: TITLE.to_owned(),
            app_id: APP_ID.to_owned(),
            size,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new().with(
            Shortcut {
                mods: Mods::NONE,
                keysym: ESCAPE,
            },
            to(Input::Dismiss),
        ),
        on_close: Some(to(Input::Closed)),
        background: WindowBackground::Surface {
            surface: Surface::Menu,
            radius: CARD_RADIUS,
            alpha,
        },
        content: Element::new(Canvas {
            size: Some(size),
            label: TITLE.to_owned(),
            paint: Box::new(move |cx| cx.set_group(alpha.clamp(0.0, 1.0) as f32, Vec::new())),
            children: vec![grid(controls, to)],
        }),
        popup: None,
        menus: None,
    }
}

/// Takes a left or right press anywhere on it and draws nothing.
struct Catcher<M> {
    on_press: M,
}

impl<M: Clone + 'static> Widget<M> for Catcher<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            // a left or right press outside the card dismisses it (global mouse monitor mask 0xa)
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left | MouseButton::Right,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    /// Not something assistive technology can reach: Escape dismisses the panel from the keyboard.
    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The transparent window under the card, over the whole output while the panel is shown.
pub fn catcher<M: Clone + 'static>(to: fn(Input) -> M) -> Window<M> {
    Window {
        key: WindowKey(Key::from("catcher")),
        spec: WindowSpec {
            title: TITLE.to_owned(),
            app_id: CATCHER_APP_ID.to_owned(),
            size: Size::ZERO,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: Some(to(Input::Closed)),
        background: WindowBackground::None,
        content: Element::new(Catcher {
            on_press: to(Input::Dismiss),
        }),
        popup: None,
        menus: None,
    }
}
