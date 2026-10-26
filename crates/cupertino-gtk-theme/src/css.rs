//! GTK 3 and GTK 4 / libadwaita style sheets.
//!
//! Named colours: libadwaita reads `@define-color` names (`accent_bg_color`, `window_bg_color`,
//! `headerbar_bg_color`, ... — the list is the one that libadwaita 1.10 defines, read from the
//! installed library) and derives its CSS variables from them, so they are the authoritative
//! override mechanism. GTK 3 themes build on the built-in contained base theme
//! (`resource:///org/gtk/libgtk/theme/Adwaita/gtk-contained[-dark].css`, present in the installed
//! GTK 3) and override its `theme_*` colours; GTK 4 theme directories import the built-in default
//! theme (`resource:///org/gtk/libgtk/theme/Default/Default-{light,dark}.css`, present in the
//! installed GTK 4).
//!
//! Control rules (heights, radii, padding, focus ring, selection) are best-effort overrides with
//! the element names both toolkits use; the numbers come from [`Metrics`].

use crate::metrics::Metrics;
use crate::palette::{css as c, trim_float, Palette};
use cupertino_theme::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toolkit {
    Gtk3,
    Gtk4,
}

struct Out(String);

impl Out {
    fn comment(&mut self, text: &str) {
        self.0.push_str(&format!("/* {text} */\n"));
    }

    fn blank(&mut self) {
        self.0.push('\n');
    }

    fn color(&mut self, name: &str, value: Color) {
        self.0.push_str(&format!("@define-color {name} {};\n", c(value)));
    }

    fn color_raw(&mut self, name: &str, value: &str) {
        self.0.push_str(&format!("@define-color {name} {value};\n"));
    }

    fn rule(&mut self, selectors: &[&str], props: &[(&str, String)]) {
        self.0.push_str(&selectors.join(",\n"));
        self.0.push_str(" {\n");
        for (k, v) in props {
            self.0.push_str(&format!("  {k}: {v};\n"));
        }
        self.0.push_str("}\n\n");
    }
}

fn px(v: f64) -> String {
    format!("{}px", trim_float(v, 2))
}

fn alpha_of(c: Color, a: f32) -> Color {
    c.with_alpha(a)
}

fn shade(dark: bool) -> Color {
    if dark {
        Color::new(0.0, 0.0, 0.0, 0.36)
    } else {
        Color::new(0.0, 0.0, 0.0, 0.07)
    }
}

