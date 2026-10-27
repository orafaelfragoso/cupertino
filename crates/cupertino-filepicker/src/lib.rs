//! The model of a file chooser. See the README.

use std::path::{Path, PathBuf};

use cupertino_typeahead::TypeAhead;

/// What the picker chooses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A folder: only folders are listed.
    Folder,
    /// A picture: folders, and the PNG and JPEG files in them.
    Photo,
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

/// An open chooser.
#[derive(Clone, Debug)]
pub struct Picker {
    mode: Mode,
    dir: PathBuf,
    entries: Vec<Entry>,
    highlighted: Option<usize>,
    hidden: bool,
    unreadable: bool,
    type_ahead: TypeAhead,
}

fn is_picture(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg"))
}

impl Picker {
    /// A chooser showing `start`, or its nearest existing parent folder, in `mode`.
    pub fn open(start: &Path, mode: Mode) -> Self {
        let mut dir = start.to_path_buf();
        while !dir.is_dir() && dir.pop() {}
        let mut picker = Picker {
            mode,
            dir,
            entries: Vec::new(),
            highlighted: None,
            hidden: false,
            unreadable: false,
            type_ahead: TypeAhead::default(),
        };
        picker.list();
        picker
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

    /// The last folder asked for could not be read; the open one stayed.
    pub fn unreadable(&self) -> bool {
        self.unreadable
    }

    /// Whether names that start with a dot are listed.
    pub fn set_hidden(&mut self, shown: bool) {
        if self.hidden != shown {
            self.hidden = shown;
            self.list();
        }
    }

    /// Read the open folder again, keeping the highlight when its entry is still there.
    fn list(&mut self) {
        let kept = self
            .highlighted
            .and_then(|i| self.entries.get(i))
            .map(|e| e.path.clone());
        let mut entries: Vec<Entry> = match std::fs::read_dir(&self.dir) {
            Ok(read) => read
                .filter_map(Result::ok)
                .filter_map(|e| {
                    let name = e.file_name().to_str()?.to_string();
                    let is_dir = e.path().is_dir();
                    let listed = (self.hidden || !name.starts_with('.'))
                        && (is_dir || (self.mode == Mode::Photo && is_picture(&name)));
                    listed.then(|| Entry {
                        name,
                        path: e.path(),
                        is_dir,
                    })
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        entries.sort_by_cached_key(|e| (!e.is_dir, e.name.to_lowercase(), e.name.clone()));
        self.entries = entries;
        self.highlighted = kept.and_then(|p| self.entries.iter().position(|e| e.path == p));
        self.type_ahead = TypeAhead::default();
    }

    /// Open `dir`; a folder that cannot be read leaves the picker where it was.
    fn open_dir(&mut self, dir: PathBuf) -> Outcome {
        if std::fs::read_dir(&dir).is_err() {
            self.unreadable = true;
            return Outcome::Changed;
        }
        self.unreadable = false;
        let from = std::mem::replace(&mut self.dir, dir);
        self.highlighted = None;
        self.list();
        // coming back up highlights the folder that was left
        self.highlighted = self.entries.iter().position(|e| e.path == from);
        Outcome::Changed
    }

    /// Open the parent folder.
    pub fn parent(&mut self) -> Outcome {
        match self.dir.parent().map(Path::to_path_buf) {
            Some(parent) => self.open_dir(parent),
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
            self.open_dir(entry.path)
        } else {
            Outcome::Choose(entry.path)
        }
    }

    /// What the Choose button picks: in `Folder` mode the highlighted folder, else the open one; in `Photo` mode the
    /// highlighted file.
    pub fn choice(&self) -> Option<PathBuf> {
        let highlighted = self.highlighted.and_then(|i| self.entries.get(i));
        match (self.mode, highlighted) {
            (Mode::Folder, Some(entry)) => Some(entry.path.clone()),
            (Mode::Folder, None) => Some(self.dir.clone()),
            (Mode::Photo, Some(entry)) if !entry.is_dir => Some(entry.path.clone()),
            (Mode::Photo, _) => None,
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
        assert_eq!(
            names(&shown),
            [".secret", "art", "Beach", ".hidden.png", "a.jpg", "b.PNG"]
        );
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
        assert_eq!(p.dir(), d.path().join("art"));
        assert_eq!(names(&p), ["deep", "c.jpeg"]);
        assert_eq!(p.highlighted(), None);
    }

    #[test]
    fn super_up_opens_the_parent_and_highlights_the_folder_that_was_left() {
        let d = tree();
        let mut p = Picker::open(&d.path().join("art/deep"), Mode::Photo);
        assert_eq!(p.key(Key::Parent, 0.0), Outcome::Changed);
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
}
