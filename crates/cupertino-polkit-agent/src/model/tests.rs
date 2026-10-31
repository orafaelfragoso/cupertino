use super::*;

const ME: User = User(1000);
const ROOT: User = User(0);

fn request(cookie: &str, users: &[User]) -> Request {
    Request {
        cookie: cookie.into(),
        action: "org.example.mount".into(),
        message: "Authentication is required to mount the disk".into(),
        details: Vec::new(),
        users: users.to_vec(),
    }
}

fn start(user: User, cookie: &str) -> Effect {
    Effect::StartHelper {
        user,
        cookie: cookie.into(),
    }
}

fn finish(cookie: &str, outcome: Outcome) -> Effect {
    Effect::Finish {
        cookie: cookie.into(),
        outcome,
    }
}

fn asked(agent: &mut Agent) {
    assert!(agent
        .helper(Line::Prompt {
            text: "Password: ".into(),
            echo: false
        })
        .is_empty());
}

fn typed(agent: &mut Agent, text: &str) -> Vec<Effect> {
    for c in text.chars() {
        assert!(agent.key(Key::Char(c)).is_empty());
    }
    agent.key(Key::Return)
}

#[test]
fn the_session_user_is_offered_first_and_a_right_answer_finishes() {
    let mut agent = Agent::new(ME);
    assert_eq!(agent.begin(request("a", &[ROOT, ME])), [start(ME, "a")]);
    let prompt = agent.prompt().unwrap();
    assert_eq!((prompt.user, prompt.phase), (ME, Phase::Waiting));
    assert!(agent.key(Key::Char('x')).is_empty());
    assert_eq!(
        agent.prompt().unwrap().field_len(),
        0,
        "no typing before the helper asks"
    );

    asked(&mut agent);
    assert!(agent.key(Key::Return).is_empty(), "nothing to answer yet");
    assert!(!agent.prompt().unwrap().can_answer());
    assert_eq!(
        typed(&mut agent, "hunter2"),
        [Effect::Answer(Zeroizing::new(b"hunter2".to_vec()))]
    );
    let prompt = agent.prompt().unwrap();
    assert_eq!(
        (prompt.phase, prompt.field_len()),
        (Phase::Checking, 7),
        "the dots stay while checking"
    );
    assert!(!prompt.can_answer());
    assert_eq!(agent.helper(Line::Success), [finish("a", Outcome::Done)]);
    assert!(agent.prompt().is_none());
}

#[test]
fn a_secret_field_is_never_shown_and_a_clear_one_is() {
    let mut agent = Agent::new(ME);
    agent.begin(request("a", &[ME]));
    asked(&mut agent);
    agent.key(Key::Char('s'));
    assert_eq!(agent.prompt().unwrap().shown_field(), None);
    agent.helper(Line::Prompt {
        text: "Login:".into(),
        echo: true,
    });
    agent.key(Key::Char('a'));
    assert_eq!(agent.prompt().unwrap().shown_field(), Some("a"));
}

#[test]
fn a_failure_tries_again_and_the_third_tells_polkitd_it_is_over() {
    let mut agent = Agent::new(ME);
    agent.begin(request("a", &[ME]));
    for attempt in 1..ATTEMPTS {
        asked(&mut agent);
        typed(&mut agent, "wrong");
        assert_eq!(
            agent.helper(Line::Failure),
            [start(ME, "a")],
            "attempt {attempt}"
        );
        let prompt = agent.prompt().unwrap();
        assert_eq!(
            (prompt.phase, prompt.error.as_deref()),
            (Phase::Waiting, Some(TRY_AGAIN))
        );
    }
    asked(&mut agent);
    agent.key(Key::Char('w'));
    assert_eq!(
        agent.prompt().unwrap().error,
        None,
        "typing clears the error line"
    );
    typed(&mut agent, "rong");
    assert_eq!(agent.helper(Line::Failure), [finish("a", Outcome::Done)]);
}

#[test]
fn a_lost_helper_counts_as_a_failed_attempt() {
    let mut agent = Agent::new(ME);
    agent.begin(request("a", &[ME]));
    assert_eq!(agent.helper_lost(), [start(ME, "a")]);
    agent.helper_lost();
    assert_eq!(agent.helper_lost(), [finish("a", Outcome::Done)]);
}

#[test]
fn pam_messages_are_shown_safe_and_an_answer_never_holds_a_control() {
    let mut agent = Agent::new(ME);
    agent.begin(request("a", &[ME]));
    agent.helper(Line::Info("Touch\nthe \u{202E}key".into()));
    agent.helper(Line::Error("Expired".into()));
    asked(&mut agent);
    let prompt = agent.prompt().unwrap();
    assert_eq!(
        (prompt.info.as_deref(), prompt.error.as_deref()),
        (Some("Touch the key"), Some("Expired"))
    );
    agent.key(Key::Char('\n'));
    agent.key(Key::Char('\u{7}'));
    assert_eq!(
        typed(&mut agent, "ok"),
        [Effect::Answer(Zeroizing::new(b"ok".to_vec()))]
    );
}

#[test]
fn the_field_takes_at_most_its_limit() {
    let mut agent = Agent::new(ME);
    agent.begin(request("a", &[ME]));
    asked(&mut agent);
    for _ in 0..MAX_ANSWER + 5 {
        agent.key(Key::Char('a'));
    }
    assert_eq!(agent.prompt().unwrap().field_len(), MAX_ANSWER);
}

#[test]
fn requests_wait_their_turn_and_escape_or_polkitd_cancels() {
    let mut agent = Agent::new(ME);
    agent.begin(request("a", &[ME]));
    assert!(agent.begin(request("b", &[ME])).is_empty());
    assert!(agent.begin(request("c", &[ME])).is_empty());
    assert_eq!(agent.waiting(), 2);
    assert_eq!(agent.cancel("c"), [finish("c", Outcome::Cancelled)]);
    assert_eq!(
        agent.key(Key::Escape),
        [finish("a", Outcome::Cancelled), start(ME, "b")]
    );
    assert_eq!(agent.cancel("b"), [finish("b", Outcome::Cancelled)]);
    assert!(agent.cancel("unknown").is_empty());
    assert!(agent.prompt().is_none());
}

#[test]
fn going_away_dismisses_the_prompt_and_the_rest_wait_until_the_session_is_back() {
    let mut agent = Agent::new(ME);
    agent.begin(request("a", &[ME]));
    agent.begin(request("b", &[ME]));
    asked(&mut agent);
    agent.key(Key::Char('s'));
    assert_eq!(agent.set_away(true), [finish("a", Outcome::Cancelled)]);
    assert!(
        agent.begin(request("c", &[ME])).is_empty(),
        "nothing shows while away"
    );
    assert!(agent.prompt().is_none());
    assert_eq!(agent.set_away(false), [start(ME, "b")]);
    assert_eq!(agent.waiting(), 1);
}

#[test]
fn choosing_another_user_starts_over_and_a_user_polkitd_did_not_offer_is_ignored() {
    let mut agent = Agent::new(ME);
    agent.begin(request("a", &[ME, ROOT]));
    asked(&mut agent);
    agent.key(Key::Char('s'));
    assert!(agent.choose_user(User(4242)).is_empty());
    assert!(agent.choose_user(ME).is_empty());
    assert_eq!(agent.choose_user(ROOT), [start(ROOT, "a")]);
    let prompt = agent.prompt().unwrap();
    assert_eq!(
        (prompt.user, prompt.phase, prompt.field_len()),
        (ROOT, Phase::Waiting, 0)
    );
}
