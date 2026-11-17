//! Which modules the panel shows and in which order, and where that puts each tile.
//!
//! Pure logic. An [`Arrangement`] is the list of the shown modules; the modules it leaves out are the ones offered to
//! add. The tiles are laid on the grid in the list's order, each in the first free place that holds it, reading the
//! grid row by row, so a small tile fills a place a larger one before it left open. An arrangement always fits the
//! grid's six rows: a move that would not is refused, and a list that does not (a stored one written by hand) is
//! shown in reading order instead, which fits whatever it leaves out.
//!
//! A stored layout is the modules' names ([`Module::id`]) in order. Reading one is tolerant: a name no module has
//! and a repeated name are skipped, and a module the list does not name is not shown.

use cupertino_geometry::{Point, Rect};

use crate::layout::{COLUMN, GAP, GRID_WIDTH, ROW, ROWS};
use crate::model::Module;

/// The grid counted in circles: four across, each one row high.
const UNITS: usize = 4;

/// How many circles `module`'s tile takes across and how many rows down.
fn span(module: Module) -> (usize, usize) {
    match module {
        Module::NowPlaying => (2, 2),
        Module::Display | Module::Sound => (UNITS, 1),
        Module::DockHiding | Module::LockScreen | Module::Mute | Module::KeyboardBrightness => {
            (1, 1)
        }
        Module::WiFi | Module::Bluetooth | Module::Microphone | Module::FocusMode => (2, 1),
    }
}

/// The frame of a tile of `span` whose first circle is at `column` and `row`, from the grid's corner.
fn frame_at(column: usize, row: usize, span: (usize, usize)) -> Rect {
    let pitch = ROW + GAP;
    let width = match span.0 {
        1 => ROW,
        2 => COLUMN,
        _ => GRID_WIDTH,
    };
    Rect::new(
        column as f64 * pitch,
        row as f64 * pitch,
        width,
        span.1 as f64 * ROW + (span.1 as f64 - 1.0) * GAP,
    )
}

/// Where each module of `order` lies, and how many rows they take.
fn pack(order: &[Module]) -> (Vec<(Module, Rect)>, usize) {
    let mut taken: Vec<[bool; UNITS]> = Vec::new();
    let mut places = Vec::with_capacity(order.len());
    for &module in order {
        let (across, down) = span(module);
        let free = |taken: &Vec<[bool; UNITS]>, column: usize, row: usize| {
            (row..row + down).all(|r| {
                (column..column + across).all(|c| taken.get(r).is_none_or(|cells| !cells[c]))
            })
        };
        let (column, row) = (0..)
            .flat_map(|row| (0..=UNITS - across).map(move |column| (column, row)))
            .find(|&(column, row)| free(&taken, column, row))
            .expect("a row past the last one is free");
        if taken.len() < row + down {
            taken.resize(row + down, [false; UNITS]);
        }
        for cells in &mut taken[row..row + down] {
            cells[column..column + across].fill(true);
        }
        places.push((module, frame_at(column, row, (across, down))));
    }
    (places, taken.len())
}

/// The shown modules in their order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arrangement {
    shown: Vec<Module>,
}

impl Default for Arrangement {
    /// Every module in reading order.
    fn default() -> Self {
        Self {
            shown: Module::ALL.to_vec(),
        }
    }
}

impl Arrangement {
    /// The arrangement a stored layout of `ids` gives: the modules it names, in its order. A name no module has and
    /// a name given again are skipped. A list that does not fit the grid is put in reading order.
    pub fn from_ids<S: AsRef<str>>(ids: &[S]) -> Self {
        let mut shown = Vec::new();
        for module in ids.iter().filter_map(|id| Module::from_id(id.as_ref())) {
            if !shown.contains(&module) {
                shown.push(module);
            }
        }
        let mut arrangement = Self { shown };
        arrangement.settle();
        arrangement
    }

