//! The `xkb` module of the `xkbcommon` crate: contexts, keymaps and keyboard state.
//!
//! Signatures follow the C-binding crate (0.8). Where the C library reports failure with a sentinel the
//! sentinel is kept: `NoSymbol` for an unknown keysym name or a key without exactly one symbol, `*_INVALID` for an
//! unknown index, `None` for a keymap that does not compile, an empty slice for an unknown key.
//!
//! Deviations from the C-binding crate, all on paths the C crate leaves undefined or defective:
//! - the `*_is_active` methods return `false` for an unknown modifier, layout or LED; the C binding returns `true`
//!   for an unknown layout or LED because it tests the C result `-1` with `!= 0`;
//! - `Keymap::layout_get_name`, `mod_get_name` and `led_get_name` panic on an index out of range, where the C binding
//!   dereferences a null pointer;
//! - `Keymap::mods`, `layouts` and `leds` return `impl Iterator<Item = &str>`, not the named iterator types;
//! - rule names left empty always select the defaults, so an explicitly empty options string does not override
//!   `XKB_DEFAULT_OPTIONS`;
//! - there is no compose, X11 or raw-pointer API.
//!
//! The engine is libxkbcommon 1.7, but its keysym name tables are the ones of libxkbcommon 1.13: the vendored copy
//! in `third-party/xkbcommon-rs` is extended by `tools/cupertino_xkb_keysyms.py`, because the `xkeysym` crate that
//! the port generates them from stops at 1.7 and a keymap that names a newer keysym (`XF86AudioBassBoost`) would log a
//! warning and leave the key without symbols. Against libxkbcommon 1.13 there are two known differences:
//! `utf32_to_keysym` keeps the legacy keysym for about 60 symbol code points where 1.13 returns the Unicode form
//! (U+2022 gives `enfilledcircbullet`, not `U2022`), gives `NoSymbol` for the noncharacters (U+FDD0, U+FFFE) where
//! 1.13 gives the Unicode form, and gives the Unicode form for U+0000 where 1.13 gives `NoSymbol`; and
//! `keysym_get_name` picks the older name of aliased keysyms (`Mode_switch`, `U2039`).

pub mod ffi;
pub mod keysyms;

use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::io;
use std::os::fd::OwnedFd;
use std::sync::Arc;

use memmap2::MmapOptions;
use xkbcommon_rs as rs;

/// A number used to represent a physical key on a keyboard. With an evdev device it is the evdev code plus 8.
pub use xkeysym::KeyCode as Keycode;

/// A number used to represent the symbols generated from a key on a keyboard.
pub use xkeysym::Keysym;

/// Index of a keyboard layout (an XKB group), starting at 0.
pub type LayoutIndex = u32;
/// A mask of layout indices.
pub type LayoutMask = u32;
/// Index of a shift level of a key, starting at 0.
pub type LevelIndex = u32;
/// Index of a modifier, starting at 0.
pub type ModIndex = u32;
/// A mask of modifier indices.
pub type ModMask = u32;
/// Index of a keyboard LED.
pub type LedIndex = u32;
/// A mask of LED indices.
pub type LedMask = u32;

/// Returned for an unknown keycode.
pub const KEYCODE_INVALID: u32 = 0xffff_ffff;
/// Returned for an unknown layout.
pub const LAYOUT_INVALID: u32 = 0xffff_ffff;
/// Returned for an unknown level.
pub const LEVEL_INVALID: u32 = 0xffff_ffff;
/// Returned for an unknown modifier.
pub const MOD_INVALID: u32 = 0xffff_ffff;
/// Returned for an unknown LED.
pub const LED_INVALID: u32 = 0xffff_ffff;
/// The largest legal keycode.
pub const KEYCODE_MAX: u32 = 0xffff_fffe;

/// Flags of [`keysym_from_name`].
pub type KeysymFlags = u32;
/// No flags: the name is case-sensitive.
pub const KEYSYM_NO_FLAGS: u32 = 0;
/// Match the name case-insensitively.
pub const KEYSYM_CASE_INSENSITIVE: u32 = 1 << 0;

