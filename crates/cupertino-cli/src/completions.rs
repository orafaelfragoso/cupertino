//! Shell completions generated from a registry: `cupertino completions bash|zsh|fish`.
//!
//! Each script has two parts. Its tables are generated from the registry alone: every command path, and for each
//! command (and for the global options) one line per argument, `KIND NAME REPEAT HINT VALUES` (`VALUES` joined by
//! commas, `-` for none). Its engine is the same
//! for every registry: it skips the global options by their table (with the value of one that takes a value), finds the
//! longest command path the words before the cursor start with, and offers the next words of the paths, the command's
//! options (a single option until it is given, as `--name` or `--name=value`; a repeatable one again), the values of the
//! option before the cursor or of the option in a `--name=` word (bash splits that word at the `=` and the engine joins
//! it again), or what the next positional argument takes: its values, files, directories or program names by its hint.
//! `help` completes command words. Nothing in a script names a command or an option except its tables.

use crate::{
    Arg, ArgKind, Args, Command, Context, Failure, Group, Reply, Run, Stability, ValueHint,
    BUILTINS, GLOBAL_OPTIONS,
};
use serde::Serialize;
use std::fmt::Write as _;

/// The shells there are completions for.
pub const SHELLS: &[&str] = &["bash", "zsh", "fish"];

pub(crate) const GROUP: Group = Group {
    name: "completions",
    summary: "Shell completions of the command",
    description: "Prints the completion script of a shell, generated from the same registry as the help, so it \
                  always knows every command, option and value of this release.",
    commands: &[COMMANDS],
};

const COMMANDS: &[Command] = &[Command {
    path: &["completions"],
    summary: "Prints the completion script of <shell> for the cupertino command",
    description: "Prints the script that completes cupertino's command words, options and option values in bash, \
                  zsh or fish. The packages install all three where each shell finds them; a script printed here \
                  serves a command installed elsewhere. Values come from the command's own lists (areas, backends, \
                  shells), and paths and program names from the shell.",
    examples: &[
        (
            "cupertino completions bash > ~/.local/share/bash-completion/completions/cupertino",
            "Install the bash completions for the current user.",
        ),
        (
            "cupertino completions fish > ~/.config/fish/completions/cupertino.fish",
            "Install the fish completions for the current user.",
        ),
        (
            "cupertino completions zsh > ~/.zfunc/_cupertino",
            "Write the zsh completions into a directory of fpath.",
        ),
    ],
    stability: Stability::Stable,
    args: &[Arg::positional("shell", "the shell to complete in").values(SHELLS)],
    run: Run::Once(completions),
}];

#[derive(Serialize)]
struct Script<'a> {
    shell: &'a str,
    script: String,
}

fn completions(args: &Args, cx: &Context) -> Result<Reply, Failure> {
    let shell = args
        .value("shell")
        .expect("<shell> is a positional argument");
    let script =
        script(shell, cx.registry()).expect("<shell> takes the shells there are scripts for");
    Ok(Reply::new(
        &Script {
            shell,
            script: script.clone(),
        },
        script,
    ))
}

/// The completion script of `shell` (`bash`, `zsh` or `fish`) for `registry`.
pub fn script(shell: &str, registry: &[Group]) -> Option<String> {
    match shell {
        "bash" => Some(bash(registry)),
        "zsh" => Some(zsh(registry)),
        "fish" => Some(fish(registry)),
        _ => None,
    }
}

/// The command paths of `registry`, longest first, then in registry order.
fn paths(registry: &[Group]) -> Vec<String> {
    let mut commands: Vec<&Command> = registry.iter().flat_map(Group::commands).collect();
    commands.sort_by_key(|c| std::cmp::Reverse(c.path.len()));
    commands.iter().map(|c| c.name()).collect()
}

/// `KIND NAME REPEAT HINT VALUES` of each argument: `VALUES` joined by commas, `-` for none.
fn arg_lines(args: &[Arg]) -> Vec<String> {
    args.iter()
        .map(|a| {
            let kind = match a.kind {
                ArgKind::Positional => "positional",
                ArgKind::Option => "option",
                ArgKind::Flag => "flag",
            };
            let hint = match a.hint {
                ValueHint::None => "none",
                ValueHint::File => "file",
                ValueHint::Dir => "dir",
                ValueHint::Command => "command",
                ValueHint::Shell => "shell",
            };
            let values = if a.values.is_empty() {
                "-".to_owned()
            } else {
                a.values.join(",")
            };
            format!("{kind} {} {} {hint} {values}", a.name, u8::from(a.repeat))
        })
        .collect()
}