    /// The names of the shown modules in order: what is stored.
    pub fn ids(&self) -> Vec<&'static str> {
        self.shown.iter().map(|module| module.id()).collect()
    }

    /// The shown modules in order.
    pub fn shown(&self) -> &[Module] {
        &self.shown
    }

    pub fn contains(&self, module: Module) -> bool {
        self.shown.contains(&module)
    }

    /// The modules not shown, in reading order: the ones offered to add.
    pub fn available(&self) -> Vec<Module> {
        Module::ALL
            .into_iter()
            .filter(|module| !self.contains(*module))
            .collect()
    }

    /// The place of `module` in the order, from 0.
    pub fn position(&self, module: Module) -> Option<usize> {
        self.shown.iter().position(|shown| *shown == module)
    }

    fn fits(order: &[Module]) -> bool {
        pack(order).1 <= ROWS
    }

    /// Puts an order that does not fit the grid into reading order, which does.
    fn settle(&mut self) {
        if !Self::fits(&self.shown) {
            let shown = std::mem::take(&mut self.shown);
            self.shown = Module::ALL
                .into_iter()
                .filter(|module| shown.contains(module))
                .collect();
        }
    }

    /// Stop showing `module`. Returns false when it was not shown.
    pub fn remove(&mut self, module: Module) -> bool {
        let Some(index) = self.position(module) else {
            return false;
        };
        self.shown.remove(index);
        self.settle();
        true
    }

    /// Show `module` after the others, or at the last place of the order where it fits the grid. Returns false when
    /// it was shown already.
    pub fn add(&mut self, module: Module) -> bool {
        if self.contains(module) {
            return false;
        }
        let fitting = (0..=self.shown.len()).rev().find(|&index| {
            let mut order = self.shown.clone();
            order.insert(index, module);
            Self::fits(&order)
        });
        match fitting {
            Some(index) => self.shown.insert(index, module),
            None => {
                self.shown.push(module);
                self.settle();
            }
        }
        true
    }

    /// The order with `module` moved to `index` (clamped to the last place), when that fits the grid.
    fn moved(&self, module: Module, index: usize) -> Option<Vec<Module>> {
        let from = self.position(module)?;
        let mut order = self.shown.clone();
        order.remove(from);
        order.insert(index.min(order.len()), module);
        Self::fits(&order).then_some(order)
    }

    /// Move `module` to `index` of the order. Returns false, with nothing changed, when it is not shown, is there
    /// already, or the tiles would no longer fit the grid.
    pub fn move_to(&mut self, module: Module, index: usize) -> bool {
        match self.moved(module, index) {
            Some(order) if order != self.shown => {
                self.shown = order;
                true
            }
            _ => false,
        }
    }

    /// Move `module` one place later (`forward`) or earlier, passing over a place where the tiles would not fit the
    /// grid. Returns false when there is no such place.
    pub fn step(&mut self, module: Module, forward: bool) -> bool {
        let Some(from) = self.position(module) else {
            return false;
        };
        let targets: Vec<usize> = if forward {
            (from + 1..self.shown.len()).collect()
        } else {
            (0..from).rev().collect()
        };
        targets.into_iter().any(|index| self.move_to(module, index))
    }

    /// Move `module` to the place of the order that puts its tile nearest to `origin`, the corner of the tile as
    /// the pointer holds it, from the grid's corner; of two places that put it equally near, to the one fewer places
    /// away. Returns whether the order changed.
    pub fn move_near(&mut self, module: Module, origin: Point) -> bool {
        let Some(from) = self.position(module) else {
            return false;
        };
        let nearest = (0..self.shown.len())
            .filter_map(|index| {
                let order = self.moved(module, index)?;
                let (places, _) = pack(&order);
                let frame = places.iter().find(|(m, _)| *m == module)?.1;
                let distance = (frame.x() - origin.x).hypot(frame.y() - origin.y);
                Some((distance, index.abs_diff(from), index))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        nearest.is_some_and(|(_, _, index)| self.move_to(module, index))
    }

    /// Where each shown module's tile lies, from the grid's corner, in the order.
    pub fn places(&self) -> Vec<(Module, Rect)> {
        pack(&self.shown).0
    }

    /// Where `module`'s tile lies, from the grid's corner, when it is shown.
    pub fn frame(&self, module: Module) -> Option<Rect> {
        self.places()
            .into_iter()
            .find(|(placed, _)| *placed == module)
            .map(|(_, frame)| frame)
    }

    /// The rows the tiles take.
    pub fn rows(&self) -> usize {
        pack(&self.shown).1
    }

    /// The height of the tiles' rows with the gaps between them; 0 with no tile.
    pub fn height(&self) -> f64 {
        match self.rows() {
            0 => 0.0,
            rows => rows as f64 * ROW + (rows as f64 - 1.0) * GAP,
        }
    }
}
