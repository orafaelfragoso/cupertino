//! The model of a file chooser. See the crate README, *The file chooser's model*.

use std::path::{Path, PathBuf};

use cupertino_ui_controls::typeahead::TypeAhead;

use crate::entry::Entry;
use crate::model::{enumerate_blocking, ModelOptions};

/// What the picker chooses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// A folder: only folders are listed.
    Folder,
    /// A picture: folders, and the PNG and JPEG files in them.
    Photo,
    /// A program: folders, and the files in them that may be executed.
    Program,
    /// Any file: folders and every file in them; the caller filters what it shows.
    File,
}

/// A key of the chooser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Home,
    End,
    /// Return: enter the highlighted folder or choose the highlighted file.
    Return,
    /// Super+Return and the Choose button.
    Choose,
    /// Super+Up: the parent folder.
    Parent,
    Escape,
    Type(char),
}

/// What a key or a click did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The listing, the highlight or the folder changed: draw it again.
    Changed,
    Choose(PathBuf),
    Cancel,
    Ignore,
}

/// How a folder is listed: the folder, the mode and whether dotfiles show; `None` when it cannot be read.
pub type Reader = dyn Fn(&Path, Mode, bool) -> Option<Vec<Entry>>;

/// A folder to list: what a worker is asked to read.
#[derive(Clone, Debug)]
pub struct Request {
    dir: PathBuf,
    mode: Mode,
    hidden: bool,
    /// The folder may not exist (the start of a chooser): list the nearest parent that does.
    nearest_parent: bool,
}

/// What a worker read: `None` when the folder could not be read.
#[derive(Clone, Debug)]
pub struct Listing {
    requested: Request,
    result: Option<(PathBuf, Vec<Entry>)>,
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The items of `dir` `mode` lists, in the family's order (by name, as Files lists a folder); `None` when it cannot be
/// read.
fn read_entries(dir: &Path, mode: Mode, hidden: bool) -> Option<Vec<Entry>> {
    let options = ModelOptions {
        show_hidden: hidden,
        watch: false,
        read_tags: false,
        ..ModelOptions::default()
    };
    let mut entries = enumerate_blocking(dir, &options).ok()?;
    entries.retain(|e| {
        e.is_dir
            || match mode {
                Mode::Folder => false,
                Mode::Photo => is_picture(&e.name),
                Mode::Program => is_executable(&e.path),
                Mode::File => true,
            }
    });
    Some(entries)
}

impl Listing {
    /// The key of the request this answers ([`Request::key`]).
    pub fn key(&self) -> (PathBuf, bool, Mode) {
        self.requested.key()
    }
}

impl Request {
    /// What this request asks for: the folder, whether dotfiles show and what is listed. Two requests with one key have one answer, so
    /// a caller may run the second one's job from the first's answer and need not start another read of a folder that
    /// is slow to answer.
    pub fn key(&self) -> (PathBuf, bool, Mode) {
        (self.dir.clone(), self.hidden, self.mode)
    }

    /// Read the folder. This is the only place the filesystem is touched: run it on a worker, a folder on a hung
    /// mount may never answer.
    pub fn run(&self) -> Listing {
        self.run_with(&read_entries)
    }

    /// [`Request::run`] with `read` as the way to list one folder (tests).
    pub fn run_with(&self, read: &Reader) -> Listing {
        let mut dir = self.dir.clone();
        loop {
            if let Some(entries) = read(&dir, self.mode, self.hidden) {
                return Listing {
                    requested: self.clone(),
                    result: Some((dir, entries)),
                };
            }
            if !self.nearest_parent || !dir.pop() {
                return Listing {
                    requested: self.clone(),
                    result: None,
                };
            }
        }
    }
}

/// An open chooser. It never reads the filesystem itself: a change of folder makes a [`Request`] to run elsewhere
/// (`take_request`, then `apply` the [`Listing`]), and the picker shows what it has until the answer comes.
#[derive(Clone, Debug)]
pub struct Picker {
    mode: Mode,
    dir: PathBuf,
    entries: Vec<Entry>,
    highlighted: Option<usize>,
    hidden: bool,
    unreadable: bool,
    type_ahead: TypeAhead,
    pending: Option<Request>,
    /// The key of the latest request: the only answer taken.
    wanted: Option<(PathBuf, bool, Mode)>,
    loading: bool,
    /// The entry to highlight when the listing in flight arrives.
    then_highlight: Option<PathBuf>,
}

fn is_picture(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg"))
}

impl Picker {
    /// A chooser that will show `start`, or its nearest existing parent folder, in `mode`; it lists nothing until its
    /// first request is answered.
    pub fn new(start: &Path, mode: Mode) -> Self {
        let mut picker = Picker {
            mode,
            dir: start.to_path_buf(),
            entries: Vec::new(),
            highlighted: None,
            hidden: false,
            unreadable: false,
            type_ahead: TypeAhead::default(),
            pending: None,
            wanted: None,
            loading: false,
            then_highlight: None,
        };
        picker.request(start.to_path_buf(), true, None);
        picker
    }

    /// [`Picker::new`] with the first listing read here, for callers with nothing to keep responsive.
    pub fn open(start: &Path, mode: Mode) -> Self {
        let mut picker = Picker::new(start, mode);
        picker.settle();
        picker
    }

    /// Run the requests in flight here, one after the other, until the picker has nothing to wait for.
    pub fn settle(&mut self) {
        while let Some(request) = self.take_request() {
            let listing = request.run();
            self.apply(listing);
        }
    }

