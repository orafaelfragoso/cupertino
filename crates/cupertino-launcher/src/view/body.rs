//! What the searching panel shows below its query row: the results, the selected one's preview and its actions.

use super::*;

/// What the searching panel shows below its query row: the results beside the selected one's preview, the enlarged
/// preview over them, or the row saying why nothing shows.
pub(super) fn search_body<M>(view: &PanelView<'_>) -> Vec<Element<M>>
where
    M: From<Input> + Clone + 'static,
{
    let panel = view.panel;
    let rows = visible_rows(panel);
    let separator = || {
        Element::new(Hairline {
            axis: Axis::Horizontal,
        })
        .key("separator")
    };
    if view.nested == Some(Nested::Preview) && panel.selected_item().is_some() {
        return vec![separator(), enlarged_preview(view).key("enlarged-preview")];
    }
    if rows.is_empty() {
        let Some(text) = status_text(&view.status) else {
            return Vec::new();
        };
        // the row says it at once; aloud it is said once the query settled, by a node of its own
        let said = view.said.as_ref().and_then(|s| {
            Some(StatusAnnouncer {
                text: status_text(s)?,
                failed: matches!(s, Status::Failed(_)),
            })
        });
        let mut body = vec![
            separator(),
            Element::new(StatusRow {
                text,
                failed: matches!(view.status, Status::Failed(_)),
                live: false,
            })
            .key("status"),
        ];
        body.extend(said.map(|s| Element::new(s).key("status-said")));
        return body;
    }
    let body = vec![
        results(view, &rows).key("results"),
        Element::new(Hairline {
            axis: Axis::Vertical,
        })
        .key("divider"),
        preview_pane(view).key("preview"),
    ];
    vec![
        separator(),
        Element::new(Plain::row(body, Align::Stretch)).key("body"),
    ]
}

/// The selected result's preview over the whole body: its picture as large as it can be shown, its name and what is
/// known of it, and its actions; or that the host gave nothing in time.
pub(super) fn enlarged_preview<M>(view: &PanelView<'_>) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let mut children = vec![preview_pane(view).key("pane")];
    if view.preview_unavailable {
        children.insert(
            0,
            Element::new(StatusRow {
                text: PREVIEW_UNAVAILABLE,
                failed: false,
                live: true,
            })
            .key("unavailable"),
        );
    }
    Element::new(Plain::sheet(children))
}

/// The shown rows: a list of the sections, each a group named by its heading over its results.
pub(super) fn results<M>(view: &PanelView<'_>, rows: &[Row]) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let panel = view.panel;
    let mut sections: Vec<Element<M>> = Vec::new();
    let mut current: Option<(String, Vec<Element<M>>)> = None;
    let flush = |sections: &mut Vec<Element<M>>, current: Option<(String, Vec<Element<M>>)>| {
        if let Some((title, children)) = current {
            let key = format!("section:{title}");
            sections.push(Element::new(SectionGroup { title, children }).key(key));
        }
    };
    for row in rows {
        match row {
            Row::Header(title) => {
                flush(&mut sections, current.take());
                current = Some((
                    title.clone(),
                    vec![Element::new(Quiet::heading(title)).key("heading")],
                ));
            }
            &Row::Item { flat, calculator } => {
                let Some(item) = panel.results().get(flat) else {
                    continue;
                };
                let element = result_row(view, item, flat, calculator, flat == panel.selected());
                match &mut current {
                    Some((_, children)) => children.push(element),
                    // the list scrolled past this section's heading
                    None => {
                        current = Some((
                            panel
                                .results()
                                .locate(flat)
                                .and_then(|(s, _)| panel.results().sections.get(s))
                                .map_or_else(String::new, |s| s.title.clone()),
                            vec![element],
                        ))
                    }
                }
            }
        }
    }
    flush(&mut sections, current);
    Element::new(ResultList {
        count: panel.results().len(),
        children: sections,
    })
}

/// The selected result's preview: a picture of it, its name and what is known of it, and its actions at the foot.
pub(super) fn preview_pane<M>(view: &PanelView<'_>) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let panel = view.panel;
    let Some(item) = panel.selected_item() else {
        return Element::new(Plain::pane(Vec::new()));
    };
    let preview = view.preview;
    let mut children: Vec<Element<M>> = Vec::new();
    if is_value(item.kind) {
        children.push(
            Element::new(Label::new(&item.title, PREVIEW_VALUE_SIZE, 500).centred(2)).key("value"),
        );
        children.push(
            Element::new(
                Label::new(item.subtitle.trim_end_matches(" ="), 13.0, 400)
                    .secondary()
                    .centred(2),
            )
            .key("expression"),
        );
    } else {
        children.push(preview_visual(view, item, preview).key("visual"));
        children.push(
            Element::new(Label::new(&item.title, PREVIEW_TITLE_SIZE, 600).centred(2)).key("name"),
        );
        let kind = match (preview, item.kind) {
            (Some(p), _) if !p.kind.is_empty() => p.kind.clone(),
            (_, ItemKind::Application) => "Application".to_owned(),
            (_, ItemKind::ApplicationAction) => item.subtitle.clone(),
            (_, ItemKind::WebSearch) => "Search the web".to_owned(),
            _ => String::new(),
        };
        if !kind.is_empty() {
            children.push(
                Element::new(Label::new(&kind, 12.0, 400).secondary().centred(1)).key("kind"),
            );
        }
        if let Some(p) = preview {
            for (label, value) in &p.details {
                children.push(
                    Element::new(Plain::detail(vec![
                        Element::new(
                            Label::new(label, 11.0, 400)
                                .secondary()
                                .ending(DETAIL_LABEL),
                        ),
                        Element::new(Label::new(value, 11.0, 400).lines(1)).fill(),
                    ]))
                    .key(format!("detail:{label}")),
                );
            }
        } else if matches!(item.kind, ItemKind::Application) && !item.subtitle.is_empty() {
            children.push(
                Element::new(Label::new(&item.subtitle, 11.0, 400).secondary().centred(2))
                    .key("description"),
            );
        }
    }
    // holding the primary modifier shows where a file or folder is
    if let (true, crate::engine::Action::OpenPath(path)) = (view.show_location, &item.action) {
        children.push(
            Element::new(
                Label::new(&crate::engine::home_relative(path), 11.0, 400)
                    .secondary()
                    .centred(2),
            )
            .key("location"),
        );
    }
    children.push(Element::new(Plain::spacer()));
    children.push(action_list(view).key("actions"));
    Element::new(Plain::pane(children))
}

/// The picture at the top of the preview: the file's own, the start of its text, or the result's icon.
pub(super) fn preview_visual<M: 'static>(
    view: &PanelView<'_>,
    item: &ResultItem,
    preview: Option<&Preview>,
) -> Element<M> {
    match preview.map(|p| &p.body) {
        Some(PreviewBody::Image { sources, size }) => {
            return Element::new(Image {
                sources: sources.clone(),
                size: *size,
                fit: ImageFit::Contain,
                radius: PREVIEW_IMAGE_RADIUS,
                label: None,
            })
        }
        Some(PreviewBody::Text(text)) => return Element::new(TextBlock { text: text.clone() }),
        _ => {}
    }
    let size = Size::new(PREVIEW_ICON_SIZE, PREVIEW_ICON_SIZE);
    let sources = preview
        .map(|p| &p.icon)
        .filter(|icon| !icon.is_empty())
        .or_else(|| view.icons.get(&item.id));
    match (sources, fallback_symbol(item.kind)) {
        (Some(sources), _) => Element::new(Image {
            sources: sources.clone(),
            size,
            fit: ImageFit::Contain,
            radius: 0.0,
            label: None,
        }),
        (None, Some(symbol)) => Element::new(Icon {
            name: symbol.to_owned(),
            size: PREVIEW_ICON_SIZE,
            direction: IconDirection::Fixed,
            label: None,
            tint: (item.kind == ItemKind::WebSearch).then_some(ColorRole::SecondaryText),
        }),
        (None, None) => Element::new(Plain::spacer()),
    }
}