/// Flags of [`Context::new`].
pub type ContextFlags = u32;
/// No flags.
pub const CONTEXT_NO_FLAGS: u32 = 0;
/// Start with an empty include path.
pub const CONTEXT_NO_DEFAULT_INCLUDES: u32 = 1 << 0;
/// Do not read the default rule names from the environment.
pub const CONTEXT_NO_ENVIRONMENT_NAMES: u32 = 1 << 1;

/// Flags of the keymap constructors.
pub type KeymapCompileFlags = u32;
/// No flags.
pub const KEYMAP_COMPILE_NO_FLAGS: u32 = 0;

/// The format of a serialised keymap.
pub type KeymapFormat = u32;
/// The classic XKB text format.
pub const KEYMAP_FORMAT_TEXT_V1: u32 = 1;
/// The format the keymap was created from.
pub const KEYMAP_FORMAT_USE_ORIGINAL: u32 = 0xffff_ffff;

/// Direction of a key event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum KeyDirection {
    /// The key was released.
    Up,
    /// The key was pressed.
    Down,
}

/// A bit mask of state components, from the `STATE_*` constants.
pub type StateComponent = u32;
/// Depressed modifiers, i.e. a key is physically holding them.
pub const STATE_MODS_DEPRESSED: u32 = ffi::XKB_STATE_MODS_DEPRESSED;
/// Latched modifiers: cleared after the next non-modifier key press.
pub const STATE_MODS_LATCHED: u32 = ffi::XKB_STATE_MODS_LATCHED;
/// Locked modifiers: cleared when the key that locked them is pressed again.
pub const STATE_MODS_LOCKED: u32 = ffi::XKB_STATE_MODS_LOCKED;
/// Effective modifiers, derived from the other components. Use this unless the origin matters.
pub const STATE_MODS_EFFECTIVE: u32 = ffi::XKB_STATE_MODS_EFFECTIVE;
/// Depressed layout.
pub const STATE_LAYOUT_DEPRESSED: u32 = ffi::XKB_STATE_LAYOUT_DEPRESSED;
/// Latched layout.
pub const STATE_LAYOUT_LATCHED: u32 = ffi::XKB_STATE_LAYOUT_LATCHED;
/// Locked layout.
pub const STATE_LAYOUT_LOCKED: u32 = ffi::XKB_STATE_LAYOUT_LOCKED;
/// Effective layout, derived from the other components. Use this unless the origin matters.
pub const STATE_LAYOUT_EFFECTIVE: u32 = ffi::XKB_STATE_LAYOUT_EFFECTIVE;
/// LEDs, derived from the other components.
pub const STATE_LEDS: u32 = ffi::XKB_STATE_LEDS;

/// The real modifier named Shift.
pub const MOD_NAME_SHIFT: &str = "Shift";
/// The real modifier named Lock (Caps Lock).
pub const MOD_NAME_CAPS: &str = "Lock";
/// The real modifier named Control.
pub const MOD_NAME_CTRL: &str = "Control";
/// The real modifier named Mod1 (Alt).
pub const MOD_NAME_ALT: &str = "Mod1";
/// The real modifier named Mod2 (Num Lock).
pub const MOD_NAME_NUM: &str = "Mod2";
/// The real modifier named Mod3.
pub const MOD_NAME_MOD3: &str = "Mod3";
/// The real modifier named Mod4 (Super).
pub const MOD_NAME_LOGO: &str = "Mod4";
/// The virtual modifier named LevelThree (AltGr).
pub const MOD_NAME_ISO_LEVEL3_SHIFT: &str = "Mod5";
/// The Caps Lock LED.
pub const LED_NAME_CAPS: &str = "Caps Lock";
/// The Num Lock LED.
pub const LED_NAME_NUM: &str = "Num Lock";
/// The Scroll Lock LED.
pub const LED_NAME_SCROLL: &str = "Scroll Lock";

