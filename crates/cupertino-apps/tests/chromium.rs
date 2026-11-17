//! Programs built on Chromium: finding them through their launchers and turning the overlay scroll bars on.

mod common;

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

fn with(items: &[&str], flags: FlagsSwitches) -> Vec<String> {
    let mut list = args(items);
    with_overlay_scroll_bars(&mut list, &flags);
    list
}

fn plain(items: &[&str]) -> Vec<String> {
    with(items, FlagsSwitches::default())
}

#[test]
fn an_executable_beside_the_resource_file_is_chromium() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = chromium_build(dir.path());
    assert!(is_chromium(chrome.to_str().unwrap(), &[]));
    assert!(launcher(chrome.to_str().unwrap(), &[]).is_some());
}

#[test]
fn an_executable_one_folder_below_the_resource_file_is_chromium() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app/chrome_100_percent.pak", "");
    let chrome = executable(dir.path(), "app/bin/chrome", "binary");
    assert!(is_chromium(chrome.to_str().unwrap(), &[]));
}

#[test]
fn a_program_is_looked_up_on_the_search_path_by_name() {
    let dir = tempfile::tempdir().unwrap();
    chromium_build(dir.path());
    assert!(is_chromium("chrome", &[dir.path().join("app")]));
    assert!(!is_chromium("chrome", &[dir.path().join("elsewhere")]));
}

#[test]
fn an_executable_without_the_resource_file_is_not_chromium() {
    let dir = tempfile::tempdir().unwrap();
    let tool = executable(dir.path(), "bin/tool", "binary");
    assert!(!is_chromium(tool.to_str().unwrap(), &[]));
}

#[test]
fn a_file_that_is_not_executable_or_does_not_exist_is_not_chromium() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app/chrome_100_percent.pak", "");
    let plain_file = write(dir.path(), "app/chrome", "binary");
    assert!(!is_chromium(plain_file.to_str().unwrap(), &[]));
    assert!(!is_chromium("/nonexistent/chrome", &[]));
}

#[test]
fn a_script_that_runs_chromium_with_the_launch_arguments_is_chromium() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = chromium_build(dir.path());
    let script = executable(
        dir.path(),
        "bin/wrapper",
        &format!("#!/bin/sh\nexec {} \"$@\"\n", chrome.display()),
    );
    assert!(is_chromium(script.to_str().unwrap(), &[]));
}

#[test]
fn a_script_that_does_not_pass_the_arguments_on_is_not_chromium() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = chromium_build(dir.path());
    let script = executable(
        dir.path(),
        "bin/wrapper",
        &format!("#!/bin/sh\nexec {} --fixed\n", chrome.display()),
    );
    assert!(!is_chromium(script.to_str().unwrap(), &[]));
}

#[test]
fn a_script_finds_its_program_through_a_plain_variable_or_beside_itself() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = chromium_build(dir.path());
    let through_variable = executable(
        dir.path(),
        "bin/by-variable",
        &format!(
            "#!/bin/sh\nBIN={}\nexec \"$BIN\" \"$@\"\n",
            chrome.display()
        ),
    );
    assert!(is_chromium(through_variable.to_str().unwrap(), &[]));
    write(dir.path(), "own/chrome_100_percent.pak", "");
    executable(dir.path(), "own/real", "binary");
    let beside = executable(
        dir.path(),
        "own/start",
        "#!/bin/sh\nexec \"$HERE/real\" \"$@\"\n",
    );
    assert!(is_chromium(beside.to_str().unwrap(), &[]));
}

#[test]
fn a_script_may_name_the_process_with_exec_a() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = chromium_build(dir.path());
    let script = executable(
        dir.path(),
        "bin/named",
        &format!("#!/bin/sh\nexec -a custom {} \"$@\"\n", chrome.display()),
    );
    assert!(is_chromium(script.to_str().unwrap(), &[]));
}

