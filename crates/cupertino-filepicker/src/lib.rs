//! The model of a file chooser. See the README.

use std::path::{Path, PathBuf};

use cupertino_typeahead::TypeAhead;

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

/// One row of the open folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
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

/// The entries of `dir` for `mode`, in the chooser's order; `None` when it cannot be read.
fn read_entries(dir: &Path, mode: Mode, hidden: bool) -> Option<Vec<Entry>> {
    let mut entries: Vec<Entry> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let is_dir = e.path().is_dir();
            let listed = (hidden || !name.starts_with('.'))
                && (is_dir
                    || match mode {
                        Mode::Folder => false,
                        Mode::Photo => is_picture(&name),
                        Mode::Program => is_executable(&e.path()),
                        Mode::File => true,
                    });
            listed.then(|| Entry {
                name,
                path: e.path(),
                is_dir,
            })
        })
        .collect();
    entries.sort_by_cached_key(|e| (!e.is_dir, e.name.to_lowercase(), e.name.clone()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tree() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for dir in ["Beach", "art", ".secret", "art/deep"] {
            fs::create_dir_all(d.path().join(dir)).unwrap();
        }
        for file in [
            "b.PNG",
            "a.jpg",
            "notes.txt",
            ".hidden.png",
            "art/c.jpeg",
            "art/deep/d.png",
        ] {
            fs::write(d.path().join(file), b"x").unwrap();
        }
        d
    }

    fn names(p: &Picker) -> Vec<&str> {
        p.entries().iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn folders_come_first_then_pictures_without_dotfiles_or_other_files() {
        let d = tree();
        let photo = Picker::open(d.path(), Mode::Photo);
        assert_eq!(names(&photo), ["art", "Beach", "a.jpg", "b.PNG"]);
        let folder = Picker::open(d.path(), Mode::Folder);
        assert_eq!(names(&folder), ["art", "Beach"]);
        let mut shown = Picker::open(d.path(), Mode::Photo);
        shown.set_hidden(true);
        shown.settle();
        assert_eq!(
            names(&shown),
            [".secret", "art", "Beach", ".hidden.png", "a.jpg", "b.PNG"]
        );
    }

    #[test]
    fn a_program_chooser_lists_folders_then_the_files_that_may_be_executed() {
        use std::os::unix::fs::PermissionsExt;
        let d = tree();
        let tool = d.path().join("tool");
        fs::write(&tool, b"x").unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        let mut p = Picker::open(d.path(), Mode::Program);
        assert_eq!(names(&p), ["art", "Beach", "tool"]);
        p.key(Key::End, 0.0);
        assert_eq!(p.choice(), Some(tool));
        p.key(Key::Home, 0.0);
        assert_eq!(p.choice(), None, "a folder is opened, not chosen");
    }

    #[test]
    fn a_file_chooser_lists_folders_then_every_file_and_chooses_only_a_file() {
        let d = tree();
        let mut p = Picker::open(d.path(), Mode::File);
        assert_eq!(names(&p), ["art", "Beach", "a.jpg", "b.PNG", "notes.txt"]);
        p.key(Key::End, 0.0);
        assert_eq!(p.choice(), Some(d.path().join("notes.txt")));
        p.key(Key::Home, 0.0);
        assert_eq!(p.choice(), None, "a folder is opened, not chosen");
    }

    #[test]
    fn a_start_that_does_not_exist_opens_its_nearest_parent() {
        let d = tree();
        let p = Picker::open(&d.path().join("art/missing/deeper"), Mode::Folder);
        assert_eq!(p.dir(), d.path().join("art"));
    }

    #[test]
    fn the_arrow_keys_move_without_wrapping_and_return_enters_a_folder_or_chooses_a_file() {
        let d = tree();
        let mut p = Picker::open(d.path(), Mode::Photo);
        assert_eq!(p.key(Key::Down, 0.0), Outcome::Changed);
        assert_eq!(p.highlighted(), Some(0));
        assert_eq!(p.key(Key::Up, 0.0), Outcome::Ignore, "no wrap at the top");
        assert_eq!(p.key(Key::End, 0.0), Outcome::Changed);
        assert_eq!(
            p.key(Key::Down, 0.0),
            Outcome::Ignore,
            "no wrap at the bottom"
        );
        assert_eq!(
            p.key(Key::Return, 0.0),
            Outcome::Choose(d.path().join("b.PNG"))
        );
        p.key(Key::Home, 0.0);
        assert_eq!(p.key(Key::Return, 0.0), Outcome::Changed);
        p.settle();
        assert_eq!(p.dir(), d.path().join("art"));
        assert_eq!(names(&p), ["deep", "c.jpeg"]);
        assert_eq!(p.highlighted(), None);
    }

    #[test]
    fn super_up_opens_the_parent_and_highlights_the_folder_that_was_left() {
        let d = tree();
        let mut p = Picker::open(&d.path().join("art/deep"), Mode::Photo);
        assert_eq!(p.key(Key::Parent, 0.0), Outcome::Changed);
        p.settle();
        assert_eq!(p.dir(), d.path().join("art"));
        assert_eq!(p.entries()[p.highlighted().unwrap()].name, "deep");
        let mut root = Picker::open(Path::new("/"), Mode::Folder);
        assert_eq!(
            root.key(Key::Parent, 0.0),
            Outcome::Ignore,
            "nothing above the root"
        );
    }

    #[test]
    fn the_choice_is_the_highlighted_folder_or_the_open_one_in_folder_mode_and_a_file_in_photo_mode(
    ) {
        let d = tree();
        let mut folder = Picker::open(d.path(), Mode::Folder);
        assert_eq!(
            folder.key(Key::Choose, 0.0),
            Outcome::Choose(d.path().to_path_buf())
        );
        folder.key(Key::Down, 0.0);
        assert_eq!(
            folder.key(Key::Choose, 0.0),
            Outcome::Choose(d.path().join("art"))
        );
        let mut photo = Picker::open(d.path(), Mode::Photo);
        assert_eq!(
            photo.key(Key::Choose, 0.0),
            Outcome::Ignore,
            "nothing highlighted"
        );
        photo.key(Key::Down, 0.0);
        assert_eq!(
            photo.key(Key::Choose, 0.0),
            Outcome::Ignore,
            "a folder is not a picture"
        );
        photo.key(Key::End, 0.0);
        assert_eq!(
            photo.key(Key::Choose, 0.0),
            Outcome::Choose(d.path().join("b.PNG"))
        );
        assert_eq!(photo.key(Key::Escape, 0.0), Outcome::Cancel);
    }

    #[test]
    fn typing_jumps_to_the_entry_that_starts_with_the_text() {
        let d = tree();
        let mut p = Picker::open(d.path(), Mode::Photo);
        assert_eq!(p.key(Key::Type('b'), 0.0), Outcome::Changed);
        assert_eq!(
            p.entries()[p.highlighted().unwrap()].name,
            "Beach",
            "case does not matter"
        );
        assert_eq!(p.key(Key::Type('b'), 5.0), Outcome::Changed);
        assert_eq!(
            p.entries()[p.highlighted().unwrap()].name,
            "b.PNG",
            "a new search steps on"
        );
        assert_eq!(p.key(Key::Type('z'), 9.0), Outcome::Ignore);
    }

    #[test]
    #[cfg(unix)]
    fn a_folder_that_cannot_be_read_leaves_the_picker_where_it_was() {
        use std::os::unix::fs::PermissionsExt;
        let d = tree();
        let locked = d.path().join("Beach");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let mut p = Picker::open(d.path(), Mode::Photo);
        let beach = p.entries().iter().position(|e| e.name == "Beach").unwrap();
        let unreadable = fs::read_dir(&locked).is_err();
        let outcome = p.activate(beach);
        p.settle();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if unreadable {
            assert_eq!(outcome, Outcome::Changed);
            assert!(p.unreadable());
            assert_eq!(
                p.dir(),
                d.path(),
                "the picker stays in the folder it was in"
            );
        }
    }

    #[test]
    fn a_new_picker_and_its_keys_touch_no_folder_and_a_slow_listing_does_not_hold_them_up() {
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let (started_tx, started) = std::sync::mpsc::channel::<()>();
        let mut p = Picker::new(Path::new("/a/folder/that/is/not/read/here"), Mode::Photo);
        assert!(p.loading());
        assert_eq!(p.entries().len(), 0);
        let request = p.take_request().expect("the first listing is asked for");
        assert!(p.take_request().is_none(), "a request is handed out once");
        // the worker: a reader that waits as a hung mount would
        let worker = std::thread::spawn(move || {
            request.run_with(&move |_, _, _| {
                started_tx.send(()).unwrap();
                blocked.recv().unwrap();
                Some(vec![Entry {
                    name: "late.png".into(),
                    path: "/a/late.png".into(),
                    is_dir: false,
                }])
            })
        });
        started
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        // meanwhile the picker answers keys at once
        assert_eq!(p.key(Key::Escape, 0.0), Outcome::Cancel);
        assert_eq!(
            p.key(Key::Down, 0.0),
            Outcome::Ignore,
            "nothing to move over yet"
        );
        assert!(p.loading(), "still waiting for the listing");
        release.send(()).unwrap();
        let listing = worker.join().unwrap();
        assert_eq!(p.apply(listing), Outcome::Changed);
        assert!(!p.loading());
        assert_eq!(p.entries()[0].name, "late.png");
    }

    #[test]
    fn an_answer_that_is_not_for_the_latest_request_is_dropped() {
        let d = tree();
        let mut p = Picker::new(d.path(), Mode::Folder);
        let first = p.take_request().unwrap();
        p.set_hidden(true);
        let second = p.take_request().unwrap();
        assert_eq!(
            p.apply(first.run()),
            Outcome::Ignore,
            "the first answer is stale"
        );
        assert!(p.loading());
        assert_eq!(p.apply(second.run()), Outcome::Changed);
        assert!(p.entries().iter().any(|e| e.name == ".secret"));
    }

    #[test]
    fn an_answer_for_a_folder_the_picker_asks_for_again_is_still_taken() {
        let d = tree();
        let mut p = Picker::new(d.path(), Mode::Folder);
        let first = p.take_request().unwrap();
        // the picker wants the same folder again before the first answer is in: its caller may skip a second read
        p.set_hidden(true);
        p.set_hidden(false);
        let again = p.take_request().unwrap();
        assert_eq!(first.key(), again.key());
        assert_eq!(
            p.apply(first.run()),
            Outcome::Changed,
            "one answer serves both"
        );
        assert!(!p.loading());
        assert_eq!(names(&p), ["art", "Beach"]);
    }
}