/// The upper bound of LED indices in a keymap.
const MAX_LEDS: u32 = 32;

/// Widen an index: every supported target has `usize` of at least 32 bits.
fn widen(index: u32) -> usize {
    usize::try_from(index).expect("usize holds a u32")
}

/// Narrow an index of the engine, which is bounded by 32 modifiers, 4 layouts and 32 LEDs.
fn narrow(index: usize) -> u32 {
    u32::try_from(index).expect("keymap indices are small")
}

/// The name of a keysym: its symbolic name, `U+xxxx` form or hexadecimal value. Empty for a value that is no keysym.
pub fn keysym_get_name(keysym: Keysym) -> String {
    rs::keysym::keysym_get_name(&keysym).unwrap_or_default()
}

/// The keysym with the given name, [`Keysym::new`]`(0)` (`NoSymbol`) when there is none or the flags are unknown.
pub fn keysym_from_name(name: &str, flags: KeysymFlags) -> Keysym {
    u8::try_from(flags)
        .ok()
        .and_then(|flags| rs::keysym::keysym_from_name(name, flags))
        .unwrap_or(Keysym::new(0))
}

/// The UTF-32 code point of a keysym, 0 when it has none.
pub fn keysym_to_utf32(keysym: Keysym) -> u32 {
    keysym.key_char().map_or(0, u32::from)
}

/// The UTF-8 text of a keysym, empty when it has none.
pub fn keysym_to_utf8(keysym: Keysym) -> String {
    keysym
        .key_char()
        .filter(|c| *c != '\0')
        .map(String::from)
        .unwrap_or_default()
}

/// The keysym of a Unicode code point, `NoSymbol` for a value that is no Unicode scalar value.
pub fn utf32_to_keysym(ucs: u32) -> Keysym {
    char::from_u32(ucs).map_or(Keysym::new(0), Keysym::from_char)
}

/// Owner of the include paths and the name table keymaps are compiled in.
#[derive(Clone)]
pub struct Context {
    engine: rs::Context,
}

impl Context {
    /// A context with the default include paths (`/etc/xkb`, `/usr/share/X11/xkb`) unless `flags` say otherwise.
    pub fn new(flags: ContextFlags) -> Context {
        let engine = rs::Context::new(flags).expect("creating a context cannot fail");
        Context { engine }
    }

    /// Identity of the context, for `Debug` output only.
    pub fn get_raw_ptr(&self) -> *const Context {
        self
    }
}

/// The compiled tables of one keymap plus the symbol lists of every level, so symbol queries can hand out slices.
struct KeymapData {
    engine: rs::Keymap,
    /// Per keycode, the symbol list of each level of each layout that has symbols.
    level_syms: BTreeMap<u32, Vec<Box<[Keysym]>>>,
}

/// A compiled keymap: key to keysym tables for every layout and level, modifiers, layouts and LEDs. Cheap to clone.
#[derive(Clone)]
pub struct Keymap {
    data: Arc<KeymapData>,
}

impl Keymap {
    /// Wraps a compiled keymap, storing the symbol list of every level of every key. A level the engine cannot
    /// return is an error: [`Keymap::level_syms`] relies on every list the engine computes being stored.
    fn from_engine(engine: rs::Keymap) -> Result<Keymap, String> {
        let mut level_syms = BTreeMap::new();
        for &code in engine.iter_keycodes() {
            let layouts = engine
                .num_layouts_for_key(code)
                .ok_or_else(|| format!("keycode {code} is listed but has no key"))?;
            let mut lists = Vec::new();
            for layout in 0..layouts {
                for level in 0..engine.num_levels_for_key(code, layout) {
                    let syms =
                        engine
                            .key_get_syms_by_level(code, layout, level)
                            .map_err(|err| {
                                format!("keycode {code}, layout {layout}, level {level}: {err}")
                            })?;
                    if !syms.is_empty() {
                        lists.push(syms.into_boxed_slice());
                    }
                }
            }
            level_syms.insert(code, lists);
        }
        Ok(Keymap {
            data: Arc::new(KeymapData { engine, level_syms }),
        })
    }

