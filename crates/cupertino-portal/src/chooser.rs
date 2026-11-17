//! One file-chooser dialog without drawing: the folder it shows, the filter, the selection, the name of a save and the
//! choices, and the [`Answer`] they make.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cupertino_filepicker::{Entry, Listing, Mode, Outcome, Picker, Request};

use crate::file_chooser::{Answer, Ask, Choice, Filter, Kind, Pattern};

/// What the accept button did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Accept {
    /// The dialog is done.
    Answer(Answer),
    /// The save would replace `0`: ask first, then [`Chooser::replace`].
    Replace(PathBuf),
    /// Nothing to accept yet (no file selected, or no name).
    Nothing,
}

/// One open dialog.
#[derive(Clone, Debug)]
pub struct Chooser {
    ask: Ask,
    picker: Picker,
    filter: Option<usize>,
    /// The indices of the picker's entries the filter lets through, worked out once for each listing and filter.
    shown: Vec<usize>,
    selection: BTreeSet<PathBuf>,
    name: String,
    choices: Vec<String>,
}

/// One piece of a glob: each but [`Token::Star`] matches exactly one character.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Star,
    Any,
    Char(char),
    Set {
        negated: bool,
        items: Vec<(char, char)>,
    },
}

impl Token {
    fn takes(&self, c: char) -> bool {
        match self {
            Token::Star | Token::Any => true,
            Token::Char(p) => *p == c,
            Token::Set { negated, items } => {
                items.iter().any(|(lo, hi)| (*lo..=*hi).contains(&c)) != *negated
            }
        }
    }
}

/// The tokens of `pattern`, a run of `*` as one; a `[` without its `]` is a literal.
fn tokens(pattern: &[char]) -> Vec<Token> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < pattern.len() {
        let token = match pattern[i] {
            '*' => Token::Star,
            '?' => Token::Any,
            '[' => match pattern[i + 1..].iter().skip(1).position(|&c| c == ']') {
                Some(close) => {
                    let body = &pattern[i + 1..i + 2 + close];
                    i += 2 + close;
                    let (negated, body) = match body.split_first() {
                        Some(('!' | '^', rest)) => (true, rest),
                        _ => (false, body),
                    };
                    let mut items = Vec::new();
                    let mut j = 0;
                    while j < body.len() {
                        if j + 2 < body.len() && body[j + 1] == '-' {
                            items.push((body[j], body[j + 2]));
                            j += 3;
                        } else {
                            items.push((body[j], body[j]));
                            j += 1;
                        }
                    }
                    Token::Set { negated, items }
                }
                None => Token::Char('['),
            },
            c => Token::Char(c),
        };
        i += 1;
        if !(token == Token::Star && out.last() == Some(&Token::Star)) {
            out.push(token);
        }
    }
    out
}

/// Whether `name` matches the shell glob `pattern` (`*`, `?` and `[…]`), without regard to case. It takes at most
/// pattern × name steps: a mismatch goes back only to the latest `*`, which is enough because every other token takes
/// exactly one character.
pub fn glob(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let tokens = tokens(&pattern);
    let name: Vec<char> = name.to_lowercase().chars().collect();
    let (mut t, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match tokens.get(t) {
            Some(Token::Star) => {
                star = Some((t, n));
                t += 1;
            }
            Some(token) if token.takes(name[n]) => {
                t += 1;
                n += 1;
            }
            _ => match star {
                Some((st, sn)) => {
                    star = Some((st, sn + 1));
                    t = st + 1;
                    n = sn + 1;
                }
                None => return false,
            },
        }
    }
    tokens[t..].iter().all(|token| *token == Token::Star)
}

/// Whether the file `name` matches the MIME type `mime` (`image/png`, `image/*` or `*/*`) by its extension.
pub fn mime(mime: &str, name: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    let (kind, sub) = mime.split_once('/').unwrap_or((mime.as_str(), "*"));
    mime_guess::from_path(name).iter().any(|guess| {
        (kind == "*" || guess.type_().as_str() == kind)
            && (sub == "*" || guess.subtype().as_str() == sub)
    })
}

