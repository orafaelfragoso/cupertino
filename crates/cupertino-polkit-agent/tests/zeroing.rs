//! What is typed into the prompt leaves no copy behind in the memory the agent frees, with the system allocator as it is
//! (the binary's allocator zeroes every freed block on top of this): the field, the answer and the line written to
//! polkit's helper are all zeroed before they are freed, and the field never grows into a new block. The helper is the
//! fixture of `tests/fixtures`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use cupertino_polkit_agent::agent::{Request, User};
use cupertino_polkit_agent::helper::{Helper, Line, Route};
use cupertino_polkit_agent::model::{Agent, Effect, Key};

/// What is typed: long enough that a field growing as it is typed would leave several blocks behind, and made of no
/// word or path, so no piece of it is found anywhere else in the process. Only this test writes it.
const MARKER: &str = "Qx7#vT2!mZ9@kL4$wR8%nB3^jH6&dF1*pY5~cG0+sN";
/// The length of the piece of [`MARKER`] the scan looks for: a block that held a part of what was typed is caught too.
const PIECE: usize = 8;

/// Blocks freed with a piece of the marker still in them.
static LEFT_BEHIND: AtomicUsize = AtomicUsize::new(0);

/// The system allocator, counting the freed blocks that hold a piece of [`MARKER`].
struct Watch;

// Safety: forwards to `System`; the scan only reads the block being freed.
unsafe impl GlobalAlloc for Watch {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let block = std::slice::from_raw_parts(ptr, layout.size());
        if MARKER
            .as_bytes()
            .windows(PIECE)
            .any(|piece| block.windows(PIECE).any(|window| window == piece))
        {
            LEFT_BEHIND.fetch_add(1, Ordering::SeqCst);
        }
        System.dealloc(ptr, layout);
    }
}

#[global_allocator]
static ALLOCATOR: Watch = Watch;

#[test]
fn nothing_typed_is_freed_with_the_text_still_in_it() {
    let me = User(1000);
    let mut agent = Agent::new(me);
    agent.begin(Request {
        cookie: "cookie".into(),
        action: "org.example.mount".into(),
        message: "Authentication is required".into(),
        details: Vec::new(),
        users: vec![me],
    });
    let route = Route::Binary(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/polkit-agent-helper-1"
        )
        .into(),
    );
    let mut helper = Helper::start(&[route], "someone", "cookie").expect("the fixture helper");
    let prompt = loop {
        match helper.read_line().expect("a line") {
            Line::Info(_) => continue,
            line => break line,
        }
    };
    agent.helper(prompt);
    for c in MARKER.chars() {
        assert!(agent.key(Key::Char(c)).is_empty());
    }
    let effects = agent.key(Key::Return);
    let [Effect::Answer(answer)] = effects.as_slice() else {
        panic!("one answer, got {effects:?}");
    };
    assert_eq!(answer.as_slice(), MARKER.as_bytes());
    helper.answer(answer).expect("answered");
    assert_eq!(helper.read_line().expect("a line"), Line::Failure);
    drop(effects);
    drop(helper);
    drop(agent);
    assert_eq!(LEFT_BEHIND.load(Ordering::SeqCst), 0);
}