    /// Hand out the stored slice equal to `syms`, which the engine computed from one level of `code`.
    fn level_syms(&self, code: u32, syms: &[Keysym]) -> &[Keysym] {
        if syms.is_empty() {
            return &[];
        }
        self.data
            .level_syms
            .get(&code)
            .and_then(|lists| lists.iter().find(|list| list[..] == *syms))
            .expect("the engine returns the symbols of a level of the key")
    }

    /// Compile a keymap from rules, model, layout, variant and options (`""` and `None` select the defaults),
    /// `None` if it does not compile. The reason is printed to standard error.
    pub fn new_from_names<S: Borrow<str> + ?Sized>(
        context: &Context,
        rules: &S,
        model: &S,
        layout: &S,
        variant: &S,
        options: Option<String>,
        flags: KeymapCompileFlags,
    ) -> Option<Keymap> {
        let names = rs::xkb_keymap::RuleNames::new(
            rules.borrow(),
            model.borrow(),
            layout.borrow(),
            variant.borrow(),
            options.as_deref().unwrap_or(""),
        );
        Self::report(rs::Keymap::new_from_names(
            context.engine.clone(),
            Some(names),
            flags,
        ))
    }

    /// Parse a keymap in text format, `None` if it does not compile. The reason is printed to standard error.
    pub fn new_from_string(
        context: &Context,
        string: String,
        format: KeymapFormat,
        flags: KeymapCompileFlags,
    ) -> Option<Keymap> {
        Self::report(rs::Keymap::new_from_string(
            context.engine.clone(),
            &string,
            format,
            flags,
        ))
    }

