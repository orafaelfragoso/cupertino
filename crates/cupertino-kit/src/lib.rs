//! CupertinoKit, the Cupertino UI toolkit. Applications depend on this crate alone; each module lists the toolkit's
//! items an application names, one by one, and [`prelude`] has what a typical application names.

pub mod bus;
pub mod file_icons;
mod icons;
pub mod image;
mod instance;
pub mod motion;
mod picture;
mod start;
pub mod task;
#[cfg(feature = "test")]
pub mod test;
pub mod time;

pub use instance::{spawn, Entered, Instance};

/// Running an application on the session's Wayland display.
pub mod app {
    pub use crate::start::{environment, mono_family, run, AppError};

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

/// What a picture is made of, and pictures a program draws.
pub mod render {
    pub use crate::picture::picture;
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

/// The theme a widget is painted with, and the colours the theme data names.
pub mod theme {
    pub use crate::start::system_color;
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

/// The file family: a folder's items, their order, types, tags and names, the icon layout, opening, renaming and the
/// file operations, and the measured metrics of the file views.
pub mod files {
    pub use cupertino_ui_files::{ColumnEvent, ViewEvent};

    /// The virtual folder of the installed applications.
    pub mod applications {
        pub use cupertino_ui_files::applications::{
            any_installed, application_dirs, location, session_entries,
        };
    }

    /// A chain of folders side by side.
    pub mod columns {
        pub use cupertino_ui_files::columns::{
            column_key, FileColumn, FileColumns, ROW_HEIGHT, ROW_ICON,
        };
    }

    /// An item of a folder.
    pub mod entry {
        pub use cupertino_ui_files::entry::{Entry, Kind};
    }

    /// The direction a key moves the keyboard in a layout.
    pub mod geom {
        pub use cupertino_ui_files::geom::Direction;
    }

    /// The folder as a grid of icons.
    pub mod grid {
        pub use cupertino_ui_files::grid::{
            cell_index, cell_key, laid_out_name, name, Grab, IconGrid, ItemIcon, NameShape,
        };
    }

    /// The art of the common items.
    pub mod item_art {
        pub use cupertino_ui_files::item_art::{draw, Art};
    }

    /// The icon layout.
    pub mod layout {
        /// The icon view's layout.
        pub mod icon {
            pub use cupertino_ui_files::layout::icon::{
                layout, max_title_lines, strip, Flow, FontMetrics, IconCell, IconLayout,
                IconParams, LabelPrefs, SectionSpec,
            };
        }
    }

    /// The folder as an outline table.
    pub mod list {
        pub use cupertino_ui_files::list::{
            row_key, FileList, Presses, RowGrab, ROW_HEIGHT, ROW_ICON,
        };
    }

    /// The measured metrics of the file views.
    pub mod metrics {
        /// The icon view.
        pub mod icon_view {
            pub use cupertino_ui_files::metrics::icon_view::{
                GRID_SPACING_DEFAULT, GRID_SPACING_MAX, GRID_SPACING_MIN, ICON_SIZE_DEFAULT,
                ICON_SIZE_MAX, ICON_SIZE_MIN, TEXT_SIZE_DEFAULT, TEXT_SIZE_MAX, TEXT_SIZE_MIN,
            };
        }

        /// The list view.
        pub mod list_view {
            pub use cupertino_ui_files::metrics::list_view::{
                HEADER_HEIGHT, ICON_SIZE_DEFAULT, ICON_SIZE_MAX, ICON_SIZE_MIN, INDENT_FLAT,
                INDENT_PER_LEVEL, INTERCELL, INTERCELL_TEMPLATE, ROW_PADDING, TEXT_SIZE_DEFAULT,
                TEXT_SIZE_MAX, TEXT_SIZE_MIN,
            };
        }

        /// The column view.
        pub mod column_view {
            pub use cupertino_ui_files::metrics::column_view::{
                MIN_WIDTH, PREFERRED_WIDTH, PREVIEW_PANE_WIDTH,
            };
        }

        /// Timings.
        pub mod timing {
            pub use cupertino_ui_files::metrics::timing::{
                DOUBLE_CLICK_INTERVAL, TYPE_SELECT_MIN_TIMEOUT,
            };
        }

        /// The selection's look.
        pub mod selection {
            pub use cupertino_ui_files::metrics::selection::CONTEXT_RING;
        }

        /// The rubber band.
        pub mod marquee {
            pub use cupertino_ui_files::metrics::marquee::BORDER_WIDTH;
        }

        /// Dragging items.
        pub mod drag {
            pub use cupertino_ui_files::metrics::drag::START_DISTANCE;
        }
    }

    /// A folder's items as a sorted list that follows its changes, read by work the program runs.
    pub mod model {
        pub use cupertino_ui_files::model::{
            DirectoryModel, ModelDelta, ModelOptions, ModelWatch, ModelWork, Notify, DEFAULT_BATCH,
            WORK_DEADLINE,
        };
    }

    /// An item's type.
    pub mod mime {
        pub use cupertino_ui_files::mime::{entry_mime, kind_description, kind_group, KindGroup};
    }

    /// The name being edited.
    pub mod name_edit {
        pub use cupertino_ui_files::name_edit::NameEdit;
    }

    /// The order of names.
    pub mod natural {
        pub use cupertino_ui_files::natural::{compare_names, type_select_key, NameKey};
    }

    /// Default applications and the Open With list.
    pub mod open {
        pub use cupertino_ui_files::open::{
            application_at, command_with, default_app_id, open_command, open_with, OpenError,
        };
    }

    /// New folders, renames, aliases, copies and moves, as blocking functions.
    pub mod operations {
        pub use cupertino_ui_files::operations::{
            drop_action, drop_into, duplicate, duplicate_with, is_cancelled, make_alias,
            new_folder, rename_item, same_volume, stopped, transfer, Cancel, DropAction, Progress,
        };
    }

    /// The places of a file browser's sidebar and of a file chooser's.
    pub mod places {
        pub use cupertino_ui_files::places::{
            chooser_places, desktop_folder, folder_icons, follow_trash, recents_place,
            removable_holding, session_places, standard_places, window_title, Medium, Place,
            PlaceGroup, PlaceKind, SessionPlaces, Volume, APPLICATIONS_NAME,
        };
    }

    /// The virtual folder of the recently used files.
    pub mod recents {
        pub use cupertino_ui_files::recents::{location, recent_entries, XBEL_NAME};
    }

    /// Name checks, free names and when a press renames.
    pub mod rename {
        pub use cupertino_ui_files::rename::{
            NameError, RenameEvent, RenameTrigger, MAX_NAME_BYTES,
        };
    }

    /// The order of a folder's items.
    pub mod sort {
        pub use cupertino_ui_files::sort::{
            compare, insertion_index, sort_entries, SortKey, SortSpec,
        };
    }

    /// An item's tags.
    pub mod tags {
        pub use cupertino_ui_files::tags::{
            add_tag, parse_tag_list, read_tags, remove_tag, TagColor,
        };
    }
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
}