/// The libadwaita named colours, which GTK 4 applications built on libadwaita read.
fn adwaita_named_colors(o: &mut Out, p: &Palette) {
    let white = Color::new(1.0, 1.0, 1.0, 1.0);
    let sh = shade(p.dark);
    o.comment("accent and status colours (accent: controlAccent token; status: system colours)");
    o.color("accent_bg_color", p.accent);
    o.color("accent_fg_color", p.accent_fg);
    o.color("accent_color", p.accent);
    o.color("destructive_bg_color", p.destructive);
    o.color("destructive_fg_color", white);
    o.color("destructive_color", p.destructive);
    o.color("success_bg_color", p.success);
    o.color("success_fg_color", white);
    o.color("success_color", p.success);
    o.color("warning_bg_color", p.warning);
    o.color("warning_fg_color", Color::new(0.0, 0.0, 0.0, 0.8));
    o.color("warning_color", p.warning);
    o.color("error_bg_color", p.error);
    o.color("error_fg_color", white);
    o.color("error_color", p.error);
    o.blank();
    o.comment("window and content surfaces");
    o.color("window_bg_color", p.window_bg);
    o.color("window_fg_color", p.window_fg);
    o.color("view_bg_color", p.view_bg);
    o.color("view_fg_color", p.view_fg);
    o.color("headerbar_bg_color", p.headerbar_bg);
    o.color("headerbar_fg_color", p.headerbar_fg);
    o.color("headerbar_border_color", p.headerbar_fg.with_alpha(0.8));
    o.color("headerbar_backdrop_color", p.headerbar_backdrop);
    o.color("headerbar_shade_color", p.separator);
    o.color("headerbar_darker_shade_color", alpha_of(Color::new(0.0, 0.0, 0.0, 1.0), if p.dark { 0.9 } else { 0.12 }));
    o.color("card_bg_color", if p.dark { p.view_bg.with_alpha(1.0) } else { p.view_bg });
    o.color("card_fg_color", p.window_fg);
    o.color("card_shade_color", sh);
    o.color("thumbnail_bg_color", p.window_bg);
    o.color("thumbnail_fg_color", p.window_fg);
    o.color("dialog_bg_color", p.window_bg);
    o.color("dialog_fg_color", p.window_fg);
    o.color("popover_bg_color", p.popover_bg);
    o.color("popover_fg_color", p.window_fg);
    o.color("popover_shade_color", sh);
    o.color("shade_color", sh);
    o.color("scrollbar_outline_color", if p.dark { Color::new(0.0, 0.0, 0.0, 1.0) } else { Color::new(1.0, 1.0, 1.0, 1.0) });
    o.blank();
    o.comment("sidebars");
    for prefix in ["sidebar", "secondary_sidebar"] {
        o.color(&format!("{prefix}_bg_color"), p.sidebar_bg);
        o.color(&format!("{prefix}_fg_color"), p.window_fg);
        o.color(&format!("{prefix}_backdrop_color"), p.window_bg);
        o.color(&format!("{prefix}_shade_color"), sh);
        o.color(&format!("{prefix}_border_color"), alpha_of(Color::new(0.0, 0.0, 0.0, 1.0), 0.07));
    }
    o.blank();
    o.comment("names used by the rules below (not part of libadwaita)");
    o.color("cupertino_focus_ring", p.focus_ring);
    o.color("cupertino_separator", p.separator);
    o.color("cupertino_fill", p.fill);
    o.color("cupertino_fill_secondary", p.fill_secondary);
    o.color("cupertino_control", p.control);
    o.color("cupertino_secondary_fg", p.secondary_fg);
    o.color("cupertino_disabled_fg", p.disabled_fg);
    o.color("cupertino_placeholder", p.placeholder);
    o.color("cupertino_selection_inactive_bg", p.selection_inactive_bg);
    o.color("cupertino_selection_inactive_fg", p.selection_inactive_fg);
    o.color("cupertino_link", p.link);
    o.color("cupertino_link_visited", p.link_visited);
    o.color("cupertino_tooltip_bg", p.tooltip_bg);
    o.color("cupertino_tooltip_fg", p.tooltip_fg);
    o.color("cupertino_border", p.border);
    o.blank();
}

/// The GTK 3 `theme_*` names of the contained base theme.
fn gtk3_named_colors(o: &mut Out, p: &Palette) {
    o.comment("names of the built-in base theme (theme_*), plus the libadwaita-style names for applications that read them");
    o.color("theme_bg_color", p.window_bg);
    o.color("theme_fg_color", p.window_fg);
    o.color("theme_base_color", p.view_bg);
    o.color("theme_text_color", p.view_fg);
    o.color("theme_selected_bg_color", p.selection_bg);
    o.color("theme_selected_fg_color", p.selection_fg);
    o.color("theme_unfocused_bg_color", p.window_bg);
    o.color("theme_unfocused_fg_color", p.secondary_fg);
    o.color("theme_unfocused_base_color", p.view_bg);
    o.color("theme_unfocused_text_color", p.secondary_fg);
    o.color("theme_unfocused_selected_bg_color", p.selection_inactive_bg);
    o.color("theme_unfocused_selected_fg_color", p.selection_inactive_fg);
    o.color("insensitive_bg_color", p.solid(p.fill_tertiary));
    o.color("insensitive_fg_color", p.disabled_fg);
    o.color("insensitive_base_color", p.view_bg);
    o.color("unfocused_insensitive_color", p.disabled_fg);
    o.color("borders", p.border);
    o.color("unfocused_borders", p.separator);
    o.color("warning_color", p.warning);
    o.color("error_color", p.error);
    o.color("success_color", p.success);
    o.color("link_color", p.link);
    o.color("link_visited_color", p.link_visited);
    o.color("content_view_bg", p.view_bg);
    o.color("wm_title", p.headerbar_fg);
    o.color("wm_unfocused_title", p.secondary_fg);
    o.color("wm_bg", p.headerbar_bg);
    o.color("wm_unfocused_bg", p.headerbar_backdrop);
    o.blank();
}