    /// Parse the keymap in the first `size - 1` bytes of the file `fd` (`size` counts the terminating NUL of the
    /// Wayland keymap file), mapped copy-on-write. `Ok(None)` if the text does not compile.
    ///
    /// # Safety
    /// No other process may truncate the file while it is mapped. A `size` larger than the file is an error.
    pub unsafe fn new_from_fd(
        context: &Context,
        fd: OwnedFd,
        size: usize,
        format: KeymapFormat,
        flags: KeymapCompileFlags,
    ) -> io::Result<Option<Keymap>> {
        if size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the keymap size is 0, which leaves no room for its terminating NUL",
            ));
        }
        let file = std::fs::File::from(fd);
        if size as u64 > file.metadata()?.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the keymap size is larger than its file",
            ));
        }
        let map = MmapOptions::new().len(size).map_copy_read_only(&file)?;
        let text = &map[..size - 1];
        let Ok(string) = std::str::from_utf8(text) else {
            eprintln!("xkbcommon: the keymap is not valid UTF-8");
            return Ok(None);
        };
        Ok(Self::new_from_string(
            context,
            string.to_owned(),
            format,
            flags,
        ))
    }

    fn report(result: Result<rs::Keymap, rs::error::keymap::KeymapCompileError>) -> Option<Keymap> {
        match result {
            Ok(engine) => match Self::from_engine(engine) {
                Ok(keymap) => Some(keymap),
                Err(err) => {
                    eprintln!("xkbcommon: cannot read the compiled keymap: {err}");
                    None
                }
            },
            Err(err) => {
                eprintln!("xkbcommon: cannot compile the keymap: {err}");
                None
            }
        }
    }

    /// The keymap in text format, which `new_from_string` parses back into the same keymap.
    pub fn get_as_string(&self, format: KeymapFormat) -> String {
        self.data
            .engine
            .get_as_string(format)
            .expect("a compiled keymap serialises")
    }

    /// The smallest keycode of the keymap.
    pub fn min_keycode(&self) -> Keycode {
        Keycode::new(self.data.engine.min_keycode().raw())
    }

    /// The largest keycode of the keymap.
    pub fn max_keycode(&self) -> Keycode {
        Keycode::new(self.data.engine.max_keycode().raw())
    }

    /// Call `closure` for every key of the keymap, in keycode order.
    pub fn key_for_each<F>(&self, mut closure: F)
    where
        F: FnMut(&Keymap, Keycode),
    {
        for &code in self.data.engine.iter_keycodes() {
            closure(self, Keycode::new(code));
        }
    }

    /// The names of the modifiers, in index order.
    pub fn mods(&self) -> impl Iterator<Item = &str> {
        (0..self.num_mods()).map(|idx| self.mod_get_name(idx))
    }

    /// The number of modifiers.
    pub fn num_mods(&self) -> ModIndex {
        narrow(self.data.engine.num_mods())
    }

    /// The name of a modifier; panics if `idx` is not below `num_mods`.
    pub fn mod_get_name(&self, idx: ModIndex) -> &str {
        self.data
            .engine
            .mod_get_name(widen(idx))
            .expect("modifier index in range")
    }

    /// The index of the modifier with this name, [`MOD_INVALID`] if there is none.
    pub fn mod_get_index<S: Borrow<str> + ?Sized>(&self, name: &S) -> ModIndex {
        self.data
            .engine
            .mod_get_index(name.borrow())
            .map_or(MOD_INVALID, narrow)
    }

    /// The names of the layouts, in index order. A layout without a name is skipped.
    pub fn layouts(&self) -> impl Iterator<Item = &str> {
        (0..self.num_layouts()).filter_map(|idx| self.data.engine.layout_get_name(widen(idx)))
    }

    /// The number of layouts.
    pub fn num_layouts(&self) -> LayoutIndex {
        narrow(self.data.engine.num_layouts())
    }

    /// The name of a layout; panics if `idx` is not below `num_layouts` or the layout has no name.
    pub fn layout_get_name(&self, idx: LayoutIndex) -> &str {
        self.data
            .engine
            .layout_get_name(widen(idx))
            .expect("layout index in range with a name")
    }

    /// The index of the layout with this name, [`LAYOUT_INVALID`] if there is none.
    pub fn layout_get_index<S: Borrow<str> + ?Sized>(&self, name: &S) -> LayoutIndex {
        self.data
            .engine
            .layout_get_index(name.borrow())
            .map_or(LAYOUT_INVALID, narrow)
    }

    /// The names of the LEDs, in index order. Unused indices are skipped.
    pub fn leds(&self) -> impl Iterator<Item = &str> {
        (0..MAX_LEDS).filter_map(|idx| self.data.engine.led_get_name(widen(idx)))
    }

    /// The name of a LED; panics if the index has no LED.
    pub fn led_get_name(&self, idx: LedIndex) -> &str {
        self.data
            .engine
            .led_get_name(widen(idx))
            .expect("LED index in use")
    }

    /// The index of the LED with this name, [`LED_INVALID`] if there is none.
    pub fn led_get_index<S: Borrow<str> + ?Sized>(&self, name: &S) -> LedIndex {
        self.data
            .engine
            .led_get_index(name.borrow())
            .map_or(LED_INVALID, narrow)
    }

    /// The name of a key such as `AE01`, `None` for a keycode without a key.
    pub fn key_get_name(&self, key: Keycode) -> Option<&str> {
        self.data.engine.key_get_name(key.raw())
    }

    /// The number of layouts that define the key, 0 for a keycode without a key.
    pub fn num_layouts_for_key(&self, key: Keycode) -> LayoutIndex {
        self.data
            .engine
            .num_layouts_for_key(key.raw())
            .map_or(0, narrow)
    }

    /// The number of levels of the key in a layout (wrapped into range like the key does), 0 for no key.
    pub fn num_levels_for_key(&self, key: Keycode, layout: LayoutIndex) -> LevelIndex {
        narrow(
            self.data
                .engine
                .num_levels_for_key(key.raw(), widen(layout)),
        )
    }

    /// The keysyms of the key at an explicit layout and level, empty for no key or an unknown level.
    pub fn key_get_syms_by_level(
        &self,
        key: Keycode,
        layout: LayoutIndex,
        level: LevelIndex,
    ) -> &[Keysym] {
        match self
            .data
            .engine
            .key_get_syms_by_level(key.raw(), widen(layout), widen(level))
        {
            Ok(syms) => self.level_syms(key.raw(), &syms),
            Err(_) => &[],
        }
    }

    /// Whether holding the key repeats it.
    pub fn key_repeats(&self, key: Keycode) -> bool {
        self.data.engine.key_repeats(key.raw())
    }

    /// Identity of the keymap, for `Debug` output only.
    pub fn get_raw_ptr(&self) -> *const Keymap {
        Arc::as_ptr(&self.data).cast()
    }
}

