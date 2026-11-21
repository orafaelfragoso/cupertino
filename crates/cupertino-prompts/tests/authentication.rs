//! The authentication agent's prompt through the crate's interface: the model from polkitd's request to its answer
//! (`cupertino_prompts::polkit::model`), the conversation with polkit's helper over its socket
//! (`cupertino_prompts::polkit::helper`) against a stand-in helper, the rows of a request's details and the field's
//! label. The bus side of the agent and the alert on screen are tested as a program
//! (`crates/cupertino-session-tests/tests/prompts.rs`).
#![expect(
    clippy::disallowed_methods,
    reason = "a test runs its stand-in helper on its own thread and awaits the helper's lines on it"
)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use cupertino_prompts::polkit::helper::{parse, Helper, HelperError, Line, Route};
use cupertino_prompts::polkit::model::{Agent, Effect, Phase, ATTEMPTS, MAX_ANSWER, TRY_AGAIN};
use cupertino_prompts::polkit::{detail_rows, Outcome, Request, User};
use cupertino_prompts::view::field_label;
use futures_lite::future::block_on;

const ME: User = User(1000);
const ROOT: User = User(0);

fn request(cookie: &str, users: &[User]) -> Request {
    Request {
        cookie: cookie.to_owned(),
        action: "org.example.do".to_owned(),
        message: "Authentication is required to do it.".to_owned(),
        details: Vec::new(),
        users: users.to_vec(),
    }
}

fn started(effects: &[Effect]) -> Option<(User, String)> {
    effects.iter().find_map(|e| match e {
        Effect::StartHelper { user, cookie } => Some((*user, cookie.clone())),
        _ => None,
    })
}

fn finished(effects: &[Effect]) -> Vec<(String, Outcome)> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Finish { cookie, outcome } => Some((cookie.clone(), *outcome)),
            _ => None,
        })
        .collect()
}

fn asked(agent: &mut Agent) {
    agent.helper(Line::Prompt {
        text: "Password: ".to_owned(),
        echo: false,
    });
}

#[test]
fn a_request_starts_the_helper_for_the_session_user_when_polkitd_accepts_them() {
    let mut agent = Agent::new(ME);
    let effects = agent.begin(request("a", &[ROOT, ME]));
    assert_eq!(started(&effects), Some((ME, "a".to_owned())));
    let mut other = Agent::new(ME);
    let effects = other.begin(request("b", &[ROOT]));
    assert_eq!(started(&effects), Some((ROOT, "b".to_owned())));
    assert_eq!(other.prompt().unwrap().phase, Phase::Waiting);
}

#[test]
fn the_question_takes_an_answer_once_and_success_answers_polkitd() {
    let mut agent = Agent::new(ME);
    agent.begin(request("a", &[ME]));
    // nothing goes to the helper before it asks
    assert!(agent.submit(b"secret").is_empty());
    asked(&mut agent);
    let prompt = agent.prompt().unwrap();
    assert_eq!(
        (prompt.phase, prompt.echo, prompt.asked),
        (Phase::Asking, false, 1)
    );
    let effects = agent.submit(b"hunter2");
    match effects.as_slice() {
        [Effect::Answer(answer)] => assert_eq!(answer.as_slice(), b"hunter2"),
        other => panic!("one answer: {other:?}"),
    }
    // the answer never prints
    assert!(!format!("{effects:?}").contains("hunter2"));
    assert_eq!(agent.prompt().unwrap().phase, Phase::Checking);
    assert!(agent.submit(b"again").is_empty(), "one answer per question");
    let effects = agent.helper(Line::Success);
    assert_eq!(finished(&effects), vec![("a".to_owned(), Outcome::Done)]);
    assert!(agent.prompt().is_none());
}

#[test]
fn an_answer_that_is_empty_too_long_or_broken_by_a_line_break_is_not_sent() {
    let mut agent = Agent::new(ME);
    agent.begin(request("a", &[ME]));
    asked(&mut agent);
    assert!(agent.submit(b"").is_empty());
    assert!(agent.submit(b"two\nlines").is_empty());
    assert!(agent.submit(&vec![b'x'; MAX_ANSWER + 1]).is_empty());
    assert_eq!(agent.prompt().unwrap().phase, Phase::Asking);
    assert_eq!(agent.submit(&vec![b'x'; MAX_ANSWER]).len(), 1);
}

