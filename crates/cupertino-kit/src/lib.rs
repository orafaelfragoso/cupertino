//! CupertinoKit, the Cupertino UI toolkit. Applications depend on this crate alone; each module lists the toolkit's
//! items an application names, one by one, and [`prelude`] has what a typical application names.

pub mod bus;
pub mod file_icons;
mod icons;
mod instance;
pub mod motion;
mod start;
pub mod task;
#[cfg(feature = "test")]
pub mod test;
pub mod time;

pub use instance::{spawn, Entered, Instance};

/// Running an application on the session's Wayland display.
pub mod app {
    pub use crate::start::{mono_family, run, AppError};

    /// Running on a window system a desktop shell set up with its surface roles, and the runtime's options of an
    /// application on this system: named until their last callers start through the shell's toolkit.
    pub use crate::start::{
        options, run_lock_screen_on_with, run_on, run_on_with, run_on_with_appearance,
    };
}

/// The application, its windows and commands, and the widget: what a widget is told, and what it states for
/// assistive technology, drag and drop, glass and icons.
pub mod core {
    pub use cupertino_ui_core::{
        glass_outline, next_resource_id, session_scroll_bars, AccessibleStates, AccessibleValue,
        Action, ActionData, Application, Change, Checked, Command, CommandRegistry, Drag,
        DragPreview, DropOver, DropPlace, DropTarget, Dropped, Element, Environment, Event,
        EventCx, FocusTarget, Glass, GlassStyle, IconImage, Icons, IdleCx, Key, Live, MeasureCx,
        Menus, PaintCx, Phase, Popup, PopupAnchor, Propagation, Relation, RendererClass, Role,
        SemanticsCx, Shortcut, SurfaceLayerMark, Widget, Window, WindowBackground, WindowKey,
        MAX_MATERIALS,
    };

    /// The runtime's options, which [`crate::app::run_on_with`] and [`crate::app::run_lock_screen_on_with`] hand a
    /// program's start and [`crate::app::options`] returns: named until those functions' last callers start
    /// through the shell's toolkit.
    pub use cupertino_ui_core::RuntimeOptions;
}

/// Values of windows and input, and the changes the compositor runs on a window's surfaces.
pub mod platform {
    pub use cupertino_ui_core::platform::{
        ClipboardData, CursorShape, DecorationMode, DragProgress, DropAction, DropToken,
        ForeignParent, GlassStyle, KeyEvent, Keysym, LayerChange, LayerEdge, LayerProperty, Mods,
        MouseButton, PointerEvent, PresentChange, PresentProperty, ScrollBars, ScrollSource,
        SurfaceLayerLook, SystemAppearance, Timestamp, WindowId, WindowRequest, WindowSpec,
        MAX_LAYER_SAMPLES,
    };

    /// The window system's interface, which [`crate::app::run_on`] and the functions beside it run on: named until
    /// their last callers start through the shell's toolkit.
    pub use cupertino_ui_core::platform::WindowSystem;
}

/// How a widget's children are laid out.
pub mod layout {
    pub use cupertino_ui_core::layout::{
        Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutDirection, LayoutSpec,
        Measured, Overflow, ScrollState, SizeSpec, Track,
    };
}

/// What a picture is made of.
pub mod render {
    pub use cupertino_ui_render::{
        Effect, FillRule, GradientStop, ImageResource, Paint, PaintOp, PathVerb, PixelFormat,
        ResourceId, Sampling, Shape, Transform,
    };

    /// The renderers and what they draw, which the control panel, the dock and the login screen draw pictures of
    /// their own with: named until those programs draw their pictures as paint operations Kit draws.
    pub use cupertino_ui_render::{
        CpuConfig, CpuRenderer, DamageRegion, DisplayList, PaintNode, PaintNodeId, PaintResources,
        PaintRevision, RenderRequest, UiRenderer,
    };
}

/// Text: the text system, styled text and its layout, and editing.
pub mod text {
    pub use cupertino_ui_text::{
        AttributedText, ContentPurpose, FontFeature, FontStyle, FontWeight, KeyInput, Keymap,
        ParagraphLayout, ParagraphStyle, Response, SecureBuffer, TextAlign, TextBuffer,
        TextFieldKeys, TextRange, TextStyle, TextSystem, Wrap,
    };

    /// The glyph rasters, which the control panel, the dock and the login screen draw text of their own with: named
    /// until those programs draw their pictures as paint operations Kit draws.
    pub use cupertino_ui_text::{
        GlyphFormat, GlyphKey, GlyphRaster, GlyphRasterizer, RasterGlyph, TextError,
    };

    /// The text editing key bindings.
    pub mod binding_file {
        pub use cupertino_ui_text::binding_file::load_default;
    }
}

/// The theme a widget is painted with.
pub mod theme {
    pub use cupertino_theme::ui::{
        Appearance, Color, ColorRole, ColorTokens, ControlFamily, ControlMetrics, ControlSize,
        FocusRing, InteractionVariant, Motion, StateVariant, Surface, ThemePreferences,
        ThemeTokens, TypeRole, TypeScale, BAR_GLASS_MATERIAL, CARET_PHASE_MS, GLASS_MATERIAL,
    };
}

