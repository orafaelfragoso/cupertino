//! `Exec` quoting, field codes and the command an entry turns into.

use crate::common;

use common::{desktop, entry, write};
use cupertino_apps::exec::{
    command_for_action, command_for_entry, expand_args, strip_field_codes, tokenize, CommandSpec,
    ExpandContext, ACTIVATION_TOKEN_ENV, STARTUP_ID_ENV,
};
use std::path::Path;

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn context<'a>(files: &'a [String]) -> ExpandContext<'a> {
    ExpandContext {
        name: "Name",
        icon: Some("icon"),
        desktop_path: Some("/apps/a.desktop"),
        files,
    }
}

#[test]
fn an_exec_line_splits_into_arguments_by_the_quoting_rules() {
    for (line, expected) in [
        ("foo  bar\tbaz", Some(&["foo", "bar", "baz"][..])),
        ("", Some(&[][..])),
        ("   ", Some(&[][..])),
        ("foo \"a b\" c", Some(&["foo", "a b", "c"][..])),
        ("pre\"a b\"post", Some(&["prea bpost"][..])),
        ("foo \"\"", Some(&["foo", ""][..])),
        (r#""a\"b""#, Some(&["a\"b"][..])),
        (r#""a\\b" "c\$d" "e\`f""#, Some(&["a\\b", "c$d", "e`f"][..])),
        (r#""a\nb""#, Some(&["a\\nb"][..])),
        ("foo \"bar", None),
    ] {
        assert_eq!(tokenize(line), expected.map(strings), "{line:?}");
    }
}

#[test]
fn field_codes_expand_from_the_files_name_icon_and_path() {
    let two = strings(&["/a", "/b"]);
    let spaced = strings(&["/a b", "/c"]);
    let none: Vec<String> = Vec::new();
    for (args, files, expected) in [
        (
            &["app", "%f", "--u=%u"][..],
            &two,
            &["app", "/a", "--u=/a"][..],
        ),
        (&["app", "%f", "--u=%u"], &none, &["app", "--u="]),
        (&["app", "%F"], &spaced, &["app", "/a b", "/c"]),
        (&["app", "%U"], &none, &["app"]),
        (&["--files=%F"], &two, &["--files=/a /b"]),
        (&["app", "%i"], &none, &["app", "--icon", "icon"]),
        (
            &["--title=%c", "%k"],
            &none,
            &["--title=Name", "/apps/a.desktop"],
        ),
        (&["100%%", "%d", "x%Dy", "%"], &none, &["100%", "xy"]),
    ] {
        assert_eq!(
            expand_args(&strings(args), &context(files)),
            expected,
            "{args:?}"
        );
    }
    let without_icon = ExpandContext {
        icon: None,
        ..context(&none)
    };
    assert_eq!(
        expand_args(&strings(&["app", "%i"]), &without_icon),
        ["app"]
    );
}

#[test]
fn stripping_field_codes_gives_the_plain_launch_or_none() {
    for (exec, expected) in [
        ("app --flag %U", Some(&["app", "--flag"][..])),
        ("", None),
        ("\"app", None),
        ("%U", None),
    ] {
        assert_eq!(
            strip_field_codes(exec, &context(&[])),
            expected.map(strings),
            "{exec:?}"
        );
    }
}

#[test]
fn a_command_is_its_program_and_arguments_optionally_inside_a_terminal() {
    assert_eq!(
        CommandSpec::new("app", &["-a", "b"]).argv(),
        ["app", "-a", "b"]
    );
    assert_eq!(
        CommandSpec::from_argv(strings(&["x", "y"])).unwrap().argv(),
        ["x", "y"]
    );
    assert_eq!(CommandSpec::from_argv(Vec::new()), None);

    let mut spec = CommandSpec::new("app", &["x"]);
    spec.terminal = true;
    let wrapped = spec.in_terminal(&strings(&["xterm", "-e"]));
    assert_eq!(wrapped.argv(), ["xterm", "-e", "app", "x"]);
    assert!(!wrapped.terminal);
    assert_eq!(
        CommandSpec::new("app", &["x"]).in_terminal(&[]).argv(),
        ["app", "x"]
    );
}

#[test]
fn the_process_command_carries_the_folder_and_the_latest_activation_token_in_both_variables() {
    let mut spec = CommandSpec::new("app", &["x"])
        .with_activation_token("old")
        .with_activation_token("new");
    for name in [ACTIVATION_TOKEN_ENV, STARTUP_ID_ENV] {
        let values: Vec<_> = spec
            .env
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(values, ["new"], "{name}");
    }
    spec.working_dir = Some("/tmp".into());
    let command = spec.to_command();
    assert_eq!(command.get_program(), "app");
    assert_eq!(command.get_args().collect::<Vec<_>>(), ["x"]);
    assert_eq!(command.get_current_dir(), Some(Path::new("/tmp")));
    let token = command
        .get_envs()
        .find(|(k, _)| *k == ACTIVATION_TOKEN_ENV)
        .and_then(|(_, v)| v);
    assert_eq!(token, Some(std::ffi::OsStr::new("new")));
}

#[test]
fn an_entry_becomes_a_command_with_its_folder_id_terminal_flag_and_files() {
    let e = entry(
        "a.desktop",
        &desktop(
            "Tool",
            "tool-for-tests-only %F --icon-name=%c",
            "Path=/work\nTerminal=true",
        ),
    );
    let spec = command_for_entry(&e, &[]).unwrap();
    assert_eq!(spec.argv(), ["tool-for-tests-only", "--icon-name=Tool"]);
    assert_eq!(spec.working_dir.as_deref(), Some(Path::new("/work")));
    assert_eq!(spec.desktop_id.as_deref(), Some("a.desktop"));
    assert!(spec.terminal);
    assert!(spec.env.is_empty());

    let dir = tempfile::tempdir().unwrap();
    let mut e = entry(
        "a.desktop",
        &desktop("Tool", "tool-for-tests-only %i %k %F", "Icon=ic"),
    );
    e.source = Some(write(dir.path(), "a.desktop", ""));
    let source = e.source.clone().unwrap().display().to_string();
    assert_eq!(
        command_for_entry(&e, &strings(&["/x", "/y"]))
            .unwrap()
            .argv(),
        ["tool-for-tests-only", "--icon", "ic", &source, "/x", "/y"]
    );

    for exec in ["\"unbalanced", "%U"] {
        assert!(
            command_for_entry(&entry("a", &desktop("A", exec, "")), &[]).is_none(),
            "{exec}"
        );
    }
}

#[test]
fn an_action_launches_its_own_exec_with_its_icon_else_the_entrys() {
    let text =
        format!(
        "{}[Desktop Action own]\nName=Own\nExec=tool-for-tests-only --own %i\nIcon=action-icon\n\
         [Desktop Action plain]\nName=Plain\nExec=tool-for-tests-only --plain %i\n",
        desktop("A", "tool-for-tests-only", "Icon=entry-icon\nActions=own;plain;\nPath=/w")
    );
    let e = entry("a.desktop", &text);
    let own = command_for_action(&e, &e.actions[0]).unwrap();
    assert_eq!(
        own.argv(),
        ["tool-for-tests-only", "--own", "--icon", "action-icon"]
    );
    let plain = command_for_action(&e, &e.actions[1]).unwrap();
    assert_eq!(
        plain.argv(),
        ["tool-for-tests-only", "--plain", "--icon", "entry-icon"]
    );
    assert_eq!(plain.working_dir.as_deref(), Some(Path::new("/w")));
    assert_eq!(plain.desktop_id.as_deref(), Some("a.desktop"));
}