fn control_rules(o: &mut Out, p: &Palette, m: &Metrics, tk: Toolkit) {
    let ring = format!("{} solid @cupertino_focus_ring", px(m.focus_ring_width));
    let inner = |h: f64| px((h - 2.0).max(1.0));
    o.comment(&format!("controls: {:?} size, heights from the control alignment table, radii approximate", m.size));
    o.rule(
        &["button"],
        &[
            ("min-height", inner(m.button_height)),
            ("min-width", px(m.button_height)),
            ("padding", format!("0 {}", px(m.padding_x))),
            ("border-radius", px(m.button_radius)),
            ("border", "1px solid @cupertino_border".into()),
            ("background-image", "none".into()),
            ("background-color", "@cupertino_control".into()),
            ("color", "@window_fg_color".into()),
            ("box-shadow", format!("0 1px 1px {}", c(p.shadow))),
            ("font-weight", "400".into()),
        ],
    );
    o.rule(&["button:hover"], &[("background-color", c(crate::palette::over(p.fill_secondary, p.solid(p.control))))]);
    o.rule(&["button:active", "button:checked"], &[("background-color", "@cupertino_fill".into()), ("box-shadow", "none".into())]);
    o.rule(&["button:disabled"], &[("color", "@cupertino_disabled_fg".into()), ("box-shadow", "none".into())]);
    o.rule(&["button.suggested-action"], &[("background-color", "@accent_bg_color".into()), ("color", "@accent_fg_color".into()), ("border-color", "transparent".into())]);
    o.rule(&["button.destructive-action"], &[("background-color", "@destructive_bg_color".into()), ("color", "@destructive_fg_color".into()), ("border-color", "transparent".into())]);
    o.rule(&["button.flat"], &[("background-color", "transparent".into()), ("border-color", "transparent".into()), ("box-shadow", "none".into())]);
    o.rule(&["button.flat:hover"], &[("background-color", "@cupertino_fill_secondary".into())]);

    let entry_sel: &[&str] = match tk {
        Toolkit::Gtk3 => &["entry", "spinbutton"],
        Toolkit::Gtk4 => &["entry", "spinbutton", "searchbar > revealer > box > entry"],
    };
    o.rule(
        entry_sel,
        &[
            ("min-height", inner(m.entry_height)),
            ("padding", format!("0 {}", px(m.padding_x - 2.0))),
            ("border-radius", px(m.entry_radius)),
            ("border", "1px solid @cupertino_border".into()),
            ("background-color", "@view_bg_color".into()),
            ("color", "@view_fg_color".into()),
            ("box-shadow", "none".into()),
        ],
    );
    o.rule(&["entry:disabled"], &[("color", "@cupertino_disabled_fg".into())]);

    let focus_sel: &[&str] = match tk {
        Toolkit::Gtk3 => &["button:focus", "entry:focus", "spinbutton:focus", "switch:focus", "scale:focus"],
        Toolkit::Gtk4 => &["button:focus-visible", "entry:focus-within", "spinbutton:focus-within", "switch:focus-visible", "scale:focus-visible"],
    };
    o.comment("focus ring: keyboardFocusIndicatorColor, width approximate");
    o.rule(focus_sel, &[("outline", ring.clone()), ("outline-offset", "-1px".into()), ("outline-style", "solid".into())]);

    let check_sel: &[&str] = &["check", "radio"];
    o.rule(
        check_sel,
        &[("min-width", px(m.check_size)), ("min-height", px(m.check_size)), ("border-radius", px(m.check_size / 4.0)), ("border", "1px solid @cupertino_border".into()), ("background-color", "@view_bg_color".into())],
    );
    o.rule(&["radio"], &[("border-radius", "50%".into())]);
    o.rule(&["check:checked", "radio:checked", "check:indeterminate", "radio:indeterminate"], &[("background-color", "@accent_bg_color".into()), ("color", "@accent_fg_color".into()), ("border-color", "transparent".into())]);

    o.rule(&["switch"], &[("min-width", px(m.switch_width)), ("min-height", px(m.switch_height)), ("border-radius", px(m.switch_height / 2.0)), ("background-color", "@cupertino_fill".into()), ("border", "none".into())]);
    o.rule(&["switch:checked"], &[("background-color", "@accent_bg_color".into())]);

    let trough: &str = match tk {
        Toolkit::Gtk3 => "scale trough",
        Toolkit::Gtk4 => "scale trough",
    };
    o.rule(&[trough], &[("min-height", px(m.slider_track)), ("border-radius", px(m.slider_track / 2.0)), ("background-color", "@cupertino_fill".into())]);
    o.rule(&["scale highlight"], &[("background-color", "@accent_bg_color".into()), ("border-radius", px(m.slider_track / 2.0))]);
    o.rule(&["scale slider"], &[("min-width", px(m.slider_knob - 2.0)), ("min-height", px(m.slider_knob - 2.0)), ("border-radius", "50%".into()), ("background-color", "@cupertino_control".into()), ("border", "1px solid @cupertino_border".into())]);

    o.rule(&["scrollbar slider"], &[("min-width", px(m.scrollbar_thickness - 6.0)), ("min-height", px(m.scrollbar_thickness - 6.0)), ("border-radius", px(m.scrollbar_thickness)), ("background-color", "@cupertino_fill".into())]);

    o.comment("headerbar and window");
    o.rule(
        &["headerbar"],
        &[("min-height", px(m.toolbar_button_height + 14.0)), ("background-color", "@headerbar_bg_color".into()), ("color", "@headerbar_fg_color".into()), ("border-bottom", "1px solid @cupertino_separator".into()), ("box-shadow", "none".into())],
    );
    o.rule(&["headerbar:backdrop"], &[("background-color", "@headerbar_backdrop_color".into()), ("color", "@cupertino_secondary_fg".into())]);
    let win_sel: &[&str] = match tk {
        Toolkit::Gtk3 => &["decoration"],
        Toolkit::Gtk4 => &["window.csd"],
    };
    o.rule(win_sel, &[("border-radius", format!("{} {} 0 0", px(m.window_radius), px(m.window_radius)))]);

    o.comment("menus and popovers: popover radius approximate, menu item radius (selection radius)");
    let pop: &[&str] = match tk {
        Toolkit::Gtk3 => &["menu", ".popup", "popover.background"],
        Toolkit::Gtk4 => &["popover > contents", "popover > arrow"],
    };
    o.rule(pop, &[("border-radius", px(m.popover_radius)), ("background-color", "@popover_bg_color".into()), ("color", "@popover_fg_color".into()), ("border", "1px solid @cupertino_border".into())]);
    let item: &[&str] = match tk {
        Toolkit::Gtk3 => &["menuitem:hover", "modelbutton:hover", "menuitem:focus"],
        Toolkit::Gtk4 => &["modelbutton:hover", "modelbutton:focus-visible", "popover.menu modelbutton:selected"],
    };
    o.rule(item, &[("background-color", "@theme_selected_bg_color".into()), ("color", "@theme_selected_fg_color".into()), ("border-radius", px(m.menu_item_radius)), ("min-height", px(m.row_height() - 4.0))]);

    o.comment("selection: accent while the window is focused, a neutral colour otherwise");
    let sel: &[&str] = match tk {
        Toolkit::Gtk3 => &["selection", "*:selected", "row:selected", "textview text selection", "entry selection"],
        Toolkit::Gtk4 => &["*:selected", "row:selected", "textview text selection", "entry selection", "label > selection"],
    };
    o.rule(sel, &[("background-color", "@theme_selected_bg_color".into()), ("color", "@theme_selected_fg_color".into())]);
    o.rule(&["*:selected:backdrop", "row:selected:backdrop"], &[("background-color", "@cupertino_selection_inactive_bg".into()), ("color", "@cupertino_selection_inactive_fg".into())]);
    o.rule(&["*:disabled"], &[("color", "@cupertino_disabled_fg".into())]);
    o.rule(&["entry text placeholder", "entry placeholder"], &[("color", "@cupertino_placeholder".into())]);
    o.rule(&["link", "*:link"], &[("color", "@cupertino_link".into())]);
    o.rule(&["*:visited"], &[("color", "@cupertino_link_visited".into())]);

    o.rule(&["tooltip", "tooltip.background"], &[("background-color", "@cupertino_tooltip_bg".into()), ("color", "@cupertino_tooltip_fg".into()), ("border-radius", px(m.entry_radius)), ("border", format!("{} solid @cupertino_border", px(m.rim_width)))]);

    let header: &[&str] = match tk {
        Toolkit::Gtk3 => &["treeview header button"],
        Toolkit::Gtk4 => &["columnview > header > button", "treeview header button"],
    };
    o.rule(header, &[("min-height", px(m.table_header_height - 2.0)), ("border-radius", "0".into()), ("font-weight", "600".into())]);
    o.rule(&["list > row", "row"], &[("min-height", px(m.row_height()))]);
}

