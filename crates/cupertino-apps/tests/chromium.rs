//! Programs built on Chromium: finding them through their launchers and turning the overlay scroll bars on.

use crate::common;

use common::{executable, write};
use cupertino_apps::chromium::{
    feature_switches, is_chromium, launch_args, launcher, with_overlay_scroll_bars, FlagsSwitches,
    DEPTH, OVERLAY_SCROLL_BARS, READ,
};
use std::path::{Path, PathBuf};

const ENABLE: &str = "--enable-features=";

/// A Chromium build in `root/app`: its executable beside the resource file every build ships.
fn chromium_build(root: &Path) -> PathBuf {
    write(root, "app/chrome_100_percent.pak", "");
    executable(root, "app/chrome", "\u{7f}ELF binary")
}

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn a_program_is_chromium_when_it_or_the_launcher_chain_reaches_a_chromium_build() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let chrome = chromium_build(root);
    let c = chrome.display();
    let below = executable(root, "app/bin/chrome", "binary");
    let tool = executable(root, "bin/tool", "binary");
    write(root, "plain/chrome_100_percent.pak", "");
    let not_executable = write(root, "plain/chrome", "binary");
    write(root, "own/chrome_100_percent.pak", "");
    executable(root, "own/real", "binary");
    let script = |name: &str, body: String| {
        executable(
            root,
            &format!("bin/{name}"),
            &format!("#!/bin/sh\n{body}\n"),
        )
    };
    let mut big = format!("exec {c} \"$@\"\n");
    big.push_str(&"#".repeat(READ as usize));
    let loop_path = root.join("bin/loop");
    let cases: Vec<(&str, PathBuf, bool)> = vec![
        ("the build itself", chrome.clone(), true),
        ("one folder below the resource file", below, true),
        ("no resource file", tool, false),
        ("not executable", not_executable, false),
        ("missing", PathBuf::from("/nonexistent/chrome"), false),
        (
            "a script passing the arguments",
            script("wrapper", format!("exec {c} \"$@\"")),
            true,
        ),
        (
            "a script with fixed arguments",
            script("fixed", format!("exec {c} --fixed")),
            false,
        ),
        (
            "through a variable",
            script("by-variable", format!("BIN={c}\nexec \"$BIN\" \"$@\"")),
            true,
        ),
        (
            "exec -a",
            script("named", format!("exec -a custom {c} \"$@\"")),
            true,
        ),
        (
            "beside itself",
            executable(root, "own/start", "#!/bin/sh\nexec \"$HERE/real\" \"$@\"\n"),
            true,
        ),
        (
            "a small program naming the full path",
            executable(
                root,
                "bin/native",
                &format!("\u{1}\u{2}junk\0{c}\0more junk"),
            ),
            true,
        ),
        ("larger than the read limit", script("big", big), false),
        (
            "starts itself",
            script("loop", format!("exec {} \"$@\"", loop_path.display())),
            false,
        ),
    ];
    for (label, program, expected) in &cases {
        assert_eq!(is_chromium(path(program), &[]), *expected, "{label}");
    }
    assert!(launcher(path(&chrome), &[]).is_some());
    assert!(
        is_chromium("chrome", &[root.join("app")]),
        "found on the search path"
    );
    assert!(!is_chromium("chrome", &[root.join("elsewhere")]));
}

#[test]
fn launchers_are_followed_up_to_the_depth_limit() {
    let dir = tempfile::tempdir().unwrap();
    let mut target = chromium_build(dir.path());
    let mut chain = Vec::new();
    for level in 1..=DEPTH + 1 {
        target = executable(
            dir.path(),
            &format!("bin/level{level}"),
            &format!("#!/bin/sh\nexec {} \"$@\"\n", target.display()),
        );
        chain.push(target.clone());
    }
    assert!(is_chromium(path(&chain[DEPTH - 1]), &[]));
    assert!(!is_chromium(path(&chain[DEPTH]), &[]));
}

#[test]
fn a_changed_launcher_is_inspected_again() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = chromium_build(dir.path());
    let script = executable(
        dir.path(),
        "bin/wrapper",
        &format!("#!/bin/sh\nexec {} \"$@\"\n", chrome.display()),
    );
    assert!(is_chromium(path(&script), &[]));
    std::fs::write(&script, "#!/bin/sh\nexec /bin/true --fixed\n").unwrap();
    assert!(!is_chromium(path(&script), &[]));
}

#[test]
fn a_launcher_names_the_flags_files_it_reads_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = chromium_build(dir.path());
    let script = executable(
        dir.path(),
        "bin/wrapper",
        &format!(
            "#!/bin/sh\n# reads ~/.config/ignored-flags.conf\nNAME=second\ncat \"$XDG_CONFIG_HOME/$NAME-flags.conf\" \
             first-flags.conf\nexec {} \"$@\"\n",
            chrome.display()
        ),
    );
    // comments are not read and a plain variable is put in
    assert_eq!(
        launcher(path(&script), &[]).unwrap().flags_names(),
        [["second".to_string(), "first".to_string()]]
    );
    assert!(launcher(path(&chrome), &[])
        .unwrap()
        .flags_names()
        .is_empty());
}

