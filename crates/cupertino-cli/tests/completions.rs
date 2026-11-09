//! The completion scripts: generated from the registry alone, and completing in the real shells. A shell that is not
//! installed skips its part, unless `CUPERTINO_SHELLS_REQUIRED=1` (CI) makes that a failure.

mod support;

use std::path::Path;
use std::process::Command as Process;

use cupertino_cli::completions::{self, SHELLS};
use cupertino_cli::{
    Arg, Args, Command, Context, Failure, Group, Reply, Run, Runner, Stability, BUILTINS,
    GLOBAL_OPTIONS, REGISTRY, RESERVED,
};
use support::{args, envelope};

fn nothing(_: &Args, _: &Context) -> Result<Reply, Failure> {
    Ok(Reply::new(&serde_json::json!({}), ""))
}

/// A registry with a command no real one has, to show the scripts follow the registry.
const PROBE: &[Group] = &[Group {
    name: "probe",
    summary: "A test group",
    description: "A test group.",
    commands: &[&[Command {
        path: &["probe", "widget"],
        summary: "A test command",
        description: "A test command.",
        examples: &[("cupertino probe widget", "Probe.")],
        stability: Stability::Preview,
        args: &[
            Arg::option("colour", "a colour").values(&["red", "green", "blue"]),
            Arg::flag("loud", "loudly"),
            Arg::positional("thing", "a thing").values(&["gear", "spring"]),
        ],
        run: Run::Once(nothing),
    }]],
}];

fn script(shell: &str, registry: &[Group]) -> String {
    completions::script(shell, registry).unwrap()
}

#[test]
fn every_command_path_option_and_value_is_in_every_script() {
    for shell in SHELLS {
        let text = script(shell, REGISTRY);
        for builtin in BUILTINS {
            assert!(
                text.contains(&format!("'{}'", builtin.name)),
                "{shell}: {}",
                builtin.name
            );
        }
        for arg in GLOBAL_OPTIONS {
            assert!(
                text.contains(&format!(" {} ", arg.name)),
                "{shell}: global {}",
                arg.name
            );
        }
        for command in REGISTRY.iter().flat_map(Group::commands) {
            assert!(
                text.contains(&format!("'{}'", command.name())),
                "{shell}: {}",
                command.name()
            );
            for arg in command.args {
                assert!(
                    text.contains(&format!(" {} ", arg.name)),
                    "{shell}: {} {}",
                    command.name(),
                    arg.name
                );
                for value in arg.values {
                    assert!(text.contains(value), "{shell}: {} {value}", arg.name);
                }
            }
        }
        assert!(!text.contains("probe"), "{shell}");
        let probe = script(shell, PROBE);
        for word in [
            "'probe widget'",
            "colour",
            "red,green,blue",
            "loud",
            "gear,spring",
        ] {
            assert!(probe.contains(word), "{shell}: {word}");
        }
        assert!(
            !probe.contains("'desktop start'"),
            "{shell}: the tables are the registry's alone"
        );
    }
    assert_eq!(completions::script("tcsh", REGISTRY), None);
}

#[test]
fn the_command_prints_the_script_of_its_registry() {
    let dir = tempfile::tempdir().unwrap();
    let run = |registry: &'static [Group], words: &[&str]| {
        Runner::new(registry)
            .context(Context::isolated(dir.path()))
            .run(&args(words), "r1")
    };
    for shell in SHELLS {
        let human = run(REGISTRY, &["completions", shell]);
        assert_eq!((human.code, human.stderr.as_str()), (0, ""));
        assert_eq!(human.stdout, script(shell, REGISTRY));
        let json = run(REGISTRY, &["--format", "json", "completions", shell]);
        let v = envelope(&json);
        assert_eq!(v["data"]["shell"], *shell);
        assert_eq!(v["data"]["script"], script(shell, REGISTRY));
    }
    let wrong = run(REGISTRY, &["--format", "json", "completions", "tcsh"]);
    assert_eq!(wrong.code, 2);
    assert_eq!(envelope(&wrong)["error"]["code"], "invalid_value");
}

