//! The portal's consent questions as the prompts model them: the order they are shown in, the arming delay before an
//! answer that grants, the answers each question gives its caller, the portal's Close, and a session that goes away.
//! Also how the alerts word what they ask.

use std::time::{Duration, Instant};

use cupertino_prompts::consent::model::{Answered, Consents};
use cupertino_prompts::consent::{
    App, Arming, Choice, Question, Row, ARMING, SCREENSHOT_CANCELLED, SCREENSHOT_GRANTED,
    SHORTCUTS_ALLOWED, SHORTCUTS_DENIED, SHORTCUTS_DISMISSED,
};
use cupertino_prompts::view::consent::{row_text, screenshot_prompt, shortcuts_prompt};

fn app(name: &str) -> App {
    App {
        id: format!("org.example.{name}"),
        name: Some(name.to_owned()),
        icon: None,
    }
}

fn screenshot(name: &str) -> Question {
    Question::Screenshot { app: app(name) }
}

fn shortcuts(name: &str) -> Question {
    Question::Shortcuts {
        app: app(name),
        rows: vec![Row {
            description: "Push to talk".to_owned(),
            keys: "\u{2303}\u{2325}T".to_owned(),
            note: String::new(),
        }],
        allowed: 0,
    }
}

fn answered(call: u64, response: u32) -> Vec<Answered> {
    vec![Answered { call, response }]
}

/// The question shown, armed as the program arms it: presented at `at`, its delay heard to pass.
fn arm_shown(consents: &mut Consents, at: Instant) {
    let call = consents.shown(at).expect("a question is shown").call;
    let wait = consents.presented(call, at).expect("its arming starts");
    assert_eq!(wait, ARMING, "the whole delay is left when it starts");
    consents.arm(call);
}

#[test]
fn questions_are_shown_one_at_a_time_in_the_order_they_came() {
    let now = Instant::now();
    let mut consents = Consents::new();
    assert!(consents.shown(now).is_none());
    assert!(consents
        .ask(1, "/request/a".to_owned(), screenshot("Notes"))
        .is_empty());
    assert!(consents
        .ask(2, "/request/b".to_owned(), shortcuts("Talk"))
        .is_empty());
    assert_eq!(consents.len(), 2);
    let shown = consents.shown(now).unwrap();
    assert_eq!((shown.call, shown.handle), (1, "/request/a"));
    assert_eq!(shown.question, &screenshot("Notes"));
    assert!(!shown.armed, "not armed before it was presented");

    assert_eq!(
        consents.answer(Choice::Deny, now),
        answered(1, SCREENSHOT_CANCELLED)
    );
    assert_eq!(consents.shown(now).unwrap().call, 2, "the next one follows");
    assert_eq!(
        consents.answer(Choice::Deny, now),
        answered(2, SHORTCUTS_DENIED)
    );
    assert!(consents.is_empty());
    assert!(
        consents.answer(Choice::Deny, now).is_empty(),
        "nothing is left to answer"
    );
}

#[test]
fn an_answer_that_grants_waits_for_the_arming_delay() {
    let opened = Instant::now();
    let mut consents = Consents::new();
    consents.ask(1, "/request/a".to_owned(), screenshot("Notes"));
    assert!(
        consents
            .answer(Choice::Allow, opened + ARMING * 2)
            .is_empty(),
        "an alert never presented grants nothing"
    );
    let call = consents.shown(opened).unwrap().call;
    let wait = consents.presented(call, opened).unwrap();
    assert_eq!(wait, ARMING);
    assert_eq!(
        consents.presented(call, opened + ARMING),
        None,
        "the delay starts with the first presented frame only"
    );
    assert!(
        consents
            .answer(Choice::Allow, opened + ARMING * 2)
            .is_empty(),
        "nothing grants before the program heard the delay pass"
    );
    consents.arm(call);
    assert!(
        consents
            .answer(Choice::Allow, opened + ARMING / 2)
            .is_empty(),
        "nor before the delay passed by the clock"
    );
    assert!(!consents.shown(opened + ARMING / 2).unwrap().armed);
    assert!(consents.shown(opened + ARMING).unwrap().armed);
    assert_eq!(
        consents.answer(Choice::Allow, opened + ARMING),
        answered(1, SCREENSHOT_GRANTED)
    );
}

#[test]
fn denying_and_dismissing_are_taken_at_once() {
    let now = Instant::now();
    let mut consents = Consents::new();
    consents.ask(1, "/a".to_owned(), screenshot("Notes"));
    assert_eq!(
        consents.answer(Choice::Dismiss, now),
        answered(1, SCREENSHOT_CANCELLED),
        "Escape cancels a screenshot"
    );
    consents.ask(2, "/b".to_owned(), shortcuts("Talk"));
    assert_eq!(
        consents.answer(Choice::Dismiss, now),
        answered(2, SHORTCUTS_DISMISSED),
        "Escape dismisses shortcuts, which the portal does not remember"
    );
    consents.ask(3, "/c".to_owned(), shortcuts("Talk"));
    assert_eq!(
        consents.answer(Choice::Deny, now),
        answered(3, SHORTCUTS_DENIED)
    );
    consents.ask(4, "/d".to_owned(), shortcuts("Talk"));
    arm_shown(&mut consents, now);
    assert_eq!(
        consents.answer(Choice::Allow, now + ARMING),
        answered(4, SHORTCUTS_ALLOWED)
    );
}