fn header(o: &mut Out, p: &Palette, what: &str) {
    o.comment(&format!("Cupertino theme, {what}. Generated by cupertino-gtk-theme from the design tokens; do not edit."));
    o.comment(&format!("appearance: {}, accent: {}", p.appearance.as_str(), p.accent_name.name()));
    o.blank();
}

/// Style sheet for libadwaita applications (`~/.config/gtk-4.0/gtk.css` or `gtk-dark.css`).
pub fn libadwaita_override(p: &Palette, m: &Metrics) -> String {
    let mut o = Out(String::new());
    header(&mut o, p, "libadwaita override");
    adwaita_named_colors(&mut o, p);
    o.color_raw("theme_bg_color", "@window_bg_color");
    o.color_raw("theme_fg_color", "@window_fg_color");
    o.color_raw("theme_base_color", "@view_bg_color");
    o.color_raw("theme_text_color", "@view_fg_color");
    o.color_raw("theme_selected_bg_color", "@accent_bg_color");
    o.color_raw("theme_selected_fg_color", "@accent_fg_color");
    o.blank();
    control_rules(&mut o, p, m, Toolkit::Gtk4);
    o.0
}

/// Style sheet of the GTK 4 theme directory: imports the built-in default theme of the same
/// brightness, then overrides.
pub fn gtk4_theme(p: &Palette, m: &Metrics) -> String {
    let mut o = Out(String::new());
    header(&mut o, p, "GTK 4 theme");
    o.0.push_str(&format!("@import url(\"resource:///org/gtk/libgtk/theme/Default/Default-{}.css\");\n\n", if p.dark { "dark" } else { "light" }));
    adwaita_named_colors(&mut o, p);
    o.color_raw("theme_bg_color", "@window_bg_color");
    o.color_raw("theme_fg_color", "@window_fg_color");
    o.color_raw("theme_base_color", "@view_bg_color");
    o.color_raw("theme_text_color", "@view_fg_color");
    o.color_raw("theme_selected_bg_color", "@accent_bg_color");
    o.color_raw("theme_selected_fg_color", "@accent_fg_color");
    o.blank();
    control_rules(&mut o, p, m, Toolkit::Gtk4);
    o.0
}

/// Style sheet of the GTK 3 theme directory: imports the contained base theme of the same
/// brightness, then overrides.
pub fn gtk3_theme(p: &Palette, m: &Metrics) -> String {
    let mut o = Out(String::new());
    header(&mut o, p, "GTK 3 theme");
    o.0.push_str(&format!("@import url(\"resource:///org/gtk/libgtk/theme/Adwaita/gtk-contained{}.css\");\n\n", if p.dark { "-dark" } else { "" }));
    gtk3_named_colors(&mut o, p);
    adwaita_named_colors(&mut o, p);
    control_rules(&mut o, p, m, Toolkit::Gtk3);
    o.0
}