#[test]
fn a_small_program_that_names_chromium_by_its_full_path_is_chromium() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = chromium_build(dir.path());
    let launcher_bin = executable(
        dir.path(),
        "bin/native",
        &format!("\u{1}\u{2}junk\0{}\0more junk", chrome.display()),
    );
    assert!(is_chromium(launcher_bin.to_str().unwrap(), &[]));
}

#[test]
fn a_launcher_larger_than_the_read_limit_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = chromium_build(dir.path());
    let mut text = format!("#!/bin/sh\nexec {} \"$@\"\n", chrome.display());
    text.push_str(&"#".repeat(READ as usize));
    let big = executable(dir.path(), "bin/big", &text);
    assert!(!is_chromium(big.to_str().unwrap(), &[]));
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
    assert!(is_chromium(chain[DEPTH - 1].to_str().unwrap(), &[]));
    assert!(!is_chromium(chain[DEPTH].to_str().unwrap(), &[]));
}

#[test]
fn a_script_that_starts_itself_ends() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bin/loop");
    let script = executable(
        dir.path(),
        "bin/loop",
        &format!("#!/bin/sh\nexec {} \"$@\"\n", path.display()),
    );
    assert!(!is_chromium(script.to_str().unwrap(), &[]));
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
    let found = launcher(script.to_str().unwrap(), &[]).unwrap();
    // comments are not read and a plain variable is put in
    assert_eq!(
        found.flags_names(),
        [["second".to_string(), "first".to_string()]]
    );
}

#[test]
fn a_launcher_that_reads_no_flags_file_names_none() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = chromium_build(dir.path());
    assert!(launcher(chrome.to_str().unwrap(), &[])
        .unwrap()
        .flags_names()
        .is_empty());
}

#[test]
fn the_feature_switches_of_a_flags_file_come_from_its_config_folder() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = chromium_build(dir.path());
    let name = "cupertino-test-flags-before";
    let script = executable(
        dir.path(),
        "bin/wrapper",
        &format!(
            "#!/bin/sh\ncat ~/.config/{name}-flags.conf\nexec {} \"$@\"\n",
            chrome.display()
        ),
    );
    write(
        dir.path(),
        &format!("config/{name}-flags.conf"),
        "--enable-features=A,B\n--ozone-platform=wayland\n# --disable-features=Z\n--disable-features=C # tail\n",
    );
    let found = launcher(script.to_str().unwrap(), &[]).unwrap();
    let switches = found.flags_switches(Some(&dir.path().join("config")));
    assert_eq!(
        switches.before,
        ["--enable-features=A,B", "--disable-features=C"]
    );
    assert!(switches.after.is_empty());
    assert_eq!(found.flags_switches(None), FlagsSwitches::default());
    assert_eq!(
        found.flags_switches(Some(&dir.path().join("no-config"))),
        FlagsSwitches::default()
    );
}

#[test]
fn a_launcher_that_reads_more_after_the_arguments_gets_its_switches_after_them() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = chromium_build(dir.path());
    let name = "cupertino-test-flags-after";
    let script = executable(
        dir.path(),
        "bin/wrapper",
        &format!(
            "#!/bin/sh\nflags=$(cat ~/.config/{name}-flags.conf)\nexec {} \"$@\" $flags\n",
            chrome.display()
        ),
    );
    write(
        dir.path(),
        &format!("config/{name}-flags.conf"),
        "--enable-features=Late\n",
    );
    let found = launcher(script.to_str().unwrap(), &[]).unwrap();
    let switches = found.flags_switches(Some(&dir.path().join("config")));
    assert!(switches.before.is_empty());
    assert_eq!(switches.after, ["--enable-features=Late"]);
}

#[test]
fn a_flags_file_is_read_again_when_the_launcher_changes() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = chromium_build(dir.path());
    let script = executable(
        dir.path(),
        "bin/wrapper",
        &format!("#!/bin/sh\nexec {} \"$@\"\n", chrome.display()),
    );
    assert!(is_chromium(script.to_str().unwrap(), &[]));
    std::fs::write(&script, "#!/bin/sh\nexec /bin/true --fixed\n").unwrap();
    // a changed launcher is inspected again
    assert!(!is_chromium(script.to_str().unwrap(), &[]));
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
fn the_switch_is_added_to_a_launch_without_one() {
    assert_eq!(
        plain(&["--foo"]),
        [
            format!("{ENABLE}{OVERLAY_SCROLL_BARS}"),
            "--foo".to_string()
        ]
    );
    assert_eq!(plain(&[]), [format!("{ENABLE}{OVERLAY_SCROLL_BARS}")]);
}