/// Whether `shell` can run; a missing one fails when the shells are required.
fn have(shell: &str) -> bool {
    let found = Process::new(shell)
        .arg("-c")
        .arg("exit 0")
        .env_clear()
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        // nothing that runs here may keep diagnostic events: no store or journal socket can be made at these paths
        .env("XDG_STATE_HOME", "/nonexistent")
        .env("CUPERTINO_DIAG_JOURNAL_SOCKET", "/nonexistent")
        .output()
        .is_ok_and(|o| o.status.success());
    if !found {
        assert_ne!(
            std::env::var("CUPERTINO_SHELLS_REQUIRED").as_deref(),
            Ok("1"),
            "{shell} is required"
        );
        eprintln!("skipped: {shell} is not installed");
    }
    found
}

/// Runs `code` in `shell` without its configuration, with a temporary HOME, in `dir`.
fn shell(shell: &str, dir: &Path, code: &str) -> String {
    let mut command = Process::new(shell);
    match shell {
        "bash" => command.args(["--norc", "--noprofile", "-c", code]),
        "zsh" => command.args(["-f", "-c", code]),
        _ => command.args(["--no-config", "-c", code]),
    };
    let out = command
        .current_dir(dir)
        .env_clear()
        .env("HOME", dir)
        .env("PATH", "/usr/bin:/bin")
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env("CUPERTINO_DIAG_JOURNAL_SOCKET", dir.join("journal"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{shell}: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// The words bash hands a completion function for `line`, the cursor at its end. With `split_at_equals` they are what
/// bash makes of them with its default `COMP_WORDBREAKS`, which holds `=`: `--area=g` is the three words `--area`, `=`
/// and `g`, and `--area=` the two words `--area` and `=`, the cursor on the `=`.
fn bash_words(line: &str, split_at_equals: bool) -> Vec<String> {
    let mut words = Vec::new();
    for word in line.split_whitespace() {
        match word.split_once('=') {
            Some((name, value)) if split_at_equals && name.starts_with("--") => {
                words.extend([name.to_owned(), "=".to_owned()]);
                if !value.is_empty() {
                    words.push(value.to_owned());
                }
            }
            _ => words.push(word.to_owned()),
        }
    }
    if line.ends_with(' ') {
        words.push(String::new());
    }
    words
}

/// What the bash script offers for `line`, the cursor at its end, split as bash splits it by default.
fn bash_offers(dir: &Path, registry: &[Group], line: &str) -> Vec<String> {
    bash_offers_split(dir, registry, line, true)
}

fn bash_offers_split(
    dir: &Path,
    registry: &[Group],
    line: &str,
    split_at_equals: bool,
) -> Vec<String> {
    std::fs::write(dir.join("cupertino.bash"), script("bash", registry)).unwrap();
    let words: Vec<String> = bash_words(line, split_at_equals)
        .iter()
        .map(|w| sh(w))
        .collect();
    let code = format!(
        "source ./cupertino.bash\n\
         COMP_WORDS=({})\n\
         COMP_CWORD=$((${{#COMP_WORDS[@]}} - 1))\n\
         _cupertino\n\
         printf '%s\\n' \"${{COMPREPLY[@]}}\"",
        words.join(" ")
    );
    let mut offers: Vec<String> = shell("bash", dir, &code)
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect();
    offers.sort();
    offers
}

fn sh(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

#[test]
fn bash_completes_commands_options_and_values() {
    if !have("bash") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert_eq!(bash_offers(d, REGISTRY, "cupertino dia"), ["diagnose"]);
    assert_eq!(bash_offers(d, REGISTRY, "cupertino de"), ["desktop", "dev"]);
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino --format json desk"),
        ["desktop"]
    );
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino --format "),
        ["human", "json", "jsonl"]
    );
    assert_eq!(bash_offers(d, REGISTRY, "cupertino desktop "), ["start"]);
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino diagnose --area g"),
        ["gpu"]
    );
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino desktop start --backend "),
        ["headless", "nested", "tty"]
    );
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino diagnose --"),
        ["--area", "--format", "--help", "--strict", "--version"]
    );
    // a single option is offered once, a repeatable one again
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino diagnose --strict --"),
        ["--area", "--format", "--help", "--version"]
    );
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino diagnose --area gpu --"),
        ["--area", "--format", "--help", "--strict", "--version"]
    );
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino completions "),
        ["bash", "fish", "zsh"]
    );
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino help dev "),
        ["layers", "manual"]
    );
    assert_eq!(bash_offers(d, REGISTRY, "cupertino he"), ["help"]);

    std::fs::create_dir(d.join("pages")).unwrap();
    std::fs::write(d.join("pages.txt"), "").unwrap();
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino dev manual pa"),
        ["pages"],
        "directories alone"
    );

    assert_eq!(bash_offers(d, PROBE, "cupertino pr"), ["probe"]);
    assert_eq!(
        bash_offers(d, PROBE, "cupertino probe widget --colour "),
        ["blue", "green", "red"]
    );
    assert_eq!(
        bash_offers(d, PROBE, "cupertino probe widget --loud "),
        ["gear", "spring"]
    );
    assert_eq!(
        bash_offers(d, PROBE, "cupertino probe widget gear "),
        Vec::<String>::new()
    );
}