#[test]
fn a_refused_answer_asks_again_and_the_last_attempt_ends_the_authentication() {
    let mut agent = Agent::new(ME);
    agent.begin(request("a", &[ME]));
    for attempt in 1..ATTEMPTS {
        asked(&mut agent);
        agent.submit(b"wrong");
        let effects = agent.helper(Line::Failure);
        assert_eq!(
            started(&effects),
            Some((ME, "a".to_owned())),
            "attempt {attempt}"
        );
        let prompt = agent.prompt().unwrap();
        assert_eq!(prompt.error.as_deref(), Some(TRY_AGAIN));
        assert_eq!(prompt.failures(), attempt);
    }
    // an edit takes the error away
    asked(&mut agent);
    agent.edited();
    assert_eq!(agent.prompt().unwrap().error, None);
    agent.submit(b"wrong");
    let effects = agent.helper(Line::Failure);
    assert_eq!(finished(&effects), vec![("a".to_owned(), Outcome::Done)]);
    assert!(agent.prompt().is_none());
}

#[test]
fn a_lost_helper_counts_as_a_failed_attempt() {
    let mut agent = Agent::new(ME);
    agent.begin(request("a", &[ME]));
    let effects = agent.helper_lost();
    assert!(started(&effects).is_some());
    assert_eq!(agent.prompt().unwrap().failures(), 1);
}

#[test]
fn pam_messages_show_and_a_question_that_shows_its_text_echoes() {
    let mut agent = Agent::new(ME);
    agent.begin(request("a", &[ME]));
    agent.helper(Line::Info("Touch the reader".to_owned()));
    agent.helper(Line::Error("Account expires soon".to_owned()));
    agent.helper(Line::Prompt {
        text: "Code: ".to_owned(),
        echo: true,
    });
    let prompt = agent.prompt().unwrap();
    assert_eq!(prompt.info.as_deref(), Some("Touch the reader"));
    assert_eq!(prompt.error.as_deref(), Some("Account expires soon"));
    assert!(prompt.echo);
    assert_eq!(field_label(&prompt.question), "Code");
    assert_eq!(field_label("Password: "), "Password");
    assert_eq!(field_label(""), "Password");
}

#[test]
fn requests_wait_their_turn_and_polkitd_withdraws_them() {
    let mut agent = Agent::new(ME);
    agent.begin(request("a", &[ME]));
    assert!(agent.begin(request("b", &[ME])).is_empty());
    assert!(agent.begin(request("c", &[ME])).is_empty());
    assert_eq!(agent.waiting(), 2);
    // a waiting one goes without a prompt
    let effects = agent.cancel("c");
    assert_eq!(
        finished(&effects),
        vec![("c".to_owned(), Outcome::Cancelled)]
    );
    // the one on screen goes, and the next one shows
    let effects = agent.cancel("a");
    assert_eq!(
        finished(&effects),
        vec![("a".to_owned(), Outcome::Cancelled)]
    );
    assert_eq!(started(&effects), Some((ME, "b".to_owned())));
    // the person dismisses it
    let effects = agent.dismiss();
    assert_eq!(
        finished(&effects),
        vec![("b".to_owned(), Outcome::Cancelled)]
    );
    assert!(agent.prompt().is_none());
    assert!(agent.cancel("nothing").is_empty());
}

#[test]
fn a_session_that_goes_away_cancels_the_prompt_and_shows_the_next_when_back() {
    let mut agent = Agent::new(ME);
    agent.begin(request("a", &[ME]));
    agent.begin(request("b", &[ME]));
    let effects = agent.set_away(true);
    assert_eq!(
        finished(&effects),
        vec![("a".to_owned(), Outcome::Cancelled)]
    );
    assert!(agent.away());
    // nothing shows while away
    assert!(agent.begin(request("c", &[ME])).is_empty());
    assert!(agent.prompt().is_none());
    let effects = agent.set_away(false);
    assert_eq!(started(&effects), Some((ME, "b".to_owned())));
}

#[test]
fn another_user_starts_over_with_a_helper_for_them() {
    let mut agent = Agent::new(ME);
    agent.begin(request("a", &[ME, ROOT]));
    asked(&mut agent);
    let asked_before = agent.prompt().unwrap().asked;
    let effects = agent.choose_user(ROOT);
    assert_eq!(started(&effects), Some((ROOT, "a".to_owned())));
    let prompt = agent.prompt().unwrap();
    assert_eq!((prompt.user, prompt.phase), (ROOT, Phase::Waiting));
    assert!(prompt.asked > asked_before, "a new, empty field");
    // a user polkitd does not accept, or the same one, changes nothing
    assert!(agent.choose_user(User(4242)).is_empty());
    assert!(agent.choose_user(ROOT).is_empty());
}