/// `(path, argument lines)` of the global options (path empty), then of every command.
fn arg_tables(registry: &[Group]) -> Vec<(String, Vec<String>)> {
    let mut tables = vec![(String::new(), arg_lines(GLOBAL_OPTIONS))];
    tables.extend(
        registry
            .iter()
            .flat_map(Group::commands)
            .map(|c| (c.name(), arg_lines(c.args))),
    );
    tables
}

/// A word in single quotes for bash and zsh.
fn sh_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// A word in single quotes for fish.
fn fish_quote(word: &str) -> String {
    format!("'{}'", word.replace('\\', r"\\").replace('\'', r"\'"))
}

fn sh_list(words: &[String]) -> String {
    words
        .iter()
        .map(|w| sh_quote(w))
        .collect::<Vec<_>>()
        .join(" ")
}

const HEADER: &str =
    "generated by `cupertino completions` from the command's registry; do not edit";

/// The tables of a bash or zsh script: `_cupertino_paths`, `_cupertino_builtins` and `_cupertino_args`.
fn sh_tables(registry: &[Group], zsh: bool) -> String {
    let mut out = String::new();
    let paths = paths(registry);
    let builtins: Vec<String> = BUILTINS.iter().map(|b| b.name.to_owned()).collect();
    let _ = writeln!(out, "# The command paths, longest first.");
    let _ = writeln!(
        out,
        "_cupertino_paths() {{\n    printf '%s\\n' {}\n}}\n",
        sh_list(&paths)
    );
    let _ = writeln!(out, "# The commands the program answers itself.");
    let _ = writeln!(
        out,
        "_cupertino_builtins() {{\n    printf '%s\\n' {}\n}}\n",
        sh_list(&builtins)
    );
    let _ = writeln!(
        out,
        "# The arguments of a command path (empty for the global options): KIND NAME REPEAT HINT VALUES"
    );
    let _ = writeln!(out, "_cupertino_args() {{\n    case $1 in");
    for (path, lines) in arg_tables(registry) {
        if lines.is_empty() {
            continue;
        }
        let pattern = if zsh {
            format!("({})", sh_quote(&path))
        } else {
            sh_quote(&path)
        };
        let _ = writeln!(out, "    {pattern}) printf '%s\\n' {} ;;", sh_list(&lines));
    }
    let _ = writeln!(out, "    esac\n}}");
    out
}

/// The bash script: `complete -F _cupertino cupertino`.
pub fn bash(registry: &[Group]) -> String {
    format!(
        "# bash completion for cupertino, {HEADER}.\n\n{}\n{BASH_ENGINE}",
        sh_tables(registry, false)
    )
}

/// The zsh script, for a directory of `fpath` as `_cupertino` or to `source` after `compinit`.
pub fn zsh(registry: &[Group]) -> String {
    format!(
        "#compdef cupertino\n# zsh completion for cupertino, {HEADER}.\n\n{}\n{ZSH_ENGINE}",
        sh_tables(registry, true)
    )
}

/// The fish script, for `vendor_completions.d` or `~/.config/fish/completions`.
pub fn fish(registry: &[Group]) -> String {
    let mut out = format!("# fish completion for cupertino, {HEADER}.\n\n");
    let paths: Vec<String> = paths(registry).iter().map(|p| fish_quote(p)).collect();
    let builtins: Vec<String> = BUILTINS.iter().map(|b| fish_quote(b.name)).collect();
    let _ = writeln!(out, "# The command paths, longest first.");
    let _ = writeln!(
        out,
        "function __cupertino_paths\n    printf '%s\\n' {}\nend\n",
        paths.join(" ")
    );
    let _ = writeln!(out, "# The commands the program answers itself.");
    let _ = writeln!(
        out,
        "function __cupertino_builtins\n    printf '%s\\n' {}\nend\n",
        builtins.join(" ")
    );
    let _ = writeln!(
        out,
        "# The arguments of a command path (empty for the global options): KIND NAME REPEAT HINT VALUES"
    );
    let _ = writeln!(out, "function __cupertino_args\n    switch \"$argv[1]\"");
    for (path, lines) in arg_tables(registry) {
        if lines.is_empty() {
            continue;
        }
        let lines: Vec<String> = lines.iter().map(|l| fish_quote(l)).collect();
        let _ = writeln!(
            out,
            "        case {}\n            printf '%s\\n' {}",
            fish_quote(&path),
            lines.join(" ")
        );
    }
    let _ = writeln!(out, "    end\nend\n");
    out.push_str(FISH_ENGINE);
    out
}