#[test]
fn the_responses_follow_the_kind_of_question() {
    assert_eq!(screenshot("A").response(Choice::Allow), SCREENSHOT_GRANTED);
    assert_eq!(screenshot("A").response(Choice::Deny), SCREENSHOT_CANCELLED);
    assert_eq!(
        screenshot("A").response(Choice::Dismiss),
        SCREENSHOT_CANCELLED
    );
    assert_eq!(shortcuts("A").response(Choice::Allow), SHORTCUTS_ALLOWED);
    assert_eq!(shortcuts("A").response(Choice::Deny), SHORTCUTS_DENIED);
    assert_eq!(
        shortcuts("A").response(Choice::Dismiss),
        SHORTCUTS_DISMISSED
    );
}

#[test]
fn the_portals_close_takes_a_question_down_shown_or_waiting() {
    let now = Instant::now();
    let mut consents = Consents::new();
    consents.ask(1, "/a".to_owned(), screenshot("Notes"));
    consents.ask(2, "/b".to_owned(), shortcuts("Talk"));
    consents.ask(3, "/c".to_owned(), screenshot("Paint"));
    assert_eq!(
        consents.close("/b"),
        answered(2, SHORTCUTS_DISMISSED),
        "a waiting one"
    );
    assert_eq!(consents.shown(now).unwrap().call, 1, "the one shown stays");
    assert_eq!(
        consents.close("/a"),
        answered(1, SCREENSHOT_CANCELLED),
        "the one shown"
    );
    assert_eq!(consents.shown(now).unwrap().call, 3);
    assert!(consents.close("/nothing").is_empty());
    assert_eq!(consents.len(), 1);
}

#[test]
fn a_session_that_goes_away_dismisses_every_question_and_shows_none() {
    let now = Instant::now();
    let mut consents = Consents::new();
    consents.ask(1, "/a".to_owned(), screenshot("Notes"));
    consents.ask(2, "/b".to_owned(), shortcuts("Talk"));
    arm_shown(&mut consents, now);
    let mut gone = consents.set_away(true);
    gone.sort_by_key(|a| a.call);
    assert_eq!(
        gone,
        vec![
            Answered {
                call: 1,
                response: SCREENSHOT_CANCELLED
            },
            Answered {
                call: 2,
                response: SHORTCUTS_DISMISSED
            },
        ],
        "nothing is granted and nothing remembered"
    );
    assert!(consents.shown(now).is_none());
    assert_eq!(
        consents.ask(3, "/c".to_owned(), screenshot("Notes")),
        answered(3, SCREENSHOT_CANCELLED),
        "a question asked while away is answered at once"
    );
    assert!(consents.answer(Choice::Allow, now + ARMING).is_empty());
    assert!(consents.set_away(false).is_empty());
    assert!(consents.shown(now).is_none(), "nothing comes back");
    consents.ask(4, "/d".to_owned(), screenshot("Notes"));
    assert_eq!(
        consents.shown(now).unwrap().call,
        4,
        "a new question shows again"
    );
}

#[test]
fn the_arming_delay_runs_from_the_first_frame() {
    let shown = Instant::now();
    let arming = Arming::new(shown);
    assert!(!arming.armed(shown));
    assert!(!arming.armed(shown + ARMING - Duration::from_millis(1)));
    assert!(arming.armed(shown + ARMING));
    assert_eq!(arming.remaining(shown + ARMING / 5), ARMING * 4 / 5);
    assert_eq!(arming.remaining(shown + ARMING * 2), Duration::ZERO);
}

#[test]
fn the_alerts_word_their_questions_as_the_portal_did() {
    assert_eq!(
        screenshot_prompt(Some("Notes")),
        "\u{201c}Notes\u{201d} wants to take a screenshot of your screen."
    );
    assert_eq!(
        screenshot_prompt(None),
        "An application wants to take a screenshot of your screen."
    );
    assert_eq!(
        shortcuts_prompt(Some("Talk"), 0),
        "\u{201c}Talk\u{201d} wants to use these keyboard shortcuts, also while you work in other apps."
    );
    assert_eq!(
        shortcuts_prompt(None, 2),
        "An application wants to use more keyboard shortcuts, also while you work in other apps."
    );
    let row = Row {
        description: "Raise a hand".to_owned(),
        keys: "H".to_owned(),
        note: "not available: it needs \u{2303}, \u{2325} or \u{2318}".to_owned(),
    };
    assert_eq!(
        row_text(&row),
        "H  Raise a hand (not available: it needs \u{2303}, \u{2325} or \u{2318})"
    );
    let available = Row {
        note: String::new(),
        ..row
    };
    assert_eq!(row_text(&available), "H  Raise a hand");
}