    fn request(&mut self, dir: PathBuf, nearest_parent: bool, then_highlight: Option<PathBuf>) {
        self.loading = true;
        self.then_highlight = then_highlight;
        let request = Request {
            dir,
            mode: self.mode,
            hidden: self.hidden,
            nearest_parent,
        };
        self.wanted = Some(request.key());
        self.pending = Some(request);
    }

    /// The folder to list next, once; the answer goes to [`Picker::apply`].
    pub fn take_request(&mut self) -> Option<Request> {
        self.pending.take()
    }

    /// Take a worker's answer; one that is not for the latest request's folder (and dotfile choice) is dropped.
    pub fn apply(&mut self, listing: Listing) -> Outcome {
        if self.wanted.as_ref() != Some(&listing.requested.key()) {
            return Outcome::Ignore;
        }
        self.loading = false;
        match listing.result {
            Some((dir, entries)) => {
                self.unreadable = false;
                self.dir = dir;
                self.entries = entries;
                self.highlighted = self
                    .then_highlight
                    .take()
                    .and_then(|p| self.entries.iter().position(|e| e.path == p));
                self.type_ahead = TypeAhead::default();
            }
            None => self.unreadable = true,
        }
        Outcome::Changed
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The open folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn highlighted(&self) -> Option<usize> {
        self.highlighted
    }

    /// A listing is on its way.
    pub fn loading(&self) -> bool {
        self.loading
    }

    /// The last folder asked for could not be read; the open one stayed.
    pub fn unreadable(&self) -> bool {
        self.unreadable
    }

    /// Whether names that start with a dot are listed.
    pub fn set_hidden(&mut self, shown: bool) {
        if self.hidden != shown {
            self.hidden = shown;
            let kept = self
                .highlighted
                .and_then(|i| self.entries.get(i))
                .map(|e| e.path.clone());
            self.request(self.dir.clone(), false, kept);
        }
    }

    /// Open `dir`; the open folder stays until the listing arrives, and a folder that cannot be read leaves it.
    fn open_dir(&mut self, dir: PathBuf, then_highlight: Option<PathBuf>) -> Outcome {
        self.request(dir, false, then_highlight);
        Outcome::Changed
    }

    /// Open `dir`, a place of a sidebar.
    pub fn go(&mut self, dir: &Path) -> Outcome {
        self.open_dir(dir.to_path_buf(), None)
    }

    /// Open the parent folder, highlighting the folder that was left.
    pub fn parent(&mut self) -> Outcome {
        match self.dir.parent().map(Path::to_path_buf) {
            Some(parent) => {
                let from = self.dir.clone();
                self.open_dir(parent, Some(from))
            }
            None => Outcome::Ignore,
        }
    }

    /// Highlight row `i`.
    pub fn highlight(&mut self, i: usize) -> Outcome {
        if i < self.entries.len() && self.highlighted != Some(i) {
            self.highlighted = Some(i);
            Outcome::Changed
        } else {
            Outcome::Ignore
        }
    }

    /// What Return does to row `i`: open a folder, choose a file.
    pub fn activate(&mut self, i: usize) -> Outcome {
        let Some(entry) = self.entries.get(i).cloned() else {
            return Outcome::Ignore;
        };
        if entry.is_dir {
            self.open_dir(entry.path, None)
        } else {
            Outcome::Choose(entry.path)
        }
    }

    /// What the Choose button picks: in `Folder` mode the highlighted folder, else the open one; in the other modes the
    /// highlighted file.
    pub fn choice(&self) -> Option<PathBuf> {
        let highlighted = self.highlighted.and_then(|i| self.entries.get(i));
        match (self.mode, highlighted) {
            (Mode::Folder, Some(entry)) => Some(entry.path.clone()),
            (Mode::Folder, None) => Some(self.dir.clone()),
            (Mode::Photo | Mode::Program | Mode::File, Some(entry)) if !entry.is_dir => {
                Some(entry.path.clone())
            }
            (Mode::Photo | Mode::Program | Mode::File, _) => None,
        }
    }

    /// The result of `key` at `now` seconds.
    pub fn key(&mut self, key: Key, now: f64) -> Outcome {
        let last = self.entries.len().saturating_sub(1);
        let move_to = |picker: &mut Picker, to: usize| {
            if picker.entries.is_empty() {
                Outcome::Ignore
            } else {
                picker.highlight(to.min(last))
            }
        };
        match key {
            Key::Up => move_to(self, self.highlighted.map_or(last, |i| i.saturating_sub(1))),
            Key::Down => move_to(self, self.highlighted.map_or(0, |i| i + 1)),
            Key::Home => move_to(self, 0),
            Key::End => move_to(self, last),
            Key::Return => match self.highlighted {
                Some(i) => self.activate(i),
                None => Outcome::Ignore,
            },
            Key::Choose => self.choice().map_or(Outcome::Ignore, Outcome::Choose),
            Key::Parent => self.parent(),
            Key::Escape => Outcome::Cancel,
            Key::Type(c) => {
                let titles: Vec<Option<&str>> =
                    self.entries.iter().map(|e| Some(e.name.as_str())).collect();
                let from = self.highlighted.unwrap_or(last);
                match self.type_ahead.find(c, now, from, &titles) {
                    Some(i) => self.highlight(i),
                    None => Outcome::Ignore,
                }
            }
        }
    }
}