const BASH_ENGINE: &str = r#"# Adds what an argument takes to COMPREPLY for the word $cur: HINT VALUES (joined by commas, - for none).
_cupertino_reply() {
    local IFS=$' \t\n'
    case $1 in
    file) compopt -o filenames 2>/dev/null; COMPREPLY+=($(compgen -f -- "$cur")) ;;
    dir) compopt -o filenames 2>/dev/null; COMPREPLY+=($(compgen -d -- "$cur")) ;;
    command | shell) COMPREPLY+=($(compgen -c -- "$cur")) ;;
    *) [[ $2 != - ]] && COMPREPLY+=($(compgen -W "${2//,/ }" -- "$cur")) ;;
    esac
}

_cupertino() {
    local IFS=$' \t\n'
    local i w cpath cmd= n=0 line kind name repeat hint values count k cur prev= taken split=0
    local -a words=() seen=() p=() topic=() candidates=() global=() spec=() rest=() opts=()
    COMPREPLY=()
    # the words up to the cursor; bash splits `--name=value` at the `=`, so the parts are joined again
    for ((i = 1; i <= COMP_CWORD; i++)); do
        w=${COMP_WORDS[i]}
        if [[ $w == = ]] && ((${#words[@]})) && [[ ${words[-1]} == --* && ${words[-1]} != *=* ]]; then
            words[-1]+==
            if ((i < COMP_CWORD)); then
                ((i++))
                words[-1]+=${COMP_WORDS[i]}
            fi
            ((i == COMP_CWORD)) && split=1
            continue
        fi
        words+=("$w")
    done
    cur=${words[-1]}
    unset 'words[-1]'
    ((${#words[@]})) && prev=${words[-1]}
    # the words before the cursor without the global options (and the value of one that takes a value)
    mapfile -t global < <(_cupertino_args '')
    for ((i = 0; i < ${#words[@]}; i++)); do
        w=${words[i]}
        taken=
        for line in "${global[@]}"; do
            read -r kind name repeat hint values <<<"$line"
            if [[ $w == "--$name" ]]; then
                taken=$kind
            elif [[ $kind == option && $w == "--$name="* ]]; then
                taken=flag
            fi
            [[ -n $taken ]] && break
        done
        case $taken in
        option) ((i++)) ;;
        flag) ;;
        *) seen+=("$w") ;;
        esac
    done
    spec=("${global[@]}")
    # the longest command path the words start with
    while IFS= read -r cpath; do
        read -ra p <<<"$cpath"
        if ((${#p[@]} > n)) && [[ " ${seen[*]:0:${#p[@]}} " == " $cpath " ]]; then
            cmd=$cpath
            n=${#p[@]}
        fi
    done < <(_cupertino_paths)
    # the next word of every path the words (after `help`) begin
    topic=("${seen[@]}")
    [[ ${seen[0]} == help ]] && topic=("${seen[@]:1}")
    while IFS= read -r cpath; do
        read -ra p <<<"$cpath"
        if ((${#p[@]} > ${#topic[@]})) && [[ " ${p[*]:0:${#topic[@]}} " == " ${topic[*]} " ]]; then
            w=${p[${#topic[@]}]}
            [[ " ${candidates[*]} " == *" $w "* ]] || candidates+=("$w")
        fi
    done < <(_cupertino_paths)
    if ((${#seen[@]} == 0)); then
        mapfile -t -O "${#candidates[@]}" candidates < <(_cupertino_builtins)
    fi
    [[ -n $cmd ]] && mapfile -t -O "${#spec[@]}" spec < <(_cupertino_args "$cmd")
    # the value of an option given as --name=value: bash replaces the part after the `=` when it split the word there,
    # else the whole word, which then keeps its --name=
    if [[ $cur == --*=* ]]; then
        for line in "${spec[@]}"; do
            read -r kind name repeat hint values <<<"$line"
            if [[ $kind == option && ${cur%%=*} == "--$name" ]]; then
                cur=${cur#*=}
                _cupertino_reply "$hint" "$values"
                ((split)) || COMPREPLY=("${COMPREPLY[@]/#/--$name=}")
                break
            fi
        done
        return 0
    fi
    # the value of the option before the cursor
    if [[ $prev == --* ]]; then
        for line in "${spec[@]}"; do
            read -r kind name repeat hint values <<<"$line"
            if [[ $kind == option && $prev == "--$name" ]]; then
                _cupertino_reply "$hint" "$values"
                return 0
            fi
        done
    fi
    if [[ ${seen[0]} == help || -z $cmd ]]; then
        if [[ $cur == -* ]]; then
            for line in "${spec[@]}"; do
                read -r kind name repeat hint values <<<"$line"
                opts+=("--$name")
            done
            COMPREPLY=($(compgen -W "${opts[*]}" -- "$cur"))
        else
            COMPREPLY=($(compgen -W "${candidates[*]}" -- "$cur"))
        fi
        return 0
    fi
    rest=("${seen[@]:n}")
    if [[ $cur == -* ]]; then
        for line in "${spec[@]}"; do
            read -r kind name repeat hint values <<<"$line"
            [[ $kind == positional ]] && continue
            # a single option is offered until it is given, as --name or --name=value
            if [[ $repeat == 1 || (" ${rest[*]} " != *" --$name "* && " ${rest[*]} " != *" --$name="*) ]]; then
                opts+=("--$name")
            fi
        done
        COMPREPLY=($(compgen -W "${opts[*]}" -- "$cur"))
        return 0
    fi
    # the positional argument the cursor is at
    count=0
    for ((i = 0; i < ${#rest[@]}; i++)); do
        w=${rest[i]}
        if [[ $w == --* ]]; then
            for line in "${spec[@]}"; do
                read -r kind name repeat hint values <<<"$line"
                [[ $kind == option && $w == "--$name" ]] && ((i++))
            done
            continue
        fi
        ((count++))
    done
    k=0
    for line in "${spec[@]}"; do
        read -r kind name repeat hint values <<<"$line"
        [[ $kind == positional ]] || continue
        if ((k == count)) || ((repeat == 1 && count > k)); then
            _cupertino_reply "$hint" "$values"
            break
        fi
        ((k++))
    done
    ((count == 0)) && COMPREPLY+=($(compgen -W "${candidates[*]}" -- "$cur"))
    return 0
}

complete -F _cupertino cupertino
"#;

const ZSH_ENGINE: &str = r#"# Offers what an argument takes: HINT VALUES (joined by commas, - for none).
_cupertino_reply() {
    local -a vals
    case $1 in
    (file) _files ;;
    (dir) _files -/ ;;
    (command|shell) _command_names -e ;;
    (*) [[ $2 != - ]] && vals=(${(s:,:)2}) && compadd -a vals ;;
    esac
}

_cupertino() {
    local cur=${words[CURRENT]} prev=${words[CURRENT-1]}
    local i w cpath cmd= n=0 line kind name repeat hint values count k taken
    local -a seen p topic candidates global spec rest opts
    typeset -U candidates
    # the words before the cursor without the global options (and the value of one that takes a value)
    global=("${(@f)$(_cupertino_args '')}")
    for (( i = 2; i < CURRENT; i++ )); do
        w=${words[i]}
        taken=
        for line in "${global[@]}"; do
            read -r kind name repeat hint values <<<"$line"
            if [[ $w == "--$name" ]]; then
                taken=$kind
            elif [[ $kind == option && $w == "--$name="* ]]; then
                taken=flag
            fi
            [[ -n $taken ]] && break
        done
        case $taken in
        (option) (( i++ )) ;;
        (flag) ;;
        (*) seen+=("$w") ;;
        esac
    done
    spec=("${global[@]}")
    # the longest command path the words start with
    for cpath in "${(@f)$(_cupertino_paths)}"; do
        p=(${=cpath})
        if (( $#p > n )) && [[ "${(j: :)seen[1,$#p]}" == "$cpath" ]]; then
            cmd=$cpath
            n=$#p
        fi
    done
    # the next word of every path the words (after `help`) begin
    topic=("${seen[@]}")
    [[ $seen[1] == help ]] && topic=("${(@)seen[2,-1]}")
    for cpath in "${(@f)$(_cupertino_paths)}"; do
        p=(${=cpath})
        if (( $#p > $#topic )) && [[ "${(j: :)p[1,$#topic]}" == "${(j: :)topic}" ]]; then
            candidates+=("$p[$#topic+1]")
        fi
    done
    (( $#seen == 0 )) && candidates+=("${(@f)$(_cupertino_builtins)}")
    [[ -n $cmd ]] && spec+=("${(@f)$(_cupertino_args "$cmd")}")
    # the value of an option given as --name=value: the --name= becomes an ignored prefix
    if [[ $cur == --*=* ]]; then
        for line in "${spec[@]}"; do
            read -r kind name repeat hint values <<<"$line"
            if [[ $kind == option && ${cur%%=*} == "--$name" ]]; then
                compset -P 1 '*='
                _cupertino_reply "$hint" "$values"
                return
            fi
        done
        return 1
    fi
    # the value of the option before the cursor
    if [[ $prev == --* ]]; then
        for line in "${spec[@]}"; do
            read -r kind name repeat hint values <<<"$line"
            if [[ $kind == option && $prev == "--$name" ]]; then
                _cupertino_reply "$hint" "$values"
                return
            fi
        done
    fi
    if [[ $seen[1] == help || -z $cmd ]]; then
        if [[ $cur == -* ]]; then
            for line in "${spec[@]}"; do
                read -r kind name repeat hint values <<<"$line"
                opts+=("--$name")
            done
            compadd -a opts
        else
            compadd -a candidates
        fi
        return
    fi
    rest=("${(@)seen[n+1,-1]}")
    if [[ $cur == -* ]]; then
        for line in "${spec[@]}"; do
            read -r kind name repeat hint values <<<"$line"
            [[ $kind == positional ]] && continue
            # a single option is offered until it is given, as --name or --name=value
            if [[ $repeat == 1 || ( -z ${(M)rest:#--$name} && -z ${(M)rest:#--$name=*} ) ]]; then
                opts+=("--$name")
            fi
        done
        compadd -a opts
        return
    fi
    # the positional argument the cursor is at
    count=0
    for (( i = 1; i <= $#rest; i++ )); do
        w=$rest[i]
        if [[ $w == --* ]]; then
            for line in "${spec[@]}"; do
                read -r kind name repeat hint values <<<"$line"
                [[ $kind == option && $w == "--$name" ]] && (( i++ ))
            done
            continue
        fi
        (( count++ ))
    done
    k=0
    for line in "${spec[@]}"; do
        read -r kind name repeat hint values <<<"$line"
        [[ $kind == positional ]] || continue
        if (( k == count )) || (( repeat == 1 && count > k )); then
            _cupertino_reply "$hint" "$values"
            break
        fi
        (( k++ ))
    done
    (( count == 0 )) && compadd -a candidates
    return 0
}

if [[ $funcstack[1] == _cupertino ]]; then
    _cupertino "$@"
else
    compdef _cupertino cupertino
fi
"#;

const FISH_ENGINE: &str = r#"# Prints what an argument takes for the word in $argv[3]: HINT VALUES (joined by commas, - for none).
function __cupertino_reply
    switch $argv[1]
        case file
            __fish_complete_path $argv[3]
        case dir
            __fish_complete_directories $argv[3]
        case command shell
            complete -C -- $argv[3]
        case '*'
            test "$argv[2]" != -; and string split , -- $argv[2]
    end
end

function __cupertino_complete
    set -l tokens (commandline -opc)
    set -l cur (commandline -ct)
    set -l prev $tokens[-1]
    # the words before the cursor without the global options (and the value of one that takes a value)
    set -l global (__cupertino_args '')
    set -l seen
    set -l i 2
    while test $i -le (count $tokens)
        set -l w $tokens[$i]
        set -l taken
        for line in $global
            set -l f (string split ' ' -- $line)
            if test "$w" = "--$f[2]"
                set taken $f[1]
                break
            else if test $f[1] = option; and string match -q -- "--$f[2]=*" $w
                set taken flag
                break
            end
        end
        switch "$taken"
            case option
                set i (math $i + 1)
            case flag
            case '*'
                set -a seen $w
        end
        set i (math $i + 1)
    end
    set -l spec $global
    # the longest command path the words start with
    set -l cmd ''
    set -l n 0
    for cpath in (__cupertino_paths)
        set -l p (string split ' ' -- $cpath)
        set -l np (count $p)
        # an index holds variables only, never a command substitution
        if test $np -gt $n; and test (count $seen) -ge $np
            if test "$seen[1..$np]" = "$cpath"
                set cmd $cpath
                set n $np
            end
        end
    end
    # the next word of every path the words (after `help`) begin
    set -l topic $seen
    if test "$seen[1]" = help
        set topic
        if test (count $seen) -ge 2
            set topic $seen[2..-1]
        end
    end
    set -l candidates
    set -l nt (count $topic)
    set -l at (math $nt + 1)
    for cpath in (__cupertino_paths)
        set -l p (string split ' ' -- $cpath)
        if test (count $p) -gt $nt
            if test $nt -eq 0; or test "$p[1..$nt]" = "$topic"
                set -l next $p[$at]
                contains -- $next $candidates; or set -a candidates $next
            end
        end
    end
    if test (count $seen) -eq 0
        set -a candidates (__cupertino_builtins)
    end
    if test -n "$cmd"
        set -a spec (__cupertino_args $cmd)
    end
    # the value of an option given as --name=value, offered with its --name=
    if string match -q -- '--*=*' $cur
        set -l given (string split -m 1 = -- $cur)
        for line in $spec
            set -l f (string split ' ' -- $line)
            if test $f[1] = option; and test $given[1] = "--$f[2]"
                for value in (__cupertino_reply $f[4] $f[5] $given[2])
                    printf '%s=%s\n' $given[1] $value
                end
                return
            end
        end
        return
    end
    # the value of the option before the cursor
    if string match -q -- '--*' $prev
        for line in $spec
            set -l f (string split ' ' -- $line)
            if test $f[1] = option; and test $prev = "--$f[2]"
                __cupertino_reply $f[4] $f[5] $cur
                return
            end
        end
    end
    if test "$seen[1]" = help; or test -z "$cmd"
        if string match -q -- '-*' $cur
            for line in $spec
                set -l f (string split ' ' -- $line)
                printf '%s\n' --$f[2]
            end
        else
            printf '%s\n' $candidates
        end
        return
    end
    set -l rest
    if test (count $seen) -gt $n
        set -l from (math $n + 1)
        set rest $seen[$from..-1]
    end
    if string match -q -- '-*' $cur
        for line in $spec
            set -l f (string split ' ' -- $line)
            test $f[1] = positional; and continue
            # a single option is offered until it is given, as --name or --name=value
            if test $f[3] = 1; or begin; not contains -- --$f[2] $rest; and not string match -q -- "--$f[2]=*" $rest; end
                printf '%s\n' --$f[2]
            end
        end
        return
    end
    # the positional argument the cursor is at
    set -l count 0
    set -l i 1
    while test $i -le (count $rest)
        set -l w $rest[$i]
        if string match -q -- '--*' $w
            for line in $spec
                set -l f (string split ' ' -- $line)
                if test $f[1] = option; and test $w = "--$f[2]"
                    set i (math $i + 1)
                end
            end
        else
            set count (math $count + 1)
        end
        set i (math $i + 1)
    end
    set -l k 0
    for line in $spec
        set -l f (string split ' ' -- $line)
        test $f[1] = positional; or continue
        if test $k -eq $count; or begin; test $f[3] = 1; and test $count -gt $k; end
            __cupertino_reply $f[4] $f[5] $cur
            break
        end
        set k (math $k + 1)
    end
    if test $count -eq 0
        printf '%s\n' $candidates
    end
end

complete -c cupertino -f -a '(__cupertino_complete)'
"#;
