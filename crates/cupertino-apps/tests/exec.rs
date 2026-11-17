//! `Exec` quoting, field codes and the command an entry turns into.

mod common;

use common::{desktop, entry, write};
use cupertino_apps::exec::{
    command_for_action, command_for_entry, expand_args, strip_field_codes, tokenize, CommandSpec,
    ExpandContext, ACTIVATION_TOKEN_ENV, STARTUP_ID_ENV,
};

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
fn arguments_split_on_unquoted_spaces_and_tabs() {
    assert_eq!(
        tokenize("foo  bar\tbaz"),
        Some(strings(&["foo", "bar", "baz"]))
    );
    assert_eq!(tokenize(""), Some(Vec::new()));
    assert_eq!(tokenize("   "), Some(Vec::new()));
}

#[test]
fn double_quotes_group_an_argument() {
    assert_eq!(
        tokenize("foo \"a b\" c"),
        Some(strings(&["foo", "a b", "c"]))
    );
    assert_eq!(tokenize("pre\"a b\"post"), Some(strings(&["prea bpost"])));
}

#[test]
fn an_empty_quoted_argument_is_kept() {
    assert_eq!(tokenize("foo \"\""), Some(strings(&["foo", ""])));
}

#[test]
fn quoted_escapes_are_resolved_and_other_backslashes_kept() {
    assert_eq!(tokenize(r#""a\"b""#), Some(strings(&["a\"b"])));
    assert_eq!(
        tokenize(r#""a\\b" "c\$d" "e\`f""#),
        Some(strings(&["a\\b", "c$d", "e`f"]))
    );
    assert_eq!(tokenize(r#""a\nb""#), Some(strings(&["a\\nb"])));
}

#[test]
fn an_unterminated_quote_gives_no_arguments() {
    assert_eq!(tokenize("foo \"bar"), None);
}

#[test]
fn single_file_codes_take_the_first_file_and_vanish_without_one() {
    let files = strings(&["/a", "/b"]);
    let args = strings(&["app", "%f", "--u=%u"]);
    assert_eq!(
        expand_args(&args, &context(&files)),
        ["app", "/a", "--u=/a"]
    );
    assert_eq!(expand_args(&args, &context(&[])), ["app", "--u="]);
    assert_eq!(
        expand_args(&strings(&["app", "%f"]), &context(&[])),
        ["app"]
    );
}

#[test]
fn list_codes_as_an_argument_become_one_argument_per_file() {
    let files = strings(&["/a b", "/c"]);
    assert_eq!(
        expand_args(&strings(&["app", "%F"]), &context(&files)),
        ["app", "/a b", "/c"]
    );
    assert_eq!(
        expand_args(&strings(&["app", "%U"]), &context(&[])),
        ["app"]
    );
}

#[test]
fn a_list_code_inside_an_argument_joins_the_files_with_spaces() {
    let files = strings(&["/a", "/b"]);
    assert_eq!(
        expand_args(&strings(&["--files=%F"]), &context(&files)),
        ["--files=/a /b"]
    );
}

#[test]
fn the_icon_code_becomes_an_icon_switch_and_vanishes_without_an_icon() {
    let with = context(&[]);
    assert_eq!(
        expand_args(&strings(&["app", "%i"]), &with),
        ["app", "--icon", "icon"]
    );
    let without = ExpandContext {
        icon: None,
        ..context(&[])
    };
    assert_eq!(expand_args(&strings(&["app", "%i"]), &without), ["app"]);
}

#[test]
fn the_name_and_path_codes_expand_in_place() {
    let args = strings(&["--title=%c", "%k"]);
    assert_eq!(
        expand_args(&args, &context(&[])),
        ["--title=Name", "/apps/a.desktop"]
    );
}

#[test]
fn a_doubled_percent_is_a_percent_and_unknown_codes_are_dropped() {
    let args = strings(&["100%%", "%d", "x%Dy", "%"]);
    assert_eq!(expand_args(&args, &context(&[])), ["100%", "xy"]);
}

#[test]
fn stripping_field_codes_gives_the_plain_launch() {
    let ctx = context(&[]);
    assert_eq!(
        strip_field_codes("app --flag %U", &ctx),
        Some(strings(&["app", "--flag"]))
    );
}

#[test]
fn stripping_field_codes_fails_for_an_empty_or_broken_exec() {
    let ctx = context(&[]);
    assert_eq!(strip_field_codes("", &ctx), None);
    assert_eq!(strip_field_codes("\"app", &ctx), None);
    assert_eq!(strip_field_codes("%U", &ctx), None);
}

#[test]
fn a_command_is_its_program_first_and_its_arguments() {
    let spec = CommandSpec::new("app", &["-a", "b"]);
    assert_eq!(spec.argv(), ["app", "-a", "b"]);
    let from = CommandSpec::from_argv(strings(&["x", "y"])).unwrap();
    assert_eq!(
        (from.program.as_str(), from.args.as_slice()),
        ("x", &strings(&["y"])[..])
    );
    assert_eq!(CommandSpec::from_argv(Vec::new()), None);
}

#[test]
fn the_activation_token_sets_both_variables_and_replaces_an_earlier_token() {
    let spec = CommandSpec::new("app", &[])
        .with_activation_token("old")
        .with_activation_token("new");
    let value = |name: &str| {
        spec.env
            .iter()
            .filter(|(k, _)| k == name)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        value(ACTIVATION_TOKEN_ENV),
        [&(ACTIVATION_TOKEN_ENV.to_string(), "new".to_string())]
    );
    assert_eq!(
        value(STARTUP_ID_ENV),
        [&(STARTUP_ID_ENV.to_string(), "new".to_string())]
    );
}

#[test]
fn a_terminal_wraps_the_command_after_its_own_prefix() {
    let mut spec = CommandSpec::new("app", &["x"]);
    spec.terminal = true;
    let wrapped = spec.in_terminal(&strings(&["xterm", "-e"]));
    assert_eq!(wrapped.argv(), ["xterm", "-e", "app", "x"]);
    assert!(!wrapped.terminal);
}

#[test]
fn an_empty_terminal_prefix_leaves_the_command_alone() {
    let spec = CommandSpec::new("app", &["x"]).in_terminal(&[]);
    assert_eq!(spec.argv(), ["app", "x"]);
}

#[test]
fn a_command_builds_a_process_command_with_its_folder_and_variables() {
    let mut spec = CommandSpec::new("app", &["x"]).with_activation_token("t");
    spec.working_dir = Some("/tmp".into());
    let command = spec.to_command();
    assert_eq!(command.get_program(), "app");
    assert_eq!(command.get_args().collect::<Vec<_>>(), ["x"]);
    assert_eq!(
        command.get_current_dir(),
        Some(std::path::Path::new("/tmp"))
    );
    let token = command
        .get_envs()
        .find(|(k, _)| *k == ACTIVATION_TOKEN_ENV)
        .and_then(|(_, v)| v);
    assert_eq!(token, Some(std::ffi::OsStr::new("t")));
}

#[test]
fn an_entry_launches_without_files_and_keeps_its_folder_id_and_terminal_flag() {
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
    assert_eq!(
        spec.working_dir.as_deref(),
        Some(std::path::Path::new("/work"))
    );
    assert_eq!(spec.desktop_id.as_deref(), Some("a.desktop"));
    assert!(spec.terminal);
    assert!(spec.env.is_empty());
}

#[test]
fn an_entry_launches_with_files_and_its_icon_and_path() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = entry(
        "a.desktop",
        &desktop("Tool", "tool-for-tests-only %i %k %F", "Icon=ic"),
    );
    e.source = Some(write(dir.path(), "a.desktop", ""));
    let spec = command_for_entry(&e, &strings(&["/x", "/y"])).unwrap();
    let source = e.source.unwrap().display().to_string();
    assert_eq!(
        spec.argv(),
        ["tool-for-tests-only", "--icon", "ic", &source, "/x", "/y"]
    );
}

#[test]
fn an_entry_with_an_unusable_exec_has_no_command() {
    let e = entry("a", &desktop("A", "\"unbalanced", ""));
    assert!(command_for_entry(&e, &[]).is_none());
    let only_codes = entry("a", &desktop("A", "%U", ""));
    assert!(command_for_entry(&only_codes, &[]).is_none());
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
    assert_eq!(
        plain.working_dir.as_deref(),
        Some(std::path::Path::new("/w"))
    );
    assert_eq!(plain.desktop_id.as_deref(), Some("a.desktop"));
}