#[test]
fn the_helpers_lines_read_as_glib_escapes_them() {
    assert_eq!(
        parse(b"PAM_PROMPT_ECHO_OFF Password: ").unwrap(),
        Line::Prompt {
            text: "Password: ".to_owned(),
            echo: false
        }
    );
    assert_eq!(
        parse(b"PAM_TEXT_INFO two\\nlines \\303\\251").unwrap(),
        Line::Info("two\nlines \u{e9}".to_owned())
    );
    assert_eq!(parse(b"SUCCESS").unwrap(), Line::Success);
    assert_eq!(parse(b"FAILURE").unwrap(), Line::Failure);
    assert!(matches!(parse(b"WHAT"), Err(HelperError::Unknown)));
}

/// A folder of the test's own.
fn dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("prompts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_helper_is_found_by_its_socket_then_its_first_installed_binary() {
    let dir = dir("routes");
    let socket = dir.join("helper.socket");
    let binary = dir.join("helper");
    let missing = dir.join("missing");
    assert!(Route::find_in(&socket, &[missing.as_path()]).is_empty());
    std::fs::write(&binary, "").unwrap();
    std::fs::write(&socket, "").unwrap();
    assert_eq!(
        Route::find_in(&socket, &[missing.as_path(), binary.as_path()]),
        vec![Route::Socket(socket.clone()), Route::Binary(binary.clone())]
    );
}

/// A stand-in for polkit's helper on `socket`: it reads the user and the cookie, asks for a password and answers
/// SUCCESS for `password`, else FAILURE; what it read comes back.
fn helper_stand_in(socket: &Path, password: &'static str) -> std::thread::JoinHandle<Vec<String>> {
    let listener = UnixListener::bind(socket).unwrap();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        let mut read = Vec::new();
        for _ in 0..2 {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            read.push(line.trim_end().to_owned());
        }
        writer
            .write_all(b"PAM_TEXT_INFO hello\nPAM_PROMPT_ECHO_OFF Password: \n")
            .unwrap();
        let mut answer = String::new();
        reader.read_line(&mut answer).unwrap();
        let verdict: &[u8] = if answer.trim_end() == password {
            b"SUCCESS\n"
        } else {
            b"FAILURE\n"
        };
        writer.write_all(verdict).unwrap();
        read.push(answer.trim_end().to_owned());
        read
    })
}

#[test]
fn the_helper_hears_the_user_the_cookie_and_the_answer_and_says_how_it_went() {
    // a socket's path has at most 107 bytes, which the target folder may pass
    let dir = std::env::temp_dir().join(format!("prompts-helper-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("helper.socket");
    let stand_in = helper_stand_in(&socket, "correct horse");
    let helper = Helper::start(&[Route::Socket(socket)], "alex", "cookie-1").unwrap();
    let (mut lines, answers) = helper.split();
    let sender = answers.sender();
    block_on(async {
        assert_eq!(
            lines.read_line().await.unwrap(),
            Line::Info("hello".to_owned())
        );
        assert!(matches!(
            lines.read_line().await.unwrap(),
            Line::Prompt { echo: false, .. }
        ));
        assert!(
            sender.answer(b"two\nlines").await.is_err(),
            "a line break is refused"
        );
        sender.answer(b"correct horse").await.unwrap();
        assert_eq!(lines.read_line().await.unwrap(), Line::Success);
    });
    answers.end();
    assert_eq!(
        stand_in.join().unwrap(),
        ["alex", "cookie-1", "correct horse"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_helper_that_does_not_answer_is_no_route() {
    let dir = dir("nobody");
    let gone = Helper::start(&[Route::Socket(dir.join("nobody.socket"))], "alex", "c");
    assert!(gone.is_err());
    assert!(Helper::start(&[], "alex", "c").is_err());
}

#[test]
fn the_details_name_the_executable_the_kernel_gives_for_polkitds_process_ids() {
    let mut asked = request("a", &[ME]);
    asked.details = vec![
        (
            "polkit.subject-pid".to_owned(),
            std::process::id().to_string(),
        ),
        (
            "polkit.caller-pid".to_owned(),
            std::process::id().to_string(),
        ),
    ];
    let rows = detail_rows(&asked);
    let exe = std::fs::read_link("/proc/self/exe").unwrap();
    assert_eq!(rows[0], ("Application", exe.to_string_lossy().into_owned()));
    // the caller is the same program: no row of its own
    assert_eq!(rows[1], ("Action", "org.example.do".to_owned()));
    assert_eq!(rows.len(), 2);
    // a process that does not exist has no row
    asked.details = vec![("polkit.subject-pid".to_owned(), "0".to_owned())];
    assert_eq!(
        detail_rows(&asked),
        vec![("Action", "org.example.do".to_owned())]
    );
}
