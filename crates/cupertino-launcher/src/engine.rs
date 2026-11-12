//! The launcher engine: turns a query into sectioned results and an activation into a command.
//!
//! Providers: the calculator ([`crate::calc`]), applications from desktop entries (and their
//! desktop actions), and an optional file index ([`crate::files`]). Every item carries a stable
//! `id` used as the frecency key (`app:<desktop id>[#action]`, `file:<path>`).
//!
//! Ranking: text score from [`crate::matcher`] plus the frecency bonus. Sections are ordered by the
//! best score of their first item, the calculator always comes first when it answers. With an empty
//! query the engine offers the most frecent applications as suggestions.
//!
//! Activation returns data only ([`Activation`]); the host spawns the command. An optional hook
//! mints an xdg-activation token per launch, which is put in the command's environment.

use crate::calc;
use crate::convert;
use crate::desktop::DesktopEntry;
use crate::exec::{self, CommandSpec};
use crate::files::FileIndex;
use crate::frecency::Frecency;
use crate::matcher::{self, Candidate, CandidateField, Query, Weights};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    LaunchApp {
        desktop_id: String,
        action: Option<String>,
    },
    OpenPath(PathBuf),
    OpenUrl(String),
    CopyText(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemKind {
    Calculator,
    /// A unit conversion ([`crate::convert`]), shown as the calculator's result is.
    Conversion,
    Application,
    ApplicationAction,
    Folder,
    File,
    WebSearch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SectionKind {
    /// The single best match across the other sections (at most one top hit).
    TopHit,
    Calculator,
    Conversion,
    Suggestions,
    Applications,
    /// Declared before `Folders`: equal-scoring sections follow the design default category
    /// order (documents before directories).
    Files,
    Folders,
    Web,
}

impl SectionKind {
    pub fn title(self) -> &'static str {
        match self {
            SectionKind::TopHit => "Top Hit",
            SectionKind::Calculator => "Calculator",
            SectionKind::Conversion => "Conversion",
            SectionKind::Suggestions => "Suggestions",
            SectionKind::Applications => "Applications",
            SectionKind::Folders => "Folders",
            SectionKind::Files => "Files",
            SectionKind::Web => "Web",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResultItem {
    /// Stable identifier (frecency key).
    pub id: String,
    pub kind: ItemKind,
    pub title: String,
    pub subtitle: String,
    /// Icon name or absolute path, resolved by the renderer.
    pub icon: Option<String>,
    pub score: f64,
    /// Character indices of `title` that matched the query.
    pub highlights: Vec<usize>,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub kind: SectionKind,
    pub title: String,
    pub items: Vec<ResultItem>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Results {
    pub query: String,
    pub sections: Vec<Section>,
}

impl Results {
    pub fn len(&self) -> usize {
        self.sections.iter().map(|s| s.items.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Item by flat index (sections in order).
    pub fn get(&self, flat: usize) -> Option<&ResultItem> {
        self.sections.iter().flat_map(|s| s.items.iter()).nth(flat)
    }

    /// `(section, item)` of a flat index.
    pub fn locate(&self, flat: usize) -> Option<(usize, usize)> {
        let mut left = flat;
        for (si, s) in self.sections.iter().enumerate() {
            if left < s.items.len() {
                return Some((si, left));
            }
            left -= s.items.len();
        }
        None
    }

    pub fn iter(&self) -> impl Iterator<Item = &ResultItem> {
        self.sections.iter().flat_map(|s| s.items.iter())
    }
}

#[derive(Clone, Debug)]
pub struct LauncherConfig {
    pub weights: Weights,
    pub max_apps: usize,
    pub max_folders: usize,
    pub max_files: usize,
    pub suggestions: usize,
    pub show_calculator: bool,
    pub include_actions: bool,
    /// Applications scoring below this are dropped (100 = any subsequence match).
    pub min_app_score: f64,
    /// Terminal emulator argv prefix for `Terminal=true` entries. unknown for this system.
    pub terminal_command: Vec<String>,
    /// Command used to open files, folders and URLs.
    pub open_command: Vec<String>,
    /// Web search fallback; `{}` is replaced with the percent-encoded query. `None` disables it.
    pub web_search_url: Option<String>,
    /// Move the best application / file / folder match into a leading single-item "Top Hit"
    /// section (the design launcher shows at most one top hit). A calculator
    /// result already leads the list and suppresses the extra section.
    pub top_hit: bool,
}

/// Limits of the design launcher's result list (shipped defaults); hosts that implement
/// a "show more" affordance can use them as the initial section size.
pub mod reference_limits {
    /// Rows shown per section before "Show More".
    pub const VISIBLE_PER_SECTION: usize = 3;
    /// Sections shown before "Show More" is offered for the remaining sections.
    pub const SECTIONS_BEFORE_SHOW_MORE: usize = 5;
    /// Top hits.
    pub const MAX_TOP_HITS: usize = 1;
    /// Suggestions in the suggestion block.
    pub const MAX_SUGGESTIONS: usize = 3;
}

/// Default order of the user-visible result categories (an enablement list); the runtime
/// order of sections is relevance driven.
pub const DEFAULT_CATEGORY_ORDER: [&str; 21] = [
    "applications",
    "suggestions",
    "conversion",
    "calculator",
    "definition",
    "settings",
    "documents",
    "folders",
    "presentations",
    "spreadsheets",
    "pdf",
    "messages",
    "contacts",
    "events",
    "images",
    "bookmarks",
    "music",
    "movies",
    "fonts",
    "other",
    "tips",
];

impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            weights: Weights::default(),
            max_apps: 8,
            max_folders: 3,
            max_files: 5,
            suggestions: 6,
            show_calculator: true,
            include_actions: true,
            min_app_score: 100.0,
            terminal_command: vec!["xterm".into(), "-e".into()],
            open_command: vec!["xdg-open".into()],
            web_search_url: None,
            top_hit: true,
        }
    }
}

/// What a launch is about to start; passed to the activation token hook.
#[derive(Clone, Debug, PartialEq)]
pub struct LaunchRequest {
    pub desktop_id: Option<String>,
    pub title: String,
}

pub type TokenHook = Box<dyn FnMut(&LaunchRequest) -> Option<String> + Send>;

/// What one of a result's actions does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionKind {
    /// The result's own activation: launch, open, copy the value or search.
    Default,
    /// Open the folder holding a file, or a folder itself, in the file manager.
    Reveal,
    /// Put the path of a file or folder on the clipboard.
    CopyPath,
    /// One of an application's desktop actions, by its id.
    Desktop(String),
}

/// One action of a result, as the panel offers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemAction {
    pub label: String,
    pub kind: ActionKind,
}

impl ItemAction {
    fn new(label: &str, kind: ActionKind) -> Self {
        Self {
            label: label.to_owned(),
            kind,
        }
    }
}

/// The desktop id of the session's file manager, which Reveal opens a folder in when it is installed.
pub const FILE_MANAGER_ID: &str = "cupertino-files.desktop";

#[derive(Clone, Debug, PartialEq)]
pub enum Activation {
    /// Spawn this command (environment already contains the activation token, if any).
    Run(CommandSpec),
    /// Open the application of desktop file ID `desktop_id` as a click on its dock tile does: bring it forward when it
    /// runs, and only otherwise spawn `spec`, its entry's command.
    OpenApp {
        desktop_id: String,
        spec: CommandSpec,
    },
    CopyToClipboard(String),
    Nothing,
}

impl Activation {
    /// The command it would spawn: [`Activation::Run`]'s, or the entry's command of [`Activation::OpenApp`].
    pub fn into_command(self) -> Option<CommandSpec> {
        match self {
            Activation::Run(spec) | Activation::OpenApp { spec, .. } => Some(spec),
            Activation::CopyToClipboard(_) | Activation::Nothing => None,
        }
    }
}

struct AppCandidate {
    entry: usize,
    action: Option<usize>,
    key: String,
    candidate: Candidate,
}

pub struct Launcher {
    pub config: LauncherConfig,
    pub frecency: Frecency,
    entries: Vec<DesktopEntry>,
    apps: Vec<AppCandidate>,
    files: Option<Arc<FileIndex>>,
    token_hook: Option<TokenHook>,
}

fn exec_basename(exec: &str) -> String {
    exec::tokenize(exec)
        .and_then(|a| a.into_iter().next())
        .map(|p| p.rsplit('/').next().unwrap_or("").to_string())
        .unwrap_or_default()
}

fn build_candidates(entries: &[DesktopEntry], include_actions: bool) -> Vec<AppCandidate> {
    let mut out = Vec::with_capacity(entries.len());
    for (i, e) in entries.iter().enumerate() {
        let mut fields = vec![CandidateField::new(&e.name, 1.0, true)];
        if !e.generic_name.is_empty() {
            fields.push(CandidateField::new(&e.generic_name, 0.7, true));
        }
        fields.extend(
            e.keywords
                .iter()
                .map(|k| CandidateField::new(k, 0.65, false)),
        );
        let base = exec_basename(&e.exec);
        if !base.is_empty() {
            fields.push(CandidateField::new(&base, 0.6, false));
        }
        let id_stem = e.id.trim_end_matches(".desktop");
        fields.push(CandidateField::new(
            id_stem.rsplit('.').next().unwrap_or(id_stem),
            0.4,
            false,
        ));
        if !e.comment.is_empty() {
            fields.push(CandidateField::new(&e.comment, 0.3, false));
        }
        out.push(AppCandidate {
            entry: i,
            action: None,
            key: format!("app:{}", e.id),
            candidate: Candidate {
                fields,
                required_field: None,
            },
        });
        if include_actions {
            for (ai, a) in e.actions.iter().enumerate() {
                let title = format!("{}: {}", e.name, a.name);
                let fields = vec![
                    CandidateField::new(&title, 0.8, true),
                    CandidateField::new(&a.name, 1.0, false),
                    CandidateField::new(&e.name, 0.5, false),
                ];
                out.push(AppCandidate {
                    entry: i,
                    action: Some(ai),
                    key: format!("app:{}#{}", e.id, a.id),
                    candidate: Candidate {
                        fields,
                        required_field: Some(1),
                    },
                });
            }
        }
    }
    out
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn home_relative(p: &std::path::Path) -> String {
    match std::env::var_os("HOME").map(PathBuf::from) {
        Some(h) if p.starts_with(&h) => {
            let rel = p.strip_prefix(&h).unwrap_or(p);
            if rel.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", rel.display())
            }
        }
        _ => p.display().to_string(),
    }
}

/// The file search as a provider: the folders and files of `index` matching `text`, scored by name alone (the usage
/// bonus and the section limits come in [`Launcher::compose`]). It reads only the in-memory index, so a host can run it
/// on any thread; it gives up, answering nothing, once `cancel` says a newer query started.
pub fn file_sections(
    index: &FileIndex,
    text: &str,
    now: f64,
    config: &LauncherConfig,
    cancel: Option<&crate::providers::Cancel>,
) -> Vec<Section> {
    let q = Query::new(text);
    if q.is_empty() || cancel.is_some_and(|c| c.is_cancelled()) {
        return Vec::new();
    }
    let found = index.search(
        &q,
        (config.max_files + config.max_folders) * 4,
        now,
        &config.weights,
    );
    if cancel.is_some_and(|c| c.is_cancelled()) {
        return Vec::new();
    }
    let mut folders = Vec::new();
    let mut files = Vec::new();
    for m in found {
        let e = index.entry(m.index);
        let item = ResultItem {
            id: format!("file:{}", e.path.display()),
            kind: if e.is_dir {
                ItemKind::Folder
            } else {
                ItemKind::File
            },
            title: e.name.clone(),
            subtitle: home_relative(e.path.parent().unwrap_or(&e.path)),
            icon: None,
            score: m.score,
            highlights: m.positions,
            action: Action::OpenPath(e.path.clone()),
        };
        if e.is_dir {
            folders.push(item);
        } else {
            files.push(item);
        }
    }
    [(SectionKind::Folders, folders), (SectionKind::Files, files)]
        .into_iter()
        .filter(|(_, items)| !items.is_empty())
        .map(|(kind, items)| Section {
            kind,
            title: kind.title().into(),
            items,
        })
        .collect()
}

impl Launcher {
    pub fn new(config: LauncherConfig, entries: Vec<DesktopEntry>, frecency: Frecency) -> Self {
        let apps = build_candidates(&entries, config.include_actions);
        Self {
            config,
            frecency,
            entries,
            apps,
            files: None,
            token_hook: None,
        }
    }

    /// Index the XDG application directories and load the persisted frecency store.
    pub fn from_system(config: LauncherConfig) -> Self {
        let entries = crate::desktop::index_applications(
            &crate::desktop::xdg_application_dirs(),
            &crate::desktop::Locale::from_env(),
            &crate::desktop::Visibility::from_env(),
        );
        Self::new(config, entries, Frecency::load_default())
    }

    pub fn set_entries(&mut self, entries: Vec<DesktopEntry>) {
        self.apps = build_candidates(&entries, self.config.include_actions);
        self.entries = entries;
    }

    pub fn entries(&self) -> &[DesktopEntry] {
        &self.entries
    }

    pub fn entry(&self, desktop_id: &str) -> Option<&DesktopEntry> {
        self.entries.iter().find(|e| e.id == desktop_id)
    }

    pub fn set_file_index(&mut self, index: Option<Arc<FileIndex>>) {
        self.files = index;
    }

    pub fn file_index(&self) -> Option<&Arc<FileIndex>> {
        self.files.as_ref()
    }

    pub fn set_activation_token_hook(&mut self, hook: Option<TokenHook>) {
        self.token_hook = hook;
    }

    /// The application `desktop_id` as a result, for the browser to open as the search would.
    pub fn app_result(&self, desktop_id: &str) -> Option<ResultItem> {
        let key = format!("app:{desktop_id}");
        let c = self.apps.iter().find(|c| c.key == key)?;
        Some(self.app_item(c, 0.0, Vec::new()))
    }

    /// The desktop IDs of the applications launched most, the most recent and frequent first: the browser's first row.
    pub fn recent_apps(&self, limit: usize, now: f64) -> Vec<String> {
        self.frecency
            .top(limit * 3, now)
            .into_iter()
            .filter_map(|(k, _)| k.strip_prefix("app:").map(str::to_owned))
            .filter(|id| !id.contains('#') && self.entry(id).is_some())
            .take(limit)
            .collect()
    }

    fn app_item(&self, c: &AppCandidate, score: f64, highlights: Vec<usize>) -> ResultItem {
        let e = &self.entries[c.entry];
        match c.action {
            None => ResultItem {
                id: c.key.clone(),
                kind: ItemKind::Application,
                title: e.name.clone(),
                subtitle: if !e.generic_name.is_empty() {
                    e.generic_name.clone()
                } else {
                    e.comment.clone()
                },
                icon: e.icon.clone(),
                score,
                highlights,
                action: Action::LaunchApp {
                    desktop_id: e.id.clone(),
                    action: None,
                },
            },
            Some(ai) => {
                let a = &e.actions[ai];
                ResultItem {
                    id: c.key.clone(),
                    kind: ItemKind::ApplicationAction,
                    title: format!("{}: {}", e.name, a.name),
                    subtitle: String::new(),
                    icon: a.icon.clone().or_else(|| e.icon.clone()),
                    score,
                    highlights,
                    action: Action::LaunchApp {
                        desktop_id: e.id.clone(),
                        action: Some(a.id.clone()),
                    },
                }
            }
        }
    }

    /// Results for `text`. `now`: Unix seconds (frecency decay, file recency).
    pub fn query(&self, text: &str, now: f64) -> Results {
        let provided = match &self.files {
            Some(index) => file_sections(index, text, now, &self.config, None),
            None => Vec::new(),
        };
        self.compose(text, now, provided)
    }

    /// The results of the providers that answer at once (the calculator, conversions, applications, the web search),
    /// ranked together with `provided`, the sections the slower providers gave (the file search,
    /// [`file_sections`]), which take their usage bonus and their section limits here.
    pub fn compose(&self, text: &str, now: f64, provided: Vec<Section>) -> Results {
        let q = Query::new(text);
        let mut results = Results {
            query: text.to_string(),
            sections: Vec::new(),
        };
        if q.is_empty() {
            let items: Vec<ResultItem> = self
                .frecency
                .top(self.config.suggestions * 3, now)
                .into_iter()
                .filter(|(k, _)| k.starts_with("app:") && !k.contains('#'))
                .filter_map(|(k, s)| {
                    let c = self.apps.iter().find(|c| c.key == k)?;
                    Some(self.app_item(c, s, Vec::new()))
                })
                .take(self.config.suggestions)
                .collect();
            if !items.is_empty() {
                results.sections.push(Section {
                    kind: SectionKind::Suggestions,
                    title: SectionKind::Suggestions.title().into(),
                    items,
                });
            }
            return results;
        }
        let qn = q.normalized();
        let mut sections: Vec<Section> = Vec::new();

        if self.config.show_calculator {
            if let Some(c) = convert::convert(text) {
                let item = ResultItem {
                    id: "convert".into(),
                    kind: ItemKind::Conversion,
                    title: c.text.clone(),
                    subtitle: format!("{} =", c.from),
                    icon: None,
                    score: f64::MAX,
                    highlights: Vec::new(),
                    action: Action::CopyText(c.text),
                };
                results.sections.push(Section {
                    kind: SectionKind::Conversion,
                    title: SectionKind::Conversion.title().into(),
                    items: vec![item],
                });
            } else if let Some(c) = calc::evaluate(text) {
                let item = ResultItem {
                    id: "calc".into(),
                    kind: ItemKind::Calculator,
                    title: c.text.clone(),
                    subtitle: c.expression.clone(),
                    icon: None,
                    score: f64::MAX,
                    highlights: Vec::new(),
                    action: Action::CopyText(c.text),
                };
                results.sections.push(Section {
                    kind: SectionKind::Calculator,
                    title: SectionKind::Calculator.title().into(),
                    items: vec![item],
                });
            }
        }

        let mut apps: Vec<(f64, f64, &AppCandidate, Vec<usize>)> = Vec::new();
        for c in &self.apps {
            let Some(s) = matcher::score(&q, &c.candidate, &self.config.weights) else {
                continue;
            };
            if s.score < self.config.min_app_score {
                continue;
            }
            let total = s.score + self.frecency.bonus(&c.key, &qn, now);
            apps.push((total, s.score, c, s.positions));
        }
        apps.sort_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then_with(|| {
                    self.entries[a.2.entry]
                        .name
                        .chars()
                        .count()
                        .cmp(&self.entries[b.2.entry].name.chars().count())
                })
                .then_with(|| {
                    self.entries[a.2.entry]
                        .name
                        .cmp(&self.entries[b.2.entry].name)
                })
                .then_with(|| a.2.key.cmp(&b.2.key))
        });
        let items: Vec<ResultItem> = apps
            .into_iter()
            .take(self.config.max_apps)
            .map(|(total, _, c, pos)| self.app_item(c, total, pos))
            .collect();
        if !items.is_empty() {
            sections.push(Section {
                kind: SectionKind::Applications,
                title: SectionKind::Applications.title().into(),
                items,
            });
        }

        sections.extend(provided.into_iter().filter_map(|mut section| {
            // a provider's matches take the usage bonus here, on the loop, where the history is
            for item in &mut section.items {
                item.score += self.frecency.bonus(&item.id, &qn, now);
            }
            let max = match section.kind {
                SectionKind::Folders => self.config.max_folders,
                SectionKind::Files => self.config.max_files,
                _ => usize::MAX,
            };
            section.items.sort_by(|a, b| {
                b.score
                    .total_cmp(&a.score)
                    .then_with(|| a.title.cmp(&b.title))
            });
            section.items.truncate(max);
            (!section.items.is_empty()).then_some(section)
        }));

        if let Some(url) = &self.config.web_search_url {
            let target = url.replace("{}", &percent_encode(text.trim()));
            sections.push(Section {
                kind: SectionKind::Web,
                title: SectionKind::Web.title().into(),
                items: vec![ResultItem {
                    id: "web".into(),
                    kind: ItemKind::WebSearch,
                    title: format!("Search the web for “{}”", text.trim()),
                    subtitle: String::new(),
                    icon: None,
                    score: 0.0,
                    highlights: Vec::new(),
                    action: Action::OpenUrl(target),
                }],
            });
        }

        let rank = |s: &Section| {
            (
                s.kind != SectionKind::Web,
                s.items.first().map_or(0.0, |i| i.score),
            )
        };
        sections.sort_by(|a, b| {
            let (ra, rb) = (rank(a), rank(b));
            rb.0.cmp(&ra.0)
                .then(rb.1.total_cmp(&ra.1))
                .then((a.kind as u8).cmp(&(b.kind as u8)))
        });
        if self.config.top_hit {
            if let Some(first) = sections.first_mut() {
                if matches!(
                    first.kind,
                    SectionKind::Applications | SectionKind::Files | SectionKind::Folders
                ) && !first.items.is_empty()
                {
                    let hit = first.items.remove(0);
                    let empty = first.items.is_empty();
                    let top = Section {
                        kind: SectionKind::TopHit,
                        title: SectionKind::TopHit.title().into(),
                        items: vec![hit],
                    };
                    if empty {
                        sections.remove(0);
                    }
                    sections.insert(0, top);
                }
            }
        }
        results.sections.extend(sections);
        results
    }

    fn open_with(&self, target: &str) -> Option<CommandSpec> {
        let (prog, rest) = self.config.open_command.split_first()?;
        let mut args = rest.to_vec();
        args.push(target.to_string());
        Some(CommandSpec {
            program: prog.clone(),
            args,
            ..Default::default()
        })
    }

    fn command_for(&self, item: &ResultItem) -> Option<CommandSpec> {
        match &item.action {
            Action::LaunchApp { desktop_id, action } => {
                let e = self.entry(desktop_id)?;
                let spec = match action {
                    None => exec::command_for_entry(e, &[])?,
                    Some(a) => exec::command_for_action(e, e.actions.iter().find(|x| &x.id == a)?)?,
                };
                Some(if e.terminal {
                    spec.in_terminal(&self.config.terminal_command)
                } else {
                    spec
                })
            }
            Action::OpenPath(p) => self.open_with(&p.to_string_lossy()),
            Action::OpenUrl(u) => self.open_with(u),
            Action::CopyText(_) => None,
        }
    }

    /// The actions `item` offers, its own activation first: Open, Reveal in Files and Copy Path for a file; Open and
    /// Copy Path for a folder; Open and each desktop action for an application; Copy for a calculation or a
    /// conversion; Search for the web search.
    pub fn actions(&self, item: &ResultItem) -> Vec<ItemAction> {
        let mut actions = Vec::new();
        match item.kind {
            ItemKind::Calculator | ItemKind::Conversion => {
                actions.push(ItemAction::new("Copy", ActionKind::Default))
            }
            ItemKind::WebSearch => actions.push(ItemAction::new("Search", ActionKind::Default)),
            ItemKind::ApplicationAction => {
                actions.push(ItemAction::new("Open", ActionKind::Default))
            }
            ItemKind::Application => {
                actions.push(ItemAction::new("Open", ActionKind::Default));
                if let Action::LaunchApp { desktop_id, .. } = &item.action {
                    if let Some(entry) = self.entry(desktop_id) {
                        actions.extend(entry.actions.iter().map(|a| ItemAction {
                            label: a.name.clone(),
                            kind: ActionKind::Desktop(a.id.clone()),
                        }));
                    }
                }
            }
            ItemKind::File => {
                actions.push(ItemAction::new("Open", ActionKind::Default));
                actions.push(ItemAction::new("Reveal in Files", ActionKind::Reveal));
                actions.push(ItemAction::new("Copy Path", ActionKind::CopyPath));
            }
            ItemKind::Folder => {
                actions.push(ItemAction::new("Open", ActionKind::Default));
                actions.push(ItemAction::new("Copy Path", ActionKind::CopyPath));
            }
        }
        actions
    }

    /// The folder Reveal opens for `path`: the path itself when it is a folder, else the folder holding it.
    pub fn reveal_folder(path: &std::path::Path) -> PathBuf {
        if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| path.to_path_buf())
        }
    }

    /// The command that opens the folder of `path` in the session's file manager ([`FILE_MANAGER_ID`]) when it is
    /// installed, else with the open command.
    pub fn reveal_command(&self, path: &std::path::Path) -> Option<CommandSpec> {
        let folder = Self::reveal_folder(path).to_string_lossy().into_owned();
        match self.entry(FILE_MANAGER_ID) {
            Some(entry) => exec::command_for_entry(entry, &[folder]),
            None => self.open_with(&folder),
        }
    }

    /// Carry out `action` of `item`: its own activation as [`Launcher::activate`] does; Reveal opens its folder in the
    /// file manager; Copy Path copies its path; a desktop action launches it. Frecency is recorded for every action
    /// that opens something.
    pub fn activate_action(
        &mut self,
        item: &ResultItem,
        action: &ItemAction,
        query: &str,
        now: f64,
    ) -> Activation {
        match (&action.kind, &item.action) {
            (ActionKind::Default, _) => self.activate(item, query, now),
            (ActionKind::Reveal, Action::OpenPath(path)) => match self.reveal_command(path) {
                Some(spec) => {
                    self.frecency
                        .record(&item.id, &Query::new(query).normalized(), now);
                    Activation::Run(spec)
                }
                None => Activation::Nothing,
            },
            (ActionKind::CopyPath, Action::OpenPath(path)) => {
                Activation::CopyToClipboard(path.to_string_lossy().into_owned())
            }
            (ActionKind::Desktop(id), Action::LaunchApp { desktop_id, .. }) => {
                let launch = ResultItem {
                    id: format!("app:{desktop_id}#{id}"),
                    action: Action::LaunchApp {
                        desktop_id: desktop_id.clone(),
                        action: Some(id.clone()),
                    },
                    ..item.clone()
                };
                self.activate(&launch, query, now)
            }
            _ => Activation::Nothing,
        }
    }

    /// Activate a result: records frecency for the item and returns what to do.
    pub fn activate(&mut self, item: &ResultItem, query: &str, now: f64) -> Activation {
        if let Action::CopyText(t) = &item.action {
            return Activation::CopyToClipboard(t.clone());
        }
        let Some(mut cmd) = self.command_for(item) else {
            return Activation::Nothing;
        };
        let desktop_id = match &item.action {
            Action::LaunchApp { desktop_id, .. } => Some(desktop_id.clone()),
            _ => None,
        };
        // the application itself, not one of its desktop actions: it is brought forward when it runs
        let opens = match &item.action {
            Action::LaunchApp {
                desktop_id,
                action: None,
            } => Some(desktop_id.clone()),
            _ => None,
        };
        if let Some(hook) = self.token_hook.as_mut() {
            if let Some(t) = hook(&LaunchRequest {
                desktop_id,
                title: item.title.clone(),
            }) {
                cmd = cmd.with_activation_token(&t);
            }
        }
        self.frecency
            .record(&item.id, &Query::new(query).normalized(), now);
        match opens {
            Some(desktop_id) => Activation::OpenApp {
                desktop_id,
                spec: cmd,
            },
            None => Activation::Run(cmd),
        }
    }
}

#[cfg(test)]
mod tests;

/// Fixtures shared with the tests of other modules.
#[cfg(test)]
pub(crate) mod tests_support {
    pub use super::tests::{app, test_launcher};
}
