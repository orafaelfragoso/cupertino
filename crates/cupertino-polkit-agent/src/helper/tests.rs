use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;

use super::*;

#[test]
fn each_kind_of_line_is_read_and_unescaped() {
    assert_eq!(
        parse(b"PAM_PROMPT_ECHO_OFF Password: ").unwrap(),
        Line::Prompt {
            text: "Password: ".into(),
            echo: false
        }
    );
    assert_eq!(
        parse(b"PAM_PROMPT_ECHO_ON Login:").unwrap(),
        Line::Prompt {
            text: "Login:".into(),
            echo: true
        }
    );
    assert_eq!(
        parse(br"PAM_ERROR_MSG Your password has expired.\nChange it now.").unwrap(),
        Line::Error("Your password has expired.\nChange it now.".into())
    );
    assert_eq!(
        parse(br#"PAM_TEXT_INFO Place your \"finger\" \303\251 \\ on the reader"#).unwrap(),
        Line::Info("Place your \"finger\" é \\ on the reader".into())
    );
    assert_eq!(parse(b"SUCCESS").unwrap(), Line::Success);
    assert_eq!(parse(b"FAILURE").unwrap(), Line::Failure);
    assert!(matches!(
        parse(b"PAM_SOMETHING x"),
        Err(HelperError::Unknown)
    ));
}

#[test]
fn the_socket_comes_first_then_the_first_installed_binary() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("agent-helper.socket");
    let first = dir.path().join("lib-helper");
    let second = dir.path().join("libexec-helper");
    std::fs::write(&second, b"").unwrap();
    let binaries = [first.as_path(), second.as_path()];
    assert_eq!(
        Route::find_in(&socket, &binaries),
        [Route::Binary(second.clone())]
    );
    std::fs::write(&first, b"").unwrap();
    assert_eq!(
        Route::find_in(&socket, &binaries),
        [Route::Binary(first.clone())]
    );
    let _listener = UnixListener::bind(&socket).unwrap();
    assert_eq!(
        Route::find_in(&socket, &binaries),
        [Route::Socket(socket.clone()), Route::Binary(first.clone())]
    );
    assert_eq!(Route::find_in(&dir.path().join("none"), &[]), []);
}

#[test]
fn over_the_socket_the_user_and_cookie_come_first_and_the_answer_follows_the_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent-helper.socket");
    let listener = UnixListener::bind(&path).unwrap();
    let service = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
        let mut stream = stream;
        let user = lines.next().unwrap().unwrap();
        let cookie = lines.next().unwrap().unwrap();
        writeln!(stream, "PAM_PROMPT_ECHO_OFF Password: ").unwrap();
        let answer = lines.next().unwrap().unwrap();
        writeln!(stream, "SUCCESS").unwrap();
        (user, cookie, answer)
    });
    let mut helper = Helper::start(&[Route::Socket(path)], "ada", "cookie-1").unwrap();
    assert_eq!(
        helper.read_line().unwrap(),
        Line::Prompt {
            text: "Password: ".into(),
            echo: false
        }
    );
    helper.answer(b"hunter2").unwrap();
    assert_eq!(helper.read_line().unwrap(), Line::Success);
    assert_eq!(
        service.join().unwrap(),
        ("ada".into(), "cookie-1".into(), "hunter2".into())
    );
}

/// A helper binary that reports its command line and environment, then checks the password.
fn fake_binary(dir: &Path) -> PathBuf {
    let path = dir.join("polkit-agent-helper-1");
    std::fs::write(
        &path,
        "#!/bin/sh\n\
         read cookie\n\
         echo \"PAM_TEXT_INFO user=$1 args=$# cookie=$cookie home=${HOME:-none}\"\n\
         echo 'PAM_PROMPT_ECHO_OFF Password: '\n\
         read password\n\
         if [ \"$password\" = right ]; then echo SUCCESS; else echo FAILURE; fi\n",
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn a_started_helper_gets_only_the_user_on_its_command_line_and_no_environment() {
    let dir = tempfile::tempdir().unwrap();
    let route = Route::Binary(fake_binary(dir.path()));
    let mut helper = Helper::start(std::slice::from_ref(&route), "ada", "cookie-2").unwrap();
    assert_eq!(
        helper.read_line().unwrap(),
        Line::Info("user=ada args=1 cookie=cookie-2 home=none".into())
    );
    assert!(matches!(
        helper.read_line().unwrap(),
        Line::Prompt { echo: false, .. }
    ));
    helper.answer(b"wrong").unwrap();
    assert_eq!(helper.read_line().unwrap(), Line::Failure);
    assert!(matches!(helper.read_line(), Err(HelperError::Closed)));

    let mut helper = Helper::start(std::slice::from_ref(&route), "ada", "cookie-3").unwrap();
    helper.read_line().unwrap();
    helper.read_line().unwrap();
    helper.answer(b"right").unwrap();
    assert_eq!(helper.read_line().unwrap(), Line::Success);
}

#[test]
fn an_answer_with_a_line_break_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut helper = Helper::start(&[Route::Binary(fake_binary(dir.path()))], "ada", "c").unwrap();
    assert_eq!(
        helper.answer(b"right\nSUCCESS").unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
}

#[test]
fn an_overlong_line_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent-helper.socket");
    let listener = UnixListener::bind(&path).unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let line = format!("PAM_TEXT_INFO {}\n", "x".repeat(MAX_LINE));
        let _ = stream.write_all(line.as_bytes());
    });
    let mut helper = Helper::start(&[Route::Socket(path)], "ada", "c").unwrap();
    assert!(matches!(helper.read_line(), Err(HelperError::TooLong)));
}

#[test]
fn dropping_a_helper_stops_one_waiting_for_an_answer() {
    let dir = tempfile::tempdir().unwrap();
    let mut helper = Helper::start(&[Route::Binary(fake_binary(dir.path()))], "ada", "c").unwrap();
    helper.read_line().unwrap();
    helper.read_line().unwrap();
    let started = std::time::Instant::now();
    drop(helper);
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn a_socket_nothing_listens_on_falls_back_to_the_binary() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("agent-helper.socket");
    drop(UnixListener::bind(&socket).unwrap());
    let routes = [
        Route::Socket(socket),
        Route::Binary(fake_binary(dir.path())),
    ];
    let mut helper = Helper::start(&routes, "ada", "cookie-4").unwrap();
    assert_eq!(
        helper.read_line().unwrap(),
        Line::Info("user=ada args=1 cookie=cookie-4 home=none".into())
    );
    assert!(Helper::start(&routes[..1], "ada", "c").is_err());
}
