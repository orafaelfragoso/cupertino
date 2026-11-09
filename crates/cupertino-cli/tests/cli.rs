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
fn a_format_other_than_human_json_or_jsonl_is_refused_before_the_command_runs() {
    let out = cupertino(&["--format", "yaml", "frobnicate"]);
    assert_eq!(
        out,
        Output {
            code: 2,
            stdout: String::new(),
            stderr: "cupertino: --format takes human, json or jsonl, not `yaml`\n".into(),
        }
    );
}

#[test]
fn the_json_help_lists_the_command_tree_under_the_topic() {
    let tree = json!([{
        "path": "appearance assets check",
        "summary": "Reads the asset pack in <dir> as the desktop processes do and reports its problems and what \
                    provides each logical name, the pack or the built-in fallback",
        "args": [{
            "name": "dir", "help": "the pack directory, with its manifest.json", "kind": "positional",
            "values": [], "repeat": false, "hint": "dir"
        }],
        "stability": "stable",
        "streams": false,
        "description": "Reads the pack in <dir> the way the desktop processes read it at start: the manifest, then \
                        every file it names. It lists the problems found inside the pack (unknown manifest keys, \
                        missing or unreadable files), which are reported but do not fail the check, and for each \
                        logical name whether the pack or the built-in fallback provides it.\n\nA pack that cannot \
                        be read at all fails with exit code 5: a missing directory or manifest (pack_unreadable), a \
                        manifest that is not valid (manifest_invalid) or a font that cannot be loaded (font_invalid). \
                        It replaces `cupertino-assets check <dir>`.",
        "examples": [
            {
                "command": "cupertino appearance assets check ~/.local/share/cupertino/assets",
                "description": "Check the pack installed for the current user."
            },
            {
                "command": "cupertino --format json appearance assets check ./my-pack",
                "description": "Report the pack in ./my-pack as one JSON envelope."
            }
        ]
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
            "usage: cupertino [--format <human|json|jsonl>] <command> [<arguments>]\n       \
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

mod parsing {
    //! The argument rules of the registry, on a test registry.

    use cupertino_cli::{
        Arg, Args, Command, Context, Failure, Group, Output, Reply, Run, Runner, Stability,
    };
    use serde_json::{json, Value};

    /// Echoes what it parsed.
    fn echo(args: &Args, cx: &Context) -> Result<Reply, Failure> {
        let data = json!({
            "shell": args.value("shell"),
            "areas": args.values("area"),
            "files": args.values("file"),
            "strict": args.flag("strict"),
            "root": cx.root(),
        });
        Ok(Reply::new(&data, ""))
    }

    const ARGS: &[Arg] = &[
        Arg::option("shell", "a shell").values(&["bash", "zsh", "fish"]),
        Arg::option("area", "an area").repeat(),
        Arg::flag("strict", "strict"),
        Arg::positional("file", "files").repeat(),
    ];

    const COMMANDS: &[Command] = &[
        Command {
            path: &["t", "echo"],
            summary: "s",
            description: "d",
            examples: &[("cupertino t echo a", "x")],
            stability: Stability::Stable,
            args: ARGS,
            run: Run::Once(echo),
        },
        Command {
            path: &["t"],
            summary: "s",
            description: "d",
            examples: &[("cupertino t", "x")],
            stability: Stability::Stable,
            args: &[Arg::flag("strict", "strict")],
            run: Run::Once(echo),
        },
        Command {
            path: &["t", "pick"],
            summary: "s",
            description: "d",
            examples: &[("cupertino t pick bash", "x")],
            stability: Stability::Stable,
            args: &[Arg::positional("shell", "a shell").values(&["bash", "zsh"])],
            run: Run::Once(echo),
        },
    ];

    const TEST: &[Group] = &[Group {
        name: "t",
        summary: "s",
        description: "d",
        commands: &[COMMANDS],
    }];

    fn run(words: &[&str]) -> Output {
        let args: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        Runner::new(TEST)
            .context(Context::isolated("/fixture"))
            .run(&args, "r1")
    }

    /// Runs `words` in JSON form; `--format` goes first, as anything after a `--` is the command's.
    fn data(words: &[&str]) -> Value {
        let words = [&["--format", "json"][..], words].concat();
        let out = run(&words);
        assert_eq!(
            (out.code, out.stderr.as_str()),
            (0, ""),
            "{words:?}: {}",
            out.stdout
        );
        serde_json::from_str::<Value>(&out.stdout).unwrap()["data"].clone()
    }

    fn refused(words: &[&str]) -> (u8, String, String) {
        let words = [&["--format", "json"][..], words].concat();
        let out = run(&words);
        let v: Value = serde_json::from_str(&out.stdout).unwrap();
        (
            out.code,
            v["error"]["code"].as_str().unwrap().to_string(),
            v["error"]["message"].as_str().unwrap().to_string(),
        )
    }

    #[test]
    fn repeatable_options_and_a_repeatable_positional_collect_every_value_in_order() {
        let d = data(&[
            "t",
            "echo",
            "a",
            "--area",
            "gpu",
            "b",
            "--area=logs",
            "--strict",
            "c",
            "--strict",
        ]);
        assert_eq!(d["areas"], json!(["gpu", "logs"]));
        assert_eq!(d["files"], json!(["a", "b", "c"]));
        assert_eq!(d["strict"], true);
        assert_eq!(d["shell"], Value::Null);
        assert_eq!(
            d["root"], "/fixture",
            "the command reads the runner's context"
        );
    }

    #[test]
    fn a_value_list_is_enforced_for_options_and_positional_arguments() {
        assert_eq!(data(&["t", "echo", "a", "--shell", "zsh"])["shell"], "zsh");
        assert_eq!(
            refused(&["t", "echo", "a", "--shell", "tcsh"]),
            (
                2,
                "invalid_value".into(),
                "--shell takes bash, zsh or fish, not `tcsh`".into()
            )
        );
        assert_eq!(
            refused(&["t", "pick", "fish"]),
            (
                2,
                "invalid_value".into(),
                "<shell> takes bash or zsh, not `fish`".into()
            )
        );
    }

    #[test]
    fn a_single_option_given_twice_and_a_flag_with_a_value_are_refused() {
        assert_eq!(
            refused(&["t", "echo", "a", "--shell", "zsh", "--shell=bash"]),
            (
                2,
                "repeated_option".into(),
                "--shell is given more than once".into()
            )
        );
        assert_eq!(
            refused(&["t", "echo", "a", "--strict=yes"]),
            (
                2,
                "unexpected_value".into(),
                "--strict takes no value".into()
            )
        );
        assert_eq!(refused(&["t", "echo", "a", "--area"]).1, "missing_value");
        assert_eq!(refused(&["t", "echo"]).1, "missing_argument");
    }

    #[test]
    fn after_a_double_dash_every_word_is_a_positional_argument() {
        let out = run(&["t", "echo", "--", "--format", "json", "--help", "-x"]);
        assert_eq!((out.code, out.stderr.as_str()), (0, ""), "{}", out.stdout);
        let d = data(&["t", "echo", "--strict", "--", "--strict", "--"]);
        assert_eq!(d["files"], json!(["--strict", "--"]));
    }

    #[test]
    fn the_longest_command_path_wins() {
        assert_eq!(data(&["t", "echo", "x"])["files"], json!(["x"]));
        let short = run(&["--format", "json", "t", "--strict"]);
        let v: Value = serde_json::from_str(&short.stdout).unwrap();
        assert_eq!((short.code, &v["command"]), (0, &json!("t")));
    }

    #[test]
    fn a_reserved_group_is_unsupported_in_this_release() {
        for words in [
            &["system", "update", "check"][..],
            &["ai"],
            &["help", "system"],
        ] {
            let out = cupertino_cli::run(
                &words
                    .iter()
                    .map(|w| w.to_string())
                    .chain(["--format".into(), "json".into()])
                    .collect::<Vec<_>>(),
                "r1",
            );
            assert_eq!(out.code, 8, "{words:?}");
            let v: Value = serde_json::from_str(&out.stdout).unwrap();
            assert_eq!(
                (&v["error"]["kind"], &v["error"]["code"]),
                (&json!("unsupported"), &json!("not_in_this_release")),
                "{words:?}"
            );
        }
        let human = cupertino_cli::run(&["system".to_string()], "r1");
        assert_eq!(
            human.stderr,
            "cupertino: `cupertino system` (system updates and recovery) is not in this release\n"
        );
    }

    #[test]
    fn an_option_before_any_command_is_an_unknown_option() {
        let out = cupertino_cli::run(&["--verbose".to_string()], "r1");
        assert_eq!(
            (out.code, out.stderr.as_str()),
            (
                2,
                "cupertino: unknown option `--verbose`; see `cupertino help`\n"
            )
        );
    }

    #[test]
    fn the_help_of_one_command_shows_its_description_and_examples() {
        let out = cupertino_cli::run(
            &[
                "help".into(),
                "appearance".into(),
                "assets".into(),
                "check".into(),
            ],
            "r1",
        );
        assert_eq!(out.code, 0);
        assert!(
            out.stdout
                .contains("\n      Reads the pack in <dir> the way"),
            "{}",
            out.stdout
        );
        assert!(
            out.stdout.contains(
                "      examples:\n        cupertino appearance assets check ~/.local/share/cupertino/assets\n"
            ),
            "{}",
            out.stdout
        );
        // a topic above the command lists it without the long text
        let group = cupertino_cli::run(&["help".into(), "appearance".into()], "r1");
        assert!(!group.stdout.contains("examples:"), "{}", group.stdout);
        // a repeatable option shows as such
        let test = run(&["help", "t", "echo"]);
        assert!(
            test.stdout.contains(
                "  t echo [--shell <bash|zsh|fish>] [--area <area>]... [--strict] <file>...\n"
            ),
            "{}",
            test.stdout
        );
    }
}