/// The controls, with their option types and layout constants.
pub mod controls {
    pub use cupertino_ui_controls::{
        caret_blinking, caret_opacity, paint_glass_edge, striped_element, unobscured_insets,
        AccountLook, AppIcon, AppIconRow, Badge, BarChart, BarState, BareField, BareSecureField,
        BatteryMeter, Button, ButtonRole, ButtonStyle, CalendarDate, CalloutPointer, Canvas,
        CaretBlink, ChartBar, ChartColor, CheckBox, ChoiceState, ColorWell, Column, ColumnWidth,
        ComboBox, CompactButton, ContextMenu, DatePart, DatePicker, DatePickerState, Dialog,
        DialogAction, DialogButton, DialogKind, DisclosureButton, DisclosureStyle, DividerState,
        EditorState, FieldState, FilePreview, FormBackground, FormGroup, FormHeader, FormRow,
        FrameClock, FullWidthSegmentedControl, GlassEdge, Grid, GroupWindow, Handler, HeaderToggle,
        Icon, IconDirection, IconTile, Image, ImageFit, ImageSource, InlinePopUpButton, ListState,
        ListView, Menu, MenuAnchor, MenuItem, MenuStyle, MenuTail, Modal, ModuleLayout,
        ModuleToggle, Navigation, NavigationRow, NavigationStack, OutlineTable, PageDots,
        PaneHeader, PopUpButton, Popover, Press, PressCatcher, PreviewContent, PreviewStyle,
        Progress, PullDownButton, RadioGroup, RichText, RowKeyHandler, ScrollView, Scroller,
        SearchField, SecureField, SecureHandler, SecureState, SegmentedControl, Selection,
        Separator, Sidebar, SidebarAccessory, SidebarItem, SidebarLook, SidebarSection,
        SidebarWindow, Slider, SliderState, SliderStyle, SmallToggle, SortOrder, Spacer,
        SplitLayout, SplitPane, SplitView, Stack, Stepper, StepperState, Swatch, SwatchFill,
        SwatchLook, SwitchState, TableView, Tabs, Text, TextBox, TextBoxState, TextChange,
        TextEditor, TextField, TextLink, TextRole, ThumbnailTile, Tile, TileColor, TileSidebar,
        TileSidebarItem, Toggle, ToolbarButton, ToolbarGlyph, ToolbarGroup, ToolbarItem,
        ToolbarMenuButton, ToolbarPicker, ToolbarSearch, ToolbarTitle, Tooltip, TooltipChange,
        TooltipState, TreeItem, TreeToggle, TreeView, ViewportState, WindowDragArea, WindowGroup,
        Wrap, COMPACT_TOOLBAR_ROW, FORM_ICON_ROW_HEIGHT, FORM_ICON_TWO_LINE_ROW_HEIGHT,
        FORM_ROW_HEIGHT, FORM_SMALL_ICON_ROW_HEIGHT, FORM_TILE_ROW_HEIGHT, PANEL_WINDOW_BACKGROUND,
        SIDEBAR_PANEL_INSET, SIDEBAR_TOP, TILE_SIDEBAR_EDGE_FADE, TILE_SIDEBAR_ROW_HEIGHT,
        TOOLBAR_CHOICE_INSET, TOOLBAR_CHOICE_SEGMENT, TOOLBAR_CONTROL, TOOLBAR_DIVIDER,
        TOOLBAR_EDGE, TOOLBAR_GAP, TOOLBAR_MENU_BUTTON, TOOLBAR_ROW, TOOLBAR_SEGMENT,
        TOOLBAR_SYMBOL, TOOLBAR_TITLE_GAP, TOOLBAR_TITLE_SIZE, TOOLBAR_TITLE_WEIGHT,
        WINDOW_CONTENT_INSET, WINDOW_CORNER_RADIUS,
    };
}

/// The points, sizes, rectangles and insets Kit's signatures name.
pub mod geometry {
    pub use cupertino_geometry::{Insets, Point, Rect, Size};
}

pub mod prelude {
    pub use cupertino_ui_controls::{
        Button, ButtonRole, ButtonStyle, Icon, SecureField, Separator, Spacer, Stack, Text,
        TextChange, TextEditor, TextField, Toggle,
    };
    pub use cupertino_ui_core::layout::{Align, Axis};
    pub use cupertino_ui_core::platform::WindowSpec;
    pub use cupertino_ui_core::{
        Application, Command, Element, Glass, GlassStyle, Key, RendererClass, Window,
        WindowBackground, WindowKey,
    };
}

/// Type-to-select for the items of a menu or a pop-up list.
pub mod typeahead {
    pub use cupertino_ui_controls::typeahead::TypeAhead;
}

/// The model of a file chooser: a folder's entries, the highlighted one, navigation, filters and type-to-select.
pub mod filepicker {
    pub use cupertino_ui_controls::filepicker::{
        Entry, Key, Listing, Mode, Outcome, Picker, Request,
    };

    /// The folders of a chooser's sidebar: the home folder and the user directories of `user-dirs.dirs`.
    pub mod places {
        pub use cupertino_ui_controls::filepicker::places::{
            parse_user_dirs, session_places, Place,
        };
    }
}