/// The state of a keyboard: which modifiers and layout are active, and the keysyms a key gives in that state.
pub struct State {
    keymap: Keymap,
    engine: rs::State,
}

impl State {
    /// A state with nothing pressed or locked.
    pub fn new(keymap: &Keymap) -> State {
        State {
            keymap: keymap.clone(),
            engine: rs::State::new(keymap.data.engine.clone()),
        }
    }

    /// The keymap the state was created with.
    pub fn get_keymap(&self) -> Keymap {
        self.keymap.clone()
    }

    /// Apply a key press or release; returns the mask of state components that changed, 0 if none.
    /// For compositors, which see the key events. Clients use [`State::update_mask`].
    pub fn update_key(&mut self, key: Keycode, direction: KeyDirection) -> StateComponent {
        let direction = match direction {
            KeyDirection::Up => rs::xkb_state::KeyDirection::Up,
            KeyDirection::Down => rs::xkb_state::KeyDirection::Down,
        };
        u32::from(self.engine.update_key(key.raw(), direction).bits())
    }

    /// Set the modifier and layout components from the values of a `wl_keyboard.modifiers` event; returns the mask of
    /// state components that changed, 0 if none.
    pub fn update_mask(
        &mut self,
        depressed_mods: ModMask,
        latched_mods: ModMask,
        locked_mods: ModMask,
        depressed_layout: LayoutIndex,
        latched_layout: LayoutIndex,
        locked_layout: LayoutIndex,
    ) -> StateComponent {
        let changed = self.engine.update_mask(
            depressed_mods,
            latched_mods,
            locked_mods,
            widen(depressed_layout),
            widen(latched_layout),
            widen(locked_layout),
        );
        u32::from(changed.bits())
    }

    /// The keysyms of the key in the current state, without the capitalisation or control transformations.
    pub fn key_get_syms(&self, key: Keycode) -> &[Keysym] {
        self.keymap
            .level_syms(key.raw(), &self.engine.key_get_syms(key.raw()))
    }

    /// The single keysym of the key in the current state with capitalisation applied; `NoSymbol` if the key has
    /// none or more than one.
    pub fn key_get_one_sym(&self, key: Keycode) -> Keysym {
        self.engine
            .key_get_one_sym(key.raw())
            .unwrap_or(Keysym::new(0))
    }

    /// The text of the key in the current state with capitalisation and control transformations applied; empty for
    /// keys without text.
    pub fn key_get_utf8(&self, key: Keycode) -> String {
        self.engine
            .key_get_utf8(key.raw())
            .map(|bytes| String::from_utf8(bytes).expect("the engine produces UTF-8"))
            .unwrap_or_default()
    }

    /// The UTF-32 code point of the key in the current state, 0 for keys without text.
    pub fn key_get_utf32(&self, key: Keycode) -> u32 {
        self.engine.key_get_utf32(key.raw()).unwrap_or(0)
    }