#[test]
fn bash_registers_the_function_for_the_command() {
    if !have("bash") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("c.bash"), script("bash", REGISTRY)).unwrap();
    assert_eq!(
        shell("bash", dir.path(), "source ./c.bash; complete -p cupertino"),
        "complete -F _cupertino cupertino\n"
    );
}

#[test]
fn zsh_reads_the_script_and_registers_it() {
    if !have("zsh") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("_cupertino"), script("zsh", REGISTRY)).unwrap();
    shell("zsh", dir.path(), "zsh -n ./_cupertino");
    let registered = shell(
        "zsh",
        dir.path(),
        "autoload -Uz compinit && compinit -u -d ./zcompdump && source ./_cupertino && print -r -- $_comps[cupertino]",
    );
    assert_eq!(registered, "_cupertino\n");
    // from fpath, as a package installs it; compinit skips a `.` entry, so the directory is absolute
    let autoloaded = shell(
        "zsh",
        dir.path(),
        "fpath=($PWD $fpath) && autoload -Uz compinit && compinit -u -d ./zcompdump2 && print -r -- $_comps[cupertino]",
    );
    assert_eq!(autoloaded, "_cupertino\n");
}

/// What the zsh script offers for `line`, the cursor at its end. Outside a completion widget `words`, `CURRENT`, `PREFIX`
/// and `IPREFIX` are plain parameters, and `compadd` and `compset` functions of the test's: `compadd` keeps the
/// candidates `PREFIX` begins and prints them after `IPREFIX`, as zsh inserts them, `compset -P 1 '*='` moves what goes
/// up to the first `=` from `PREFIX` to `IPREFIX`, and `_files` stands for the file completion.
fn zsh_offers(dir: &Path, registry: &[Group], line: &str) -> Vec<String> {
    std::fs::write(dir.join("_cupertino"), script("zsh", registry)).unwrap();
    let mut words: Vec<&str> = line.split_whitespace().collect();
    if line.ends_with(' ') {
        words.push("");
    }
    let words: Vec<String> = words.iter().map(|w| sh(w)).collect();
    let code = format!(
        "compdef() {{ :; }}\n\
         compadd() {{ local -a xs; xs=(${{(M)${{(P)2}}:#${{PREFIX}}*}}); (( $#xs )) && print -rl -- $IPREFIX${{^xs}}; return 0; }}\n\
         compset() {{ [[ $1 == -P && $2 == 1 && $3 == '*=' && $PREFIX == *=* ]] && IPREFIX+=${{PREFIX%%=*}}= && PREFIX=${{PREFIX#*=}}; }}\n\
         _files() {{ print -r -- \"$IPREFIX(files $*)\"; }}\n\
         source ./_cupertino\n\
         words=({}) CURRENT={}\n\
         PREFIX=$words[CURRENT] IPREFIX=\n\
         _cupertino",
        words.join(" "),
        words.len()
    );
    let mut offers: Vec<String> = shell("zsh", dir, &code)
        .lines()
        .map(str::to_owned)
        .collect();
    offers.sort();
    offers
}

