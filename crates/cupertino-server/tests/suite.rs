// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[allow(dead_code, unused_imports, unused_macros)]
mod common;

#[path = "activation_env.rs"]
mod activation_env;
#[path = "app_chooser.rs"]
mod app_chooser;
#[path = "app_effects_v1.rs"]
mod app_effects_v1;
#[path = "appearance_fade.rs"]
mod appearance_fade;
#[path = "asset-isolation.rs"]
mod asset_isolation;
#[path = "backdrop_tone.rs"]
mod backdrop_tone;
#[path = "capture.rs"]
mod capture;
#[path = "clipboard.rs"]
mod clipboard;
#[path = "content_type.rs"]
mod content_type;
#[path = "contracts.rs"]
mod contracts;
#[path = "cursor.rs"]
mod cursor;
#[path = "data_control.rs"]
mod data_control;
#[path = "dbus.rs"]
mod dbus;
#[path = "desktop-start.rs"]
mod desktop_start;
#[path = "diagnostic_events.rs"]
mod diagnostic_events;
#[path = "diagnostic-isolation.rs"]
mod diagnostic_isolation;
#[path = "display_power.rs"]
mod display_power;
#[path = "dock_v1.rs"]
mod dock_v1;
#[path = "drag-and-drop.rs"]
mod drag_and_drop;
#[path = "drawn-buffers.rs"]
mod drawn_buffers;
#[path = "event-sizes.rs"]
mod event_sizes;
#[path = "fake_renderer.rs"]
mod fake_renderer;
#[path = "files_window.rs"]
mod files_window;
#[path = "focus_on_map.rs"]
mod focus_on_map;
#[path = "force_quit.rs"]
mod force_quit;
#[path = "frame_pacing.rs"]
mod frame_pacing;
#[path = "goldens.rs"]
mod goldens;
#[path = "gpu-reset.rs"]
mod gpu_reset;
#[path = "headless.rs"]
mod headless;
#[path = "hung_apps.rs"]
mod hung_apps;
#[path = "kit-toolbar.rs"]
mod kit_toolbar;
#[path = "layer_input.rs"]
mod layer_input;
#[path = "live_settings.rs"]
mod live_settings;
#[path = "lock_screen.rs"]
mod lock_screen;
#[path = "loop_wakeups.rs"]
mod loop_wakeups;
#[path = "menu_bar_v1.rs"]
mod menu_bar_v1;
#[path = "mice-property.rs"]
mod mice_property;
#[path = "move-resize.rs"]
mod move_resize;
#[path = "output_management.rs"]
mod output_management;
#[path = "output_removal.rs"]
mod output_removal;
#[path = "output_scale.rs"]
mod output_scale;
#[path = "overlays.rs"]
mod overlays;
#[path = "pam-isolation.rs"]
mod pam_isolation;
#[path = "pointer_buttons.rs"]
mod pointer_buttons;
#[path = "pointer_constraints.rs"]
mod pointer_constraints;
#[path = "pointer_output.rs"]
mod pointer_output;
#[path = "popup-grab.rs"]
mod popup_grab;
#[path = "popup-grab-keyboard.rs"]
mod popup_grab_keyboard;
#[path = "popup_over_dock.rs"]
mod popup_over_dock;
#[path = "prompt.rs"]
mod prompt;
#[path = "quit.rs"]
mod quit;
#[path = "record.rs"]
mod record;
#[path = "sandbox_globals.rs"]
mod sandbox_globals;
#[path = "scene_composition.rs"]
mod scene_composition;
#[path = "scene_cost.rs"]
mod scene_cost;
#[path = "scene_repaint.rs"]
mod scene_repaint;
#[path = "scene_resources.rs"]
mod scene_resources;
#[path = "scene_transform.rs"]
mod scene_transform;
#[path = "screen_reader.rs"]
mod screen_reader;
#[path = "screen_share.rs"]
mod screen_share;
#[path = "scroll_v1.rs"]
mod scroll_v1;
#[path = "session-end.rs"]
mod session_end;
#[path = "session_lock.rs"]
mod session_lock;
#[path = "session_restore.rs"]
mod session_restore;
#[path = "session_v1.rs"]
mod session_v1;
#[path = "settings-popup.rs"]
mod settings_popup;
#[path = "shell_diagnostics.rs"]
mod shell_diagnostics;
#[path = "shell_processes.rs"]
mod shell_processes;
#[path = "shell_roles.rs"]
mod shell_roles;
#[path = "shm_damage.rs"]
mod shm_damage;
#[path = "shortcut_inhibit.rs"]
mod shortcut_inhibit;
#[path = "shortcut_inhibit_scene.rs"]
mod shortcut_inhibit_scene;
#[path = "single_pixel_buffer.rs"]
mod single_pixel_buffer;
#[path = "socket_names.rs"]
mod socket_names;
#[path = "standard-clients.rs"]
mod standard_clients;
#[path = "subsurface_backdrops.rs"]
mod subsurface_backdrops;
#[path = "surface_output.rs"]
mod surface_output;
#[path = "text_input.rs"]
mod text_input;
#[path = "toplevel_icon.rs"]
mod toplevel_icon;
#[path = "unified_titlebar.rs"]
mod unified_titlebar;
#[path = "wallpaper.rs"]
mod wallpaper;
#[path = "window_kinds.rs"]
mod window_kinds;
#[path = "window_manager_v1.rs"]
mod window_manager_v1;
#[path = "window_popup_backdrop.rs"]
mod window_popup_backdrop;
#[path = "xdg_foreign.rs"]
mod xdg_foreign;