    /// The layout the key uses in the current state, [`LAYOUT_INVALID`] for a keycode without a key.
    pub fn key_get_layout(&self, key: Keycode) -> LayoutIndex {
        self.engine
            .key_get_layout(key.raw())
            .map_or(LAYOUT_INVALID, narrow)
    }

    /// The modifier mask of the given components (`STATE_MODS_*`), for a `wl_keyboard.modifiers` event.
    pub fn serialize_mods(&self, components: StateComponent) -> ModMask {
        self.engine.serialize_mods(component_flags(components))
    }

    /// The layout index of the given component (`STATE_LAYOUT_*`), for a `wl_keyboard.modifiers` event.
    pub fn serialize_layout(&self, components: StateComponent) -> LayoutIndex {
        narrow(self.engine.serialize_layout(component_flags(components)))
    }

    /// Whether the named modifier is active in the given components; `false` for an unknown name.
    pub fn mod_name_is_active<S: Borrow<str> + ?Sized>(
        &self,
        name: &S,
        components: StateComponent,
    ) -> bool {
        matches!(
            self.engine
                .mod_name_is_active(name.borrow(), component_flags(components)),
            Ok(true)
        )
    }

    /// Whether the modifier with this index is active in the given components; `false` for an unknown index.
    pub fn mod_index_is_active(&self, idx: ModIndex, components: StateComponent) -> bool {
        matches!(
            self.engine
                .mod_index_is_active(widen(idx), component_flags(components)),
            Ok(true)
        )
    }

    /// Whether the layout with this index is active in the given components; `false` for an unknown index.
    pub fn layout_index_is_active(&self, idx: LayoutIndex, components: StateComponent) -> bool {
        matches!(
            self.engine
                .layout_index_is_active(widen(idx), component_flags(components)),
            Ok(true)
        )
    }

    /// Whether the named LED is lit; `false` for an unknown name.
    pub fn led_name_is_active<S: Borrow<str> + ?Sized>(&self, name: &S) -> bool {
        matches!(self.engine.led_name_is_active(name.borrow()), Ok(true))
    }

    /// Whether the LED with this index is lit; `false` for an unknown index.
    pub fn led_index_is_active(&self, idx: LedIndex) -> bool {
        matches!(self.engine.led_index_is_active(widen(idx)), Ok(true))
    }

    /// Identity of the state, for `Debug` output only.
    pub fn get_raw_ptr(&self) -> *const State {
        self
    }
}

/// The engine's flag set for a mask of `STATE_*` bits; unknown bits are dropped like the C library does.
fn component_flags(components: StateComponent) -> rs::xkb_state::StateComponent {
    rs::xkb_state::StateComponent::from_bits_truncate(
        u16::try_from(components & 0xffff).expect("masked to 16 bits"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_size_keymap_file_is_an_error() {
        let fd = OwnedFd::from(std::fs::File::open("/dev/null").unwrap());
        let context = Context::new(CONTEXT_NO_FLAGS);
        // SAFETY: nothing is mapped for a zero size.
        let result = unsafe {
            Keymap::new_from_fd(
                &context,
                fd,
                0,
                KEYMAP_FORMAT_TEXT_V1,
                KEYMAP_COMPILE_NO_FLAGS,
            )
        };
        assert_eq!(
            result.err().map(|err| err.kind()),
            Some(io::ErrorKind::InvalidData)
        );
    }

    #[test]
    fn a_keymap_size_beyond_the_file_is_an_error() {
        let file = std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
        let size = usize::try_from(file.metadata().unwrap().len()).unwrap() + 1;
        let context = Context::new(CONTEXT_NO_FLAGS);
        // SAFETY: the size is rejected before anything is mapped.
        let result = unsafe {
            Keymap::new_from_fd(
                &context,
                OwnedFd::from(file),
                size,
                KEYMAP_FORMAT_TEXT_V1,
                KEYMAP_COMPILE_NO_FLAGS,
            )
        };
        assert_eq!(
            result.err().map(|err| err.kind()),
            Some(io::ErrorKind::InvalidData)
        );
    }
}