#[test]
fn the_feature_switches_of_a_flags_file_go_before_or_after_the_arguments_as_the_launcher_reads_it()
{
    let dir = tempfile::tempdir().unwrap();
    let chrome = chromium_build(dir.path());
    let config = dir.path().join("config");
    for (name, body, contents, before, after) in [
        (
            "cupertino-test-flags-before",
            "cat ~/.config/{name}-flags.conf\nexec {chrome} \"$@\"",
            "--enable-features=A,B\n--ozone-platform=wayland\n# --disable-features=Z\n--disable-features=C # tail\n",
            &["--enable-features=A,B", "--disable-features=C"][..],
            &[][..],
        ),
        (
            "cupertino-test-flags-after",
            "flags=$(cat ~/.config/{name}-flags.conf)\nexec {chrome} \"$@\" $flags",
            "--enable-features=Late\n",
            &[],
            &["--enable-features=Late"],
        ),
    ] {
        let body = body.replace("{name}", name).replace("{chrome}", &chrome.display().to_string());
        let script = executable(dir.path(), &format!("bin/{name}"), &format!("#!/bin/sh\n{body}\n"));
        write(&config, &format!("{name}-flags.conf"), contents);
        let found = launcher(path(&script), &[]).unwrap();
        let switches = found.flags_switches(Some(&config));
        assert_eq!(switches.before, before, "{name}");
        assert_eq!(switches.after, after, "{name}");
        assert_eq!(found.flags_switches(None), FlagsSwitches::default(), "{name}");
        assert_eq!(found.flags_switches(Some(&dir.path().join("no-config"))), FlagsSwitches::default(), "{name}");
    }
}

#[test]
fn feature_switches_are_the_enable_and_disable_words_before_a_comment() {
    let text = "--enable-features=A\n  \"--disable-features=B\" --other # --enable-features=C\n\
                # --enable-features=D\n'--enable-features=E' --disable-features=F\n";
    assert_eq!(
        feature_switches(text),
        [
            "--enable-features=A",
            "--disable-features=B",
            "--enable-features=E",
            "--disable-features=F"
        ]
    );
    assert!(feature_switches("").is_empty());
}

#[test]
fn the_overlay_feature_is_merged_into_the_launch_unless_it_is_already_decided() {
    let on = format!("{ENABLE}{OVERLAY_SCROLL_BARS}");
    let flags = |before: &[&str], after: &[&str]| FlagsSwitches {
        before: args(before),
        after: args(after),
    };
    let none = FlagsSwitches::default;
    for (label, launch, flags, expected) in [
        (
            "added first",
            &["--foo"][..],
            none(),
            vec![on.as_str(), "--foo"],
        ),
        ("added to nothing", &[], none(), vec![on.as_str()]),
        (
            "before the double dash",
            &["--foo", "--", "file"],
            none(),
            vec!["--foo", &on, "--", "file"],
        ),
        (
            "after the double dash does not count",
            &["--", "--disable-features=OverlayScrollbar"],
            none(),
            vec![&on, "--", "--disable-features=OverlayScrollbar"],
        ),
        (
            "joins a list",
            &["--enable-features=A,B"],
            none(),
            vec!["--enable-features=A,B,OverlayScrollbar"],
        ),
        (
            "lists merge into the last",
            &["--enable-features=A", "--x", "--enable-features=B,A,"],
            none(),
            vec!["--x", "--enable-features=A,B,OverlayScrollbar"],
        ),
        (
            "flags files first",
            &["--enable-features=A"],
            flags(&["--enable-features=F"], &[]),
            vec!["--enable-features=F,A,OverlayScrollbar"],
        ),
        (
            "named in a list that is not the last",
            &["--enable-features=OverlayScrollbar", "--enable-features=B"],
            none(),
            vec!["--enable-features=OverlayScrollbar,B"],
        ),
        (
            "already on",
            &["--enable-features=OverlayScrollbar"],
            none(),
            vec!["--enable-features=OverlayScrollbar"],
        ),
        (
            "on with a star",
            &["--enable-features=A,*OverlayScrollbar"],
            none(),
            vec!["--enable-features=A,*OverlayScrollbar"],
        ),
        (
            "on with a parameter",
            &["--enable-features=OverlayScrollbar:opt/1"],
            none(),
            vec!["--enable-features=OverlayScrollbar:opt/1"],
        ),
        (
            "on with a study",
            &["--enable-features=OverlayScrollbar<Study"],
            none(),
            vec!["--enable-features=OverlayScrollbar<Study"],
        ),
        (
            "turned off",
            &["--disable-features=OverlayScrollbar"],
            none(),
            vec!["--disable-features=OverlayScrollbar"],
        ),
        (
            "turned off by a flags file",
            &["--foo"],
            flags(&["--disable-features=X,OverlayScrollbar"], &[]),
            vec!["--foo"],
        ),
        (
            "a list after the launch",
            &["--foo"],
            flags(&[], &["--enable-features=Late"]),
            vec!["--foo"],
        ),
    ] {
        let mut list = args(launch);
        with_overlay_scroll_bars(&mut list, &flags);
        assert_eq!(list, expected, "{label}");
    }
}

#[test]
fn launch_args_turn_the_feature_on_for_a_chromium_program_only() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = chromium_build(dir.path());
    let tool = executable(dir.path(), "bin/tool", "binary");
    let mut chromium_args = args(&["--foo"]);
    launch_args(path(&chrome), &mut chromium_args);
    assert_eq!(
        chromium_args,
        [&format!("{ENABLE}{OVERLAY_SCROLL_BARS}"), "--foo"]
    );
    let mut tool_args = args(&["--foo"]);
    launch_args(path(&tool), &mut tool_args);
    assert_eq!(tool_args, ["--foo"]);
}