impl Filter {
    /// Whether a file named `name` is shown under this filter.
    pub fn matches(&self, name: &str) -> bool {
        self.patterns.iter().any(|pattern| match pattern {
            Pattern::Glob(pattern) => glob(pattern, name),
            Pattern::Mime(pattern) => mime(pattern, name),
        })
    }
}

impl Choice {
    /// Whether `value` is one of this choice's options, or `true` or `false` for a check box.
    pub fn takes(&self, value: &str) -> bool {
        if self.options.is_empty() {
            matches!(value, "true" | "false")
        } else {
            self.options.iter().any(|(id, _)| id == value)
        }
    }

    /// The value it starts at: its initial value when it takes it, else its first option, else unchecked.
    fn start(&self) -> String {
        if self.takes(&self.initial) {
            self.initial.clone()
        } else {
            self.options
                .first()
                .map_or_else(|| "false".to_owned(), |(id, _)| id.clone())
        }
    }
}

impl Chooser {
    /// The dialog for `ask`, in its folder: `current_folder`, else the folder of `current_file`, else `home`. It lists
    /// nothing until its first [`Chooser::take_request`] is answered.
    pub fn new(ask: Ask, home: &Path) -> Self {
        let start = ask
            .current_folder
            .clone()
            .or_else(|| {
                ask.current_file
                    .as_deref()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
            })
            .unwrap_or_else(|| home.to_path_buf());
        let mode = if ask.directory || ask.kind == Kind::SaveFiles {
            Mode::Folder
        } else {
            Mode::File
        };
        let filter = ask
            .current_filter
            .as_ref()
            .and_then(|current| ask.filters.iter().position(|f| f == current))
            .or((!ask.filters.is_empty()).then_some(0));
        let name = ask
            .current_name
            .clone()
            .or_else(|| {
                ask.current_file
                    .as_deref()
                    .and_then(Path::file_name)
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        let choices = ask.choices.iter().map(Choice::start).collect();
        Self {
            picker: Picker::new(&start, mode),
            ask,
            filter,
            shown: Vec::new(),
            selection: BTreeSet::new(),
            name,
            choices,
        }
    }

    fn refilter(&mut self) {
        let filter = self.filter.and_then(|i| self.ask.filters.get(i));
        self.shown = (self.picker.entries().iter().enumerate())
            .filter(|(_, e)| e.is_dir || filter.is_none_or(|f| f.matches(&e.name)))
            .map(|(i, _)| i)
            .collect();
        let shown: BTreeSet<PathBuf> = self.entries().iter().map(|e| e.path.clone()).collect();
        self.selection.retain(|p| shown.contains(p));
    }

    pub fn ask(&self) -> &Ask {
        &self.ask
    }

    /// The open folder and whether its listing is on its way or could not be read.
    pub fn picker(&self) -> &Picker {
        &self.picker
    }

    /// The rows shown: the folders, and the files the filter lets through.
    pub fn entries(&self) -> Vec<&Entry> {
        let entries = self.picker.entries();
        self.shown.iter().filter_map(|&i| entries.get(i)).collect()
    }

    pub fn filter(&self) -> Option<usize> {
        self.filter
    }

    /// Show the filter at `index`; the selection keeps only what it still shows.
    pub fn set_filter(&mut self, index: usize) {
        if index < self.ask.filters.len() {
            self.filter = Some(index);
            self.refilter();
        }
    }

    pub fn choice(&self, index: usize) -> Option<&str> {
        self.choices.get(index).map(String::as_str)
    }

    /// Set choice `index` to `value`, when it takes it.
    pub fn set_choice(&mut self, index: usize, value: &str) {
        if self.ask.choices.get(index).is_some_and(|c| c.takes(value)) {
            self.choices[index] = value.to_owned();
        }
    }

    /// The name a save writes.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: String) {
        self.name = name;
    }

    /// Of `name`, the part a Save As field selects first: the stem, not the extension.
    pub fn stem_len(name: &str) -> usize {
        match name.rfind('.') {
            Some(0) | None => name.len(),
            Some(dot) => dot,
        }
    }

    pub fn selection(&self) -> &BTreeSet<PathBuf> {
        &self.selection
    }

    /// Select these rows of [`Chooser::entries`]; a dialog that picks one keeps the last.
    pub fn select(&mut self, rows: impl IntoIterator<Item = usize>) {
        let entries = self.entries();
        let mut paths: Vec<PathBuf> = rows
            .into_iter()
            .filter_map(|row| entries.get(row).map(|e| e.path.clone()))
            .collect();
        if !self.ask.multiple {
            paths.drain(..paths.len().saturating_sub(1));
        }
        self.selection = paths.into_iter().collect();
    }

    /// What a double click or Return on `row` does: a folder opens, a file is chosen as the accept button would.
    pub fn open(&mut self, row: usize) -> Accept {
        let Some(entry) = self.entries().get(row).map(|e| (*e).clone()) else {
            return Accept::Nothing;
        };
        if entry.is_dir {
            self.go(&entry.path);
            return Accept::Nothing;
        }
        match self.ask.kind {
            Kind::Open if !self.ask.directory => {
                self.selection = BTreeSet::from([entry.path]);
                self.accept()
            }
            Kind::Save => {
                self.name = entry.name;
                self.accept()
            }
            Kind::Open | Kind::SaveFiles => Accept::Nothing,
        }
    }

    /// Open the folder `dir`, a place of the sidebar or a row.
    pub fn go(&mut self, dir: &Path) {
        self.picker.go(dir);
    }

    pub fn parent(&mut self) {
        self.picker.parent();
    }

    /// The folder to list next, to read on a worker.
    pub fn take_request(&mut self) -> Option<Request> {
        self.picker.take_request()
    }

    /// Take a worker's listing; the selection empties when the folder changes.
    pub fn apply(&mut self, listing: Listing) {
        let before = self.picker.dir().to_path_buf();
        if self.picker.apply(listing) == Outcome::Changed {
            if self.picker.dir() != before {
                self.selection.clear();
            }
            self.refilter();
        }
    }

    /// Read the folders asked for here, for callers with nothing to keep responsive.
    pub fn settle(&mut self) {
        while let Some(request) = self.take_request() {
            let listing = request.run();
            self.apply(listing);
        }
    }

    /// The label of the accept button.
    pub fn accept_label(&self) -> String {
        if let Some(label) = &self.ask.accept_label {
            return label.replace('_', "");
        }
        match self.ask.kind {
            Kind::Open => "Open",
            Kind::Save | Kind::SaveFiles => "Save",
        }
        .to_owned()
    }

    fn chosen(&self, paths: Vec<PathBuf>) -> Answer {
        Answer::Chosen {
            paths,
            filter: self.filter.and_then(|i| self.ask.filters.get(i)).cloned(),
            choices: self
                .ask
                .choices
                .iter()
                .zip(&self.choices)
                .map(|(choice, value)| (choice.id.clone(), value.clone()))
                .collect(),
        }
    }

    fn selected(&self, dirs: bool) -> Vec<PathBuf> {
        let entries = self.entries();
        self.selection
            .iter()
            .filter(|p| entries.iter().any(|e| e.is_dir == dirs && e.path == **p))
            .cloned()
            .collect()
    }

    /// What the accept button does now.
    pub fn accept(&self) -> Accept {
        let dir = self.picker.dir().to_path_buf();
        match self.ask.kind {
            Kind::Open if self.ask.directory => match self.selected(true) {
                dirs if dirs.is_empty() => Accept::Answer(self.chosen(vec![dir])),
                dirs => Accept::Answer(self.chosen(dirs)),
            },
            Kind::Open => match self.selected(false) {
                files if files.is_empty() => Accept::Nothing,
                files => Accept::Answer(self.chosen(files)),
            },
            Kind::Save => {
                let name = self.name.trim();
                if name.is_empty() || name == "." || name == ".." || name.contains('/') {
                    return Accept::Nothing;
                }
                let path = dir.join(name);
                if self.picker.entries().iter().any(|e| e.path == path) {
                    Accept::Replace(path)
                } else {
                    Accept::Answer(self.chosen(vec![path]))
                }
            }
            Kind::SaveFiles => {
                let into = self.selected(true).into_iter().next().unwrap_or(dir);
                Accept::Answer(self.chosen(self.ask.files.iter().map(|f| into.join(f)).collect()))
            }
        }
    }

    /// The answer once the person agreed to replace `path`.
    pub fn replace(&self, path: PathBuf) -> Answer {
        self.chosen(vec![path])
    }
}