/// The selected result's actions as buttons, the chosen one filled; at most [`MAX_ACTIONS`] of them, the chosen one
/// always among them.
pub(super) fn action_list<M>(view: &PanelView<'_>) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let panel = view.panel;
    let chosen = panel.action();
    let first = (chosen + 1).saturating_sub(MAX_ACTIONS);
    let buttons = view
        .actions
        .iter()
        .enumerate()
        .skip(first)
        .take(MAX_ACTIONS)
        .map(|(index, action)| {
            Element::new(ActionButton {
                flat: panel.selected(),
                index,
                label: action.label.clone(),
                chosen: index == chosen,
            })
            .key(format!("action:{index}"))
        })
        .collect();
    Element::new(ActionList {
        count: view.actions.len(),
        children: buttons,
    })
}

pub(super) fn text<M: 'static>(text: &str, role: TextRole, secondary: bool) -> Element<M> {
    Element::new(Text {
        content: AttributedText::plain(text, TextStyle::new(13.0)),
        role,
        wrap: false,
        secondary,
        selected: false,
    })
}

pub(super) fn result_row<M>(
    view: &PanelView<'_>,
    item: &ResultItem,
    flat: usize,
    calculator: bool,
    selected: bool,
) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let icon = match (view.icons.get(&item.id), fallback_symbol(item.kind)) {
        (Some(sources), _) => Some(Element::new(Image {
            sources: sources.clone(),
            size: Size::new(ICON_SIZE, ICON_SIZE),
            fit: ImageFit::Contain,
            radius: 0.0,
            label: None,
        })),
        (None, Some(symbol)) => Some(Element::new(Icon {
            name: symbol.to_owned(),
            size: ICON_SIZE,
            direction: IconDirection::Fixed,
            label: None,
            tint: (item.kind == ItemKind::WebSearch).then_some(ColorRole::SecondaryText),
        })),
        // the calculator's value starts at the icon's place
        (None, None) => None,
    };
    let title_role = if calculator {
        TextRole::Title
    } else {
        TextRole::Body
    };
    let mut lines = vec![text(&item.title, title_role, false)];
    if !item.subtitle.is_empty() {
        lines.push(text(&item.subtitle, TextRole::Caption, true));
    }
    let label = Element::new(Quiet::lines(lines)).fill();
    Element::new(ResultRow {
        flat,
        title: item.title.clone(),
        subtitle: item.subtitle.clone(),
        height: if calculator {
            CALCULATOR_ROW_HEIGHT
        } else {
            ROW_HEIGHT
        },
        selected,
        children: icon.into_iter().chain([label]).collect(),
    })
    .key(format!("item:{}", item.id))
}

/// A container's leaf callbacks: layout sizes it.
pub(super) fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

pub(super) fn column(insets: Insets) -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fill,
        insets,
        ..LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Stretch,
            justify: Justify::Start,
        })
    }
}