#[test]
fn the_switch_goes_before_the_double_dash_that_ends_the_options() {
    assert_eq!(
        plain(&["--foo", "--", "file"]),
        [
            "--foo",
            &format!("{ENABLE}{OVERLAY_SCROLL_BARS}"),
            "--",
            "file"
        ]
    );
}

#[test]
fn arguments_after_the_double_dash_do_not_count() {
    assert_eq!(
        plain(&["--", "--disable-features=OverlayScrollbar"]),
        [
            &format!("{ENABLE}{OVERLAY_SCROLL_BARS}"),
            "--",
            "--disable-features=OverlayScrollbar"
        ]
    );
}

#[test]
fn the_overlay_feature_joins_the_list_the_launch_already_has() {
    assert_eq!(
        plain(&["--enable-features=A,B"]),
        ["--enable-features=A,B,OverlayScrollbar"]
    );
}

#[test]
fn the_features_of_every_list_are_merged_into_the_last_one() {
    assert_eq!(
        plain(&["--enable-features=A", "--x", "--enable-features=B,A,"]),
        ["--x", "--enable-features=A,B,OverlayScrollbar"]
    );
}

#[test]
fn the_features_of_the_flags_files_come_before_those_of_the_launch() {
    let flags = FlagsSwitches {
        before: args(&["--enable-features=F"]),
        after: Vec::new(),
    };
    assert_eq!(
        with(&["--enable-features=A"], flags),
        ["--enable-features=F,A,OverlayScrollbar"]
    );
}

#[test]
fn a_launch_that_already_turns_the_feature_on_is_left_alone() {
    for named in [
        "--enable-features=OverlayScrollbar",
        "--enable-features=A,*OverlayScrollbar",
        "--enable-features=OverlayScrollbar:opt/1",
        "--enable-features=OverlayScrollbar<Study",
    ] {
        assert_eq!(plain(&[named]), [named], "{named}");
    }
}

#[test]
fn a_launch_that_turns_the_feature_off_is_left_alone() {
    assert_eq!(
        plain(&["--disable-features=OverlayScrollbar"]),
        ["--disable-features=OverlayScrollbar"]
    );
    let flags = FlagsSwitches {
        before: args(&["--disable-features=X,OverlayScrollbar"]),
        after: Vec::new(),
    };
    assert_eq!(with(&["--foo"], flags), ["--foo"]);
}

#[test]
fn a_list_among_the_switches_put_after_the_launch_leaves_it_as_it_is() {
    let flags = FlagsSwitches {
        before: Vec::new(),
        after: args(&["--enable-features=Late"]),
    };
    assert_eq!(with(&["--foo"], flags), ["--foo"]);
}

#[test]
fn the_feature_is_added_when_it_is_named_in_a_list_that_is_not_the_last() {
    assert_eq!(
        plain(&["--enable-features=OverlayScrollbar", "--enable-features=B"]),
        ["--enable-features=OverlayScrollbar,B"]
    );
}

#[test]
fn launch_args_turn_the_feature_on_for_a_chromium_program_only() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = chromium_build(dir.path());
    let tool = executable(dir.path(), "bin/tool", "binary");
    let mut chromium_args = args(&["--foo"]);
    launch_args(chrome.to_str().unwrap(), &mut chromium_args);
    assert_eq!(
        chromium_args,
        [&format!("{ENABLE}{OVERLAY_SCROLL_BARS}"), "--foo"]
    );
    let mut tool_args = args(&["--foo"]);
    launch_args(tool.to_str().unwrap(), &mut tool_args);
    assert_eq!(tool_args, ["--foo"]);
}