#[test]
fn zsh_completes_commands_options_and_values() {
    if !have("zsh") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert_eq!(zsh_offers(d, REGISTRY, "cupertino dia"), ["diagnose"]);
    assert_eq!(zsh_offers(d, REGISTRY, "cupertino de"), ["desktop", "dev"]);
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino --format json desk"),
        ["desktop"]
    );
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino --format "),
        ["human", "json", "jsonl"]
    );
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino diagnose --area g"),
        ["gpu"]
    );
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino desktop start --backend "),
        ["headless", "nested", "tty"]
    );
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino diagnose --strict --"),
        ["--area", "--format", "--help", "--version"]
    );
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino diagnose --area gpu --"),
        ["--area", "--format", "--help", "--strict", "--version"]
    );
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino help dev "),
        ["layers", "manual"]
    );
    assert_eq!(zsh_offers(d, REGISTRY, "cupertino he"), ["help"]);
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino dev manual "),
        ["(files -/)"],
        "directories alone"
    );
    assert_eq!(
        zsh_offers(d, PROBE, "cupertino probe widget --colour "),
        ["blue", "green", "red"]
    );
    assert_eq!(
        zsh_offers(d, PROBE, "cupertino probe widget "),
        ["gear", "spring"]
    );
}

/// What fish offers for `line` with the script loaded.
fn fish_offers(dir: &Path, registry: &[Group], line: &str) -> Vec<String> {
    std::fs::write(dir.join("cupertino.fish"), script("fish", registry)).unwrap();
    let code = format!("source ./cupertino.fish; complete -C {}", sh(line));
    let mut offers: Vec<String> = shell("fish", dir, &code)
        .lines()
        .map(|l| l.split('\t').next().unwrap().to_owned())
        .collect();
    offers.sort();
    offers
}

#[test]
fn fish_completes_commands_options_and_values() {
    if !have("fish") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert_eq!(fish_offers(d, REGISTRY, "cupertino dia"), ["diagnose"]);
    assert_eq!(
        fish_offers(d, REGISTRY, "cupertino diagnose --area g"),
        ["gpu"]
    );
    assert_eq!(
        fish_offers(d, REGISTRY, "cupertino desktop start --backend "),
        ["headless", "nested", "tty"]
    );
    assert_eq!(
        fish_offers(d, REGISTRY, "cupertino help dev "),
        ["layers", "manual"]
    );
    assert_eq!(
        fish_offers(d, PROBE, "cupertino probe widget --colour "),
        ["blue", "green", "red"]
    );
}

#[test]
fn no_engine_names_a_global_option() {
    for shell in SHELLS {
        let text = script(shell, REGISTRY);
        for arg in GLOBAL_OPTIONS {
            assert!(
                !text.contains(&format!("--{}", arg.name)),
                "{shell}: the global options come from the table alone, not `--{}`",
                arg.name
            );
        }
    }
}

