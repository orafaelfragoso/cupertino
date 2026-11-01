use cupertino_cli::{run, Output};
use serde_json::{json, Value};

fn cupertino(args: &[&str]) -> Output {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    run(&args, "r1")
}

fn json(out: &Output) -> Value {
    assert_eq!(out.stderr, "");
    serde_json::from_str(&out.stdout).unwrap()
}

fn empty_pack() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("manifest.json"),
        r#"{"version": 2, "name": "empty", "surplus": true}"#,
    )
    .unwrap();
    d
}

#[test]
fn the_version_in_json_is_one_envelope_line_in_field_order() {
    let out = cupertino(&["--format", "json", "--version"]);
    assert_eq!(
        out,
        Output {
            code: 0,
            stdout: format!(
                "{{\"schema\":\"cupertino.cli.v1\",\"command\":\"version\",\"request_id\":\"r1\",\"ok\":true,\
                 \"data\":{{\"cli\":\"{}\"}}}}\n",
                env!("CARGO_PKG_VERSION")
            ),
            stderr: String::new(),
        }
    );
}

#[test]
fn an_unknown_command_is_invalid_usage_with_exit_code_2() {
    let out = cupertino(&["frobnicate", "now", "--format=json"]);
    assert_eq!(out.code, 2);
    assert_eq!(
        json(&out),
        json!({
            "schema": "cupertino.cli.v1",
            "command": "frobnicate now",
            "request_id": "r1",
            "ok": false,
            "error": {
                "kind": "invalid_usage",
                "code": "unknown_command",
                "message": "unknown command `frobnicate now`; see `cupertino help`"
            }
        })
    );
}

#[test]
fn a_human_failure_goes_to_stderr_only() {
    let out = cupertino(&["appearance", "assets", "check"]);
    assert_eq!(
        out,
        Output {
            code: 2,
            stdout: String::new(),
            stderr: "cupertino: `appearance assets check` needs <dir>\n".into(),
        }
    );
}

#[test]
fn a_group_without_a_subcommand_names_the_help_to_read() {
    let out = cupertino(&["appearance"]);
    assert_eq!(out.code, 2);
    assert_eq!(
        out.stderr,
        "cupertino: `appearance` needs a subcommand; see `cupertino help appearance`\n"
    );
}

#[test]
fn an_extra_argument_and_an_unknown_option_are_invalid_usage() {
    let extra = cupertino(&["appearance", "assets", "check", "a", "b"]);
    assert_eq!(
        (extra.code, extra.stderr.as_str()),
        (
            2,
            "cupertino: `appearance assets check` takes no argument `b`\n"
        )
    );
    let option = cupertino(&["appearance", "assets", "check", "a", "--deep"]);
    assert_eq!(
        (option.code, option.stderr.as_str()),
        (
            2,
            "cupertino: `appearance assets check` has no option --deep\n"
        )
    );
}

#[test]
fn a_format_other_than_human_or_json_is_refused_before_the_command_runs() {
    let out = cupertino(&["--format", "jsonl", "frobnicate"]);
    assert_eq!(
        out,
        Output {
            code: 2,
            stdout: String::new(),
            stderr: "cupertino: --format takes human or json, not `jsonl`\n".into(),
        }
    );
}

#[test]
fn the_json_help_lists_the_command_tree_under_the_topic() {
    let tree = json!([{
        "path": "appearance assets check",
        "summary": "Reads the asset pack in <dir> as the desktop processes do and reports its problems and what \
                    provides each logical name, the pack or the built-in fallback",
        "args": [{"name": "dir", "help": "the pack directory, with its manifest.json", "kind": "positional"}]
    }]);
    for args in [
        &["--format", "json", "help"][..],
        &["--format", "json", "help", "appearance", "assets"],
        &["--format", "json", "appearance", "--help"],
    ] {
        let out = cupertino(args);
        assert_eq!((out.code, &json(&out)["data"]), (0, &tree), "{args:?}");
    }
    let unknown = cupertino(&["help", "packages"]);
    assert_eq!(
        (unknown.code, unknown.stderr.as_str()),
        (
            2,
            "cupertino: unknown command `packages`; see `cupertino help`\n"
        )
    );
}

#[test]
fn no_arguments_print_the_help() {
    let out = cupertino(&[]);
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout, cupertino(&["help"]).stdout);
    assert!(
        out.stdout.starts_with(
            "usage: cupertino [--format human|json] <command> [<arguments>]\n       \
             cupertino help [<command>]\n       cupertino --version\n\ncommands:\n  \
             appearance assets check <dir>\n"
        ),
        "{}",
        out.stdout
    );
}

#[test]
fn the_asset_check_reports_the_pack_in_json() {
    let pack = empty_pack();
    let dir = pack.path().to_str().unwrap();
    let out = cupertino(&["appearance", "assets", "check", dir, "--format", "json"]);
    assert_eq!(out.code, 0);
    let v = json(&out);
    assert_eq!(v["command"], "appearance assets check");
    let data = &v["data"];
    assert_eq!(
        data["packs"],
        json!([{"root": dir, "name": "empty", "credits": null}])
    );
    assert_eq!(
        data["problems"],
        json!(["manifest: unknown key \"surplus\" ignored"])
    );
    assert_eq!(
        data["fonts"][0],
        json!({"name": "ui", "provider": "built-in (Inter Variable)"})
    );
    assert_eq!(data["cursor_theme"], Value::Null);
    assert_eq!(
        data["fallbacks"],
        json!([
            "font ui",
            "cursor theme",
            "wallpaper light",
            "wallpaper dark"
        ])
    );
}

#[test]
fn the_asset_check_in_human_form_is_the_report_text() {
    let pack = empty_pack();
    let dir = pack.path().to_str().unwrap();
    let out = cupertino(&["appearance", "assets", "check", dir]);
    assert_eq!((out.code, out.stderr.as_str()), (0, ""));
    assert!(
        out.stdout.starts_with(&format!(
            "pack {dir} (empty)\n  credits: none\nproblems: 1\n  manifest: unknown key \"surplus\" ignored\nfonts:\n"
        )),
        "{}",
        out.stdout
    );
}

#[test]
fn a_pack_that_cannot_be_read_is_a_validation_failure_with_exit_code_5() {
    let d = tempfile::tempdir().unwrap();
    let missing = d.path().join("nothing");
    let out = cupertino(&[
        "--format",
        "json",
        "appearance",
        "assets",
        "check",
        missing.to_str().unwrap(),
    ]);
    assert_eq!(out.code, 5);
    let v = json(&out);
    assert_eq!(
        (&v["ok"], &v["error"]["kind"], &v["error"]["code"]),
        (
            &json!(false),
            &json!("validation"),
            &json!("pack_unreadable")
        )
    );
}