#[test]
fn bash_completes_the_value_of_a_name_equals_option() {
    if !have("bash") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::create_dir(d.join("pages")).unwrap();
    std::fs::write(d.join("pages.txt"), "").unwrap();
    // bash splits the word at the `=` and replaces what follows it
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino diagnose --area=g"),
        ["gpu"]
    );
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino desktop start --backend="),
        ["headless", "nested", "tty"]
    );
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino --format=j"),
        ["json", "jsonl"]
    );
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino dev layers --root=pa"),
        ["pages"],
        "directories alone"
    );
    assert_eq!(
        bash_offers(d, PROBE, "cupertino probe widget --colour=r"),
        ["red"]
    );
    // a global option given with its value is not a command word
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino --format=json dia"),
        ["diagnose"]
    );
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino --help dia"),
        ["diagnose"]
    );
    // nor a positional argument, and a single option given so is not offered again
    assert_eq!(
        bash_offers(d, PROBE, "cupertino probe widget --colour=red "),
        ["gear", "spring"]
    );
    assert_eq!(
        bash_offers(
            d,
            REGISTRY,
            "cupertino desktop start --backend=headless --s"
        ),
        ["--scale", "--size", "--spawn"]
    );
    assert_eq!(
        bash_offers(
            d,
            REGISTRY,
            "cupertino desktop start --backend=headless --b"
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino diagnose --area=gpu --a"),
        ["--area"],
        "a repeatable one is"
    );
    // the reserved groups answer that they are not in this release, so they are not offered
    let first = bash_offers(d, REGISTRY, "cupertino ");
    assert!(first.contains(&"diagnose".to_owned()), "{first:?}");
    for reserved in RESERVED {
        assert!(!first.contains(&reserved.name.to_owned()), "{first:?}");
        assert_eq!(
            bash_offers(d, REGISTRY, &format!("cupertino {} ", reserved.name)),
            Vec::<String>::new()
        );
    }
    // without `=` in COMP_WORDBREAKS the word is whole, and so is what replaces it
    assert_eq!(
        bash_offers_split(d, REGISTRY, "cupertino diagnose --area=g", false),
        ["--area=gpu"]
    );
    assert_eq!(
        bash_offers_split(d, REGISTRY, "cupertino dev layers --root=pa", false),
        ["--root=pages"]
    );
    assert_eq!(
        bash_offers_split(d, REGISTRY, "cupertino --format=json dia", false),
        ["diagnose"]
    );
}

#[test]
fn zsh_completes_the_value_of_a_name_equals_option() {
    if !have("zsh") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino diagnose --area=g"),
        ["--area=gpu"]
    );
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino desktop start --backend="),
        ["--backend=headless", "--backend=nested", "--backend=tty"]
    );
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino --format=j"),
        ["--format=json", "--format=jsonl"]
    );
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino dev layers --root=pa"),
        ["--root=(files -/)"]
    );
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino --format=json dia"),
        ["diagnose"]
    );
    assert_eq!(
        zsh_offers(d, PROBE, "cupertino probe widget --colour=red "),
        ["gear", "spring"]
    );
    assert_eq!(
        zsh_offers(
            d,
            REGISTRY,
            "cupertino desktop start --backend=headless --s"
        ),
        ["--scale", "--size", "--spawn"]
    );
    assert_eq!(
        zsh_offers(
            d,
            REGISTRY,
            "cupertino desktop start --backend=headless --b"
        ),
        Vec::<String>::new()
    );
}

#[test]
fn fish_completes_the_value_of_a_name_equals_option() {
    if !have("fish") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::create_dir(d.join("pages")).unwrap();
    std::fs::write(d.join("pages.txt"), "").unwrap();
    assert_eq!(
        fish_offers(d, REGISTRY, "cupertino diagnose --area=g"),
        ["--area=gpu"]
    );
    assert_eq!(
        fish_offers(d, REGISTRY, "cupertino desktop start --backend="),
        ["--backend=headless", "--backend=nested", "--backend=tty"]
    );
    assert_eq!(
        fish_offers(d, REGISTRY, "cupertino --format=j"),
        ["--format=json", "--format=jsonl"]
    );
    assert_eq!(
        fish_offers(d, REGISTRY, "cupertino dev layers --root=pa"),
        ["--root=pages/"],
        "directories alone"
    );
    assert_eq!(
        fish_offers(d, REGISTRY, "cupertino --format=json dia"),
        ["diagnose"]
    );
    assert_eq!(
        fish_offers(d, PROBE, "cupertino probe widget --colour=red "),
        ["gear", "spring"]
    );
    assert_eq!(
        fish_offers(
            d,
            REGISTRY,
            "cupertino desktop start --backend=headless --s"
        ),
        ["--scale", "--size", "--spawn"]
    );
    assert_eq!(
        fish_offers(
            d,
            REGISTRY,
            "cupertino desktop start --backend=headless --b"
        ),
        Vec::<String>::new()
    );
    assert!(
        fish_offers(
            d,
            REGISTRY,
            "cupertino desktop start --backend headless --spawn=ba"
        )
        .iter()
        .any(|c| c == "--spawn=bash"),
        "program names after --spawn="
    );
}
