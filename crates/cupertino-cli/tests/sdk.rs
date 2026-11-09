//! `cupertino sdk`: each command against a temporary root that holds a fake SDK and fake tools, never the machine's.

use cupertino_cli::{Context, Output, Runner, REGISTRY};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;

fn run_in(root: &Path, words: &[&str]) -> Output {
    let words: Vec<String> = words.iter().map(|w| w.to_string()).collect();
    Runner::new(REGISTRY)
        .context(Context::isolated(root))
        .run(&words, "r1")
}

fn json(out: &Output) -> Value {
    assert_eq!(out.stderr, "", "{}", out.stdout);
    serde_json::from_str(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", out.stdout))
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn executable(path: &Path, script: &str) {
    write(path, script);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A little-endian ELF64 shared object whose dynamic section names `soname`, the part of one that `sdk info` reads.
fn elf(soname: &str) -> Vec<u8> {
    let mut strings = vec![0u8];
    strings.extend_from_slice(soname.as_bytes());
    strings.push(0);
    let strings_at = 64u64;
    let dynamic_at = (strings_at + strings.len() as u64).next_multiple_of(8);
    let mut dynamic = Vec::new();
    for (tag, value) in [(14u64, 1u64), (0, 0)] {
        dynamic.extend_from_slice(&tag.to_le_bytes());
        dynamic.extend_from_slice(&value.to_le_bytes());
    }
    let sections_at = dynamic_at + dynamic.len() as u64;
    let mut file = vec![0u8; 64];
    file[..8].copy_from_slice(b"\x7fELF\x02\x01\x01\x00");
    file[0x10..0x12].copy_from_slice(&3u16.to_le_bytes());
    file[0x12..0x14].copy_from_slice(&0x3eu16.to_le_bytes());
    file[0x14..0x18].copy_from_slice(&1u32.to_le_bytes());
    file[0x28..0x30].copy_from_slice(&sections_at.to_le_bytes());
    file[0x34..0x36].copy_from_slice(&64u16.to_le_bytes());
    file[0x3a..0x3c].copy_from_slice(&64u16.to_le_bytes());
    file[0x3c..0x3e].copy_from_slice(&3u16.to_le_bytes());
    file.extend_from_slice(&strings);
    file.resize(dynamic_at as usize, 0);
    file.extend_from_slice(&dynamic);
    let section = |kind: u32, offset: u64, size: u64, link: u32| {
        let mut s = vec![0u8; 64];
        s[4..8].copy_from_slice(&kind.to_le_bytes());
        s[0x18..0x20].copy_from_slice(&offset.to_le_bytes());
        s[0x20..0x28].copy_from_slice(&size.to_le_bytes());
        s[0x28..0x2c].copy_from_slice(&link.to_le_bytes());
        s
    };
    file.extend(section(0, 0, 0, 0));
    file.extend(section(3, strings_at, strings.len() as u64, 0));
    file.extend(section(6, dynamic_at, dynamic.len() as u64, 1));
    file
}

const HEADER: &str =
    "/* the header */\n#define CUPERTINOKIT_ABI_MAJOR 1\n#define CUPERTINOKIT_ABI_MINOR 0\n";

/// The files of the cupertino-kit package under `root`, as release.py installs them.
fn install_sdk(root: &Path) {
    write(
        &root.join("usr/include/cupertinokit-1/cupertinokit.h"),
        HEADER,
    );
    let lib = root.join("usr/lib");
    fs::create_dir_all(lib.join("pkgconfig")).unwrap();
    fs::write(
        lib.join("libcupertinokit.so.1.0.0"),
        elf("libcupertinokit.so.1"),
    )
    .unwrap();
    symlink("libcupertinokit.so.1.0.0", lib.join("libcupertinokit.so.1")).unwrap();
    symlink("libcupertinokit.so.1", lib.join("libcupertinokit.so")).unwrap();
    write(
        &lib.join("pkgconfig/cupertinokit.pc"),
        "prefix=/usr\nName: cupertinokit\nVersion: 1.0.0\nLibs: -lcupertinokit\n",
    );
    write(
        &root.join("usr/share/cupertino-kit/build-info.json"),
        r#"{"package": "cupertino-kit", "version": "0.1.0", "commit": "0123456789abcdef0123456789abcdef01234567",
            "source_date_epoch": 0}"#,
    );
}

fn parts(info: &Value) -> Vec<(String, String)> {
    info["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["name"].as_str().unwrap().to_owned(),
                p["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn statuses(status: &str) -> Vec<(String, String)> {
    ["header", "library", "link", "pkg_config", "package"]
        .iter()
        .map(|n| (n.to_string(), status.to_string()))
        .collect()
}

#[test]
fn info_reports_an_sdk_that_is_not_installed_and_exits_0() {
    let root = tempfile::tempdir().unwrap();
    let out = run_in(root.path(), &["--format", "json", "sdk", "info"]);
    assert_eq!(out.code, 0);
    let data = &json(&out)["data"];
    assert_eq!(data["installed"], false);
    assert_eq!(parts(data), statuses("not_installed"));
    assert_eq!(data["abi"], Value::Null);

    let human = run_in(root.path(), &["sdk", "info"]);
    assert_eq!(human.code, 0);
    assert!(
        human
            .stdout
            .starts_with("CupertinoKit C SDK: not installed (install the cupertino-kit package)\n"),
        "{}",
        human.stdout
    );
}

#[test]
fn info_require_fails_with_unavailable_when_the_sdk_is_missing() {
    let root = tempfile::tempdir().unwrap();
    let out = run_in(
        root.path(),
        &["--format", "json", "sdk", "info", "--require"],
    );
    assert_eq!(out.code, 3);
    let envelope = json(&out);
    assert_eq!(envelope["error"]["kind"], "unavailable");
    assert_eq!(envelope["error"]["code"], "sdk_not_installed");
    assert_eq!(parts(&envelope["data"]), statuses("not_installed"));
}

#[test]
fn info_reads_every_file_of_an_installed_sdk() {
    let root = tempfile::tempdir().unwrap();
    install_sdk(root.path());
    let out = run_in(
        root.path(),
        &["--format", "json", "sdk", "info", "--require"],
    );
    assert_eq!(out.code, 0, "{}", out.stdout);
    let data = &json(&out)["data"];
    assert_eq!(data["installed"], true);
    assert_eq!(parts(data), statuses("ok"));
    assert_eq!(data["abi"], "1.0");
    assert_eq!(data["soname"], "libcupertinokit.so.1");
    assert_eq!(data["pkg_config_version"], "1.0.0");
    assert_eq!(data["package_version"], "0.1.0");
    assert_eq!(data["parts"][1]["target"], "libcupertinokit.so.1.0.0");
    assert_eq!(data["parts"][2]["target"], "libcupertinokit.so.1");
    assert_eq!(data["problems"], serde_json::json!([]));

    let human = run_in(root.path(), &["sdk", "info"]);
    assert_eq!(
        human.stdout,
        "CupertinoKit C SDK: installed, ABI 1.0\n\
         \x20 header      /usr/include/cupertinokit-1/cupertinokit.h: ABI 1.0\n\
         \x20 library     /usr/lib/libcupertinokit.so.1 -> libcupertinokit.so.1.0.0: soname libcupertinokit.so.1\n\
         \x20 link        /usr/lib/libcupertinokit.so -> libcupertinokit.so.1\n\
         \x20 pkg-config  /usr/lib/pkgconfig/cupertinokit.pc: version 1.0.0\n\
         \x20 package     /usr/share/cupertino-kit/build-info.json: cupertino-kit 0.1.0 (0123456789ab)\n"
    );
}

#[test]
fn info_root_reads_an_sdk_under_another_directory() {
    let machine = tempfile::tempdir().unwrap();
    install_sdk(&machine.path().join("staged"));
    let out = run_in(
        machine.path(),
        &["--format", "json", "sdk", "info", "--root", "staged"],
    );
    assert_eq!(out.code, 0);
    let data = &json(&out)["data"];
    assert_eq!(data["installed"], true);
    assert_eq!(
        data["root"],
        machine.path().join("staged").to_str().unwrap()
    );
    let missing = run_in(
        machine.path(),
        &["--format", "json", "sdk", "info", "--root", "nowhere"],
    );
    assert_eq!(missing.code, 2);
    assert_eq!(json(&missing)["error"]["code"], "invalid_value");
}

#[test]
fn files_that_disagree_are_problems_and_fail_require_with_validation() {
    let root = tempfile::tempdir().unwrap();
    install_sdk(root.path());
    write(
        &root.path().join("usr/lib/pkgconfig/cupertinokit.pc"),
        "Name: cupertinokit\nVersion: 1.1.0\n",
    );
    fs::write(
        root.path().join("usr/lib/libcupertinokit.so.1.0.0"),
        elf("libcupertinokit.so.2"),
    )
    .unwrap();
    let out = run_in(root.path(), &["--format", "json", "sdk", "info"]);
    assert_eq!(out.code, 0);
    let data = &json(&out)["data"];
    assert_eq!(data["installed"], false);
    assert_eq!(parts(data), statuses("ok"));
    assert_eq!(
        data["problems"],
        serde_json::json!([
            "the library's soname is libcupertinokit.so.2, the header's ABI 1.0 needs libcupertinokit.so.1",
            "the pkg-config Version is 1.1.0, the header's ABI 1.0 is 1.0.0",
        ])
    );
    let required = run_in(
        root.path(),
        &["--format", "json", "sdk", "info", "--require"],
    );
    assert_eq!(required.code, 5);
    let envelope = json(&required);
    assert_eq!(envelope["error"]["kind"], "validation");
    assert_eq!(envelope["error"]["code"], "sdk_invalid");
    assert_eq!(envelope["data"]["problems"].as_array().unwrap().len(), 2);
}

#[test]
fn a_file_that_is_not_what_it_should_be_is_unreadable() {
    let root = tempfile::tempdir().unwrap();
    install_sdk(root.path());
    fs::write(
        root.path().join("usr/lib/libcupertinokit.so.1.0.0"),
        "not a library",
    )
    .unwrap();
    write(
        &root
            .path()
            .join("usr/include/cupertinokit-1/cupertinokit.h"),
        "/* no version */\n",
    );
    write(
        &root.path().join("usr/share/cupertino-kit/build-info.json"),
        r#"{"package": "cupertino", "version": "0.1.0", "commit": null}"#,
    );
    fs::remove_file(root.path().join("usr/lib/libcupertinokit.so")).unwrap();
    fs::write(root.path().join("usr/lib/libcupertinokit.so"), "a copy").unwrap();
    let out = run_in(root.path(), &["--format", "json", "sdk", "info"]);
    let data = &json(&out)["data"];
    assert_eq!(
        parts(data),
        [
            ("header", "unreadable"),
            ("library", "unreadable"),
            ("link", "unreadable"),
            ("pkg_config", "ok"),
            ("package", "unreadable"),
        ]
        .map(|(n, s)| (n.to_owned(), s.to_owned()))
    );
    assert_eq!(
        data["parts"][1]["detail"], "it is cut short",
        "a 13-byte file has no ELF header"
    );
    assert_eq!(
        data["parts"][4]["detail"],
        "it is the record of cupertino, not cupertino-kit"
    );
    let required = run_in(root.path(), &["sdk", "info", "--require"]);
    assert_eq!(required.code, 5);
    assert!(
        required.stderr.starts_with(
            "cupertino: the installed C SDK is not consistent: /usr/include/cupertinokit-1/cupertinokit.h is \
             unreadable; "
        ),
        "{}",
        required.stderr
    );
}

/// pkg-config for an SDK under the sysroot it is given, as pkgconf answers.
const PKG_CONFIG: &str = r#"#!/bin/sh
case "$1" in
--modversion) echo 1.0.0 ;;
--cflags) echo "-I$PKG_CONFIG_SYSROOT_DIR/usr/include/cupertinokit-1" ;;
--libs) echo "-L$PKG_CONFIG_SYSROOT_DIR/usr/lib -lcupertinokit" ;;
*) exit 2 ;;
esac
"#;

/// A compiler that writes a probe printing `output`, and fails unless it runs with the environment `check` gives it.
fn compiler(output: &str) -> String {
    format!(
        "#!/bin/sh\n\
         [ \"$PATH\" = /usr/bin ] && [ -z \"$HOME\" ] && [ \"$LC_ALL\" = C ] || {{ echo \"cc: environment: $PATH $HOME\" >&2; exit 1; }}\n\
         printf '#!/bin/sh\\nprintf \"{output}\"\\n[ -n \"$LD_LIBRARY_PATH\" ]\\n' > probe\n\
         chmod +x probe\n"
    )
}

fn tools(root: &Path, cc: &str) {
    executable(&root.join("usr/bin/cc"), cc);
    executable(&root.join("usr/bin/pkg-config"), PKG_CONFIG);
}

const GOOD_PROBE: &str = "header 1.0\\nlibrary 1.0\\nruntime 0\\nrelease 0\\n";

#[test]
fn check_compiles_and_runs_the_probe_and_reports_each_step() {
    let root = tempfile::tempdir().unwrap();
    tools(root.path(), &compiler(GOOD_PROBE));
    let out = run_in(root.path(), &["--format", "json", "sdk", "check"]);
    assert_eq!(out.code, 0, "{}", out.stdout);
    let data = &json(&out)["data"];
    assert_eq!(data["ok"], true);
    assert_eq!(data["abi"], "1.0");
    assert!(
        data.get("events").is_none(),
        "the report keeps its own data"
    );
    let steps: Vec<&str> = data["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["step"].as_str().unwrap())
        .collect();
    assert_eq!(steps, ["compiler", "pkg_config", "compile", "run"]);
    let include = format!("-I{}/usr/include/cupertinokit-1", root.path().display());
    assert_eq!(data["steps"][1]["cflags"], include.as_str());
    assert!(data["steps"][2]["command"]
        .as_str()
        .unwrap()
        .starts_with(&format!(
            "/usr/bin/cc -std=c11 -Wall -Wextra {include} probe.c -o probe -L"
        )));

    let lines = run_in(root.path(), &["--format", "jsonl", "sdk", "check"]);
    assert_eq!(lines.code, 0);
    let records: Vec<Value> = lines
        .stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let kinds: Vec<&str> = records
        .iter()
        .map(|r| r["event"].as_str().unwrap_or(r["type"].as_str().unwrap()))
        .collect();
    assert_eq!(kinds, ["step", "step", "step", "step", "result", "end"]);

    let human = run_in(root.path(), &["sdk", "check"]);
    let lines: Vec<&str> = human.stdout.lines().collect();
    assert_eq!(lines[0], "compiler: /usr/bin/cc");
    assert_eq!(
        lines[3],
        "run: header and library are ABI 1.0; a headless runtime started and stopped"
    );
    assert_eq!(lines[4], "The C SDK builds and runs: ABI 1.0");
}

#[test]
fn check_without_a_compiler_or_pkg_config_is_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let out = run_in(root.path(), &["--format", "json", "sdk", "check"]);
    assert_eq!(out.code, 3);
    assert_eq!(json(&out)["error"]["code"], "no_compiler");

    executable(&root.path().join("usr/bin/cc"), &compiler(GOOD_PROBE));
    let out = run_in(root.path(), &["--format", "json", "sdk", "check"]);
    assert_eq!(out.code, 3);
    assert_eq!(json(&out)["error"]["code"], "no_pkg_config");

    executable(
        &root.path().join("usr/bin/pkg-config"),
        "#!/bin/sh\necho 'Package cupertinokit was not found' >&2\nexit 1\n",
    );
    let out = run_in(root.path(), &["--format", "json", "sdk", "check"]);
    assert_eq!(out.code, 3);
    let envelope = json(&out);
    assert_eq!(envelope["error"]["code"], "sdk_not_installed");
    assert_eq!(
        envelope["data"]["stderr"],
        "Package cupertinokit was not found\n"
    );
}

#[test]
fn a_compile_error_is_validation_with_the_compiler_messages() {
    let root = tempfile::tempdir().unwrap();
    tools(
        root.path(),
        "#!/bin/sh\necho 'probe.c:3:10: fatal error: cupertinokit.h: No such file or directory' >&2\nexit 1\n",
    );
    let out = run_in(root.path(), &["--format", "json", "sdk", "check"]);
    assert_eq!(out.code, 5);
    let envelope = json(&out);
    assert_eq!(envelope["error"]["kind"], "validation");
    assert_eq!(envelope["error"]["code"], "compile_failed");
    assert_eq!(
        envelope["data"]["stderr"],
        "probe.c:3:10: fatal error: cupertinokit.h: No such file or directory\n"
    );
    let steps: Vec<&str> = envelope["data"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["step"].as_str().unwrap())
        .collect();
    assert_eq!(steps, ["compiler", "pkg_config"]);
}

#[test]
fn a_header_and_library_of_different_versions_are_an_abi_mismatch() {
    let root = tempfile::tempdir().unwrap();
    tools(
        root.path(),
        &compiler("header 1.1\\nlibrary 1.0\\nruntime 0\\nrelease 0\\n"),
    );
    let out = run_in(root.path(), &["--format", "json", "sdk", "check"]);
    assert_eq!(out.code, 5);
    assert_eq!(json(&out)["error"]["code"], "abi_mismatch");

    tools(
        root.path(),
        &compiler("header 1.0\\nlibrary 1.0\\nruntime 2\\n"),
    );
    let out = run_in(root.path(), &["--format", "json", "sdk", "check"]);
    assert_eq!(out.code, 5);
    let envelope = json(&out);
    assert_eq!(envelope["error"]["code"], "probe_failed");
    assert_eq!(
        envelope["data"]["stdout"],
        "header 1.0\nlibrary 1.0\nruntime 2\n"
    );
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

#[test]
fn new_writes_a_c_project_named_after_the_app() {
    let cwd = tempfile::tempdir().unwrap();
    let out = run_in(
        cwd.path(),
        &["--format", "json", "sdk", "new", "--lang", "c", "demo"],
    );
    assert_eq!(out.code, 0, "{}", out.stdout);
    let data = &json(&out)["data"];
    assert_eq!(data["lang"], "c");
    assert_eq!(data["dir"], "demo");
    let files: Vec<&str> = data["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(files, ["main.c", "Makefile", "README.md", "demo.desktop"]);
    let dir = cwd.path().join("demo");
    assert_eq!(mode(&dir), 0o755);
    for f in files {
        assert_eq!(mode(&dir.join(f)), 0o644, "{f}");
        let text = fs::read_to_string(dir.join(f)).unwrap();
        assert!(!text.contains("{{"), "{f} keeps a placeholder");
    }
    let main = fs::read_to_string(dir.join("main.c")).unwrap();
    assert!(main.contains("#define APP_ID \"demo\""));
    let makefile = fs::read_to_string(dir.join("Makefile")).unwrap();
    assert!(
        makefile.contains("\n\t$(CC) $(CPPFLAGS) $(CFLAGS) $(LDFLAGS) -o $@ main.c $(LDLIBS)\n")
    );
    assert!(makefile.starts_with("# Builds demo "));
    assert!(makefile.contains("\ndemo: main.c\n"));
    let entry = fs::read_to_string(dir.join("demo.desktop")).unwrap();
    assert!(entry.contains("\nName=demo\n") && entry.contains("\nExec=demo\n"));

    let human = run_in(cwd.path(), &["sdk", "new", "--lang", "c", "other"]);
    assert!(
        human
            .stdout
            .ends_with("Build and run it: make -C other && ./other/other\n"),
        "{}",
        human.stdout
    );
}

#[test]
fn new_writes_a_rust_project_on_the_cupertino_kit_of_this_release() {
    let cwd = tempfile::tempdir().unwrap();
    let out = run_in(
        cwd.path(),
        &[
            "sdk",
            "new",
            "--lang",
            "rust",
            "org.example.Counter",
            "--dir",
            "nested/counter",
        ],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let dir = cwd.path().join("nested/counter");
    for d in [cwd.path().join("nested"), dir.clone(), dir.join("src")] {
        assert_eq!(mode(&d), 0o755);
    }
    let manifest = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("\nname = \"org-example-counter\"\n"));
    let tag = format!(
        "{{ git = \"https://github.com/orafaelfragoso/cupertino\", tag = \"v{}\" }}",
        env!("CARGO_PKG_VERSION")
    );
    assert!(manifest.contains(&format!("\ncupertino-kit = {tag}\n")));
    assert!(manifest.contains(&format!("\n[patch.crates-io]\nxkbcommon = {tag}\n")));
    let main = fs::read_to_string(dir.join("src/main.rs")).unwrap();
    assert!(main.contains("const APP_ID: &str = \"org.example.Counter\";"));
    let entry = fs::read_to_string(dir.join("org.example.Counter.desktop")).unwrap();
    assert!(entry.contains("\nExec=org-example-counter\n"));
    assert_eq!(
        fs::read_to_string(dir.join(".gitignore")).unwrap(),
        "/target\n"
    );
}

#[test]
fn new_refuses_a_bad_name_a_missing_language_and_a_used_directory() {
    let cwd = tempfile::tempdir().unwrap();
    for (words, code, kind) in [
        (&["sdk", "new", "demo"][..], 2, "missing_option"),
        (
            &["sdk", "new", "--lang", "go", "demo"][..],
            2,
            "invalid_value",
        ),
        (
            &["sdk", "new", "--lang", "c", "2048"][..],
            2,
            "invalid_name",
        ),
        (
            &["sdk", "new", "--lang", "c", "org..app"][..],
            2,
            "invalid_name",
        ),
        (&["sdk", "new", "--lang", "c", "a/b"][..], 2, "invalid_name"),
        (&["sdk", "new", "--lang", "c", ".."][..], 2, "invalid_name"),
        (
            &["sdk", "new", "--lang", "rust", "Self"][..],
            2,
            "invalid_name",
        ),
    ] {
        let mut line = vec!["--format", "json"];
        line.extend_from_slice(words);
        let out = run_in(cwd.path(), &line);
        assert_eq!(out.code, code, "{words:?}");
        assert_eq!(json(&out)["error"]["code"], kind, "{words:?}");
    }
    assert_eq!(fs::read_dir(cwd.path()).unwrap().count(), 0);

    write(&cwd.path().join("used/file"), "");
    write(&cwd.path().join("plain"), "");
    let used = run_in(
        cwd.path(),
        &["--format", "json", "sdk", "new", "--lang", "c", "used"],
    );
    assert_eq!(used.code, 5);
    assert_eq!(json(&used)["error"]["code"], "directory_not_empty");
    let plain = run_in(
        cwd.path(),
        &["--format", "json", "sdk", "new", "--lang", "c", "plain"],
    );
    assert_eq!(plain.code, 5);
    assert_eq!(json(&plain)["error"]["code"], "not_a_directory");
    fs::create_dir(cwd.path().join("empty")).unwrap();
    let empty = run_in(cwd.path(), &["sdk", "new", "--lang", "c", "empty"]);
    assert_eq!(
        empty.code, 0,
        "an empty directory is used: {}",
        empty.stderr
    );
}

#[test]
fn new_into_a_directory_it_cannot_write_is_denied() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: root writes into any directory");
        return;
    }
    let cwd = tempfile::tempdir().unwrap();
    let locked = cwd.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    let out = run_in(
        cwd.path(),
        &[
            "--format",
            "json",
            "sdk",
            "new",
            "--lang",
            "c",
            "demo",
            "--dir",
            "locked/demo",
        ],
    );
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(out.code, 4);
    let envelope = json(&out);
    assert_eq!(envelope["error"]["kind"], "denied");
    assert_eq!(envelope["error"]["code"], "not_writable");
    assert_eq!(fs::read_dir(&locked).unwrap().count(), 0);
}

#[test]
fn a_library_whose_offsets_overflow_is_unreadable() {
    let root = tempfile::tempdir().unwrap();
    install_sdk(root.path());
    // the string table's offset is the largest there is, so its soname's offset overflows
    let mut library = elf("libcupertinokit.so.1");
    let sections = u64::from_le_bytes(library[0x28..0x30].try_into().unwrap()) as usize;
    library[sections + 64 + 0x18..sections + 64 + 0x20].copy_from_slice(&u64::MAX.to_le_bytes());
    fs::write(
        root.path().join("usr/lib/libcupertinokit.so.1.0.0"),
        library,
    )
    .unwrap();
    let out = run_in(root.path(), &["--format", "json", "sdk", "info"]);
    assert_eq!(out.code, 0);
    let data = &json(&out)["data"];
    assert_eq!(data["parts"][1]["status"], "unreadable");
    assert_eq!(data["parts"][1]["detail"], "its offsets are not valid");
}

/// The crates of a Cupertino checkout that a Rust project depends on, under `dir`.
fn checkout(dir: &Path) {
    for krate in ["cupertino-kit", "cupertino-xkbcommon"] {
        write(&dir.join("crates").join(krate).join("Cargo.toml"), "");
    }
}

#[test]
fn new_rust_with_source_depends_on_the_checkouts_crates_by_path() {
    let cwd = tempfile::tempdir().unwrap();
    checkout(&cwd.path().join("src/cupertino \"x\""));
    let out = run_in(
        cwd.path(),
        &[
            "--format",
            "json",
            "sdk",
            "new",
            "--lang",
            "rust",
            "demo",
            "--source",
            "src/cupertino \"x\"",
        ],
    );
    assert_eq!(out.code, 0, "{}", out.stdout);
    let checkout = fs::canonicalize(cwd.path().join("src/cupertino \"x\"")).unwrap();
    let checkout = checkout.to_str().unwrap();
    let data = &json(&out)["data"];
    assert_eq!(
        data["kit"],
        serde_json::json!({ "path": format!("{checkout}/crates/cupertino-kit") })
    );
    let manifest = fs::read_to_string(cwd.path().join("demo/Cargo.toml")).unwrap();
    let quoted =
        |krate: &str| serde_json::to_string(&format!("{checkout}/crates/{krate}")).unwrap();
    assert!(manifest.contains(&format!(
        "\ncupertino-kit = {{ path = {} }}\n",
        quoted("cupertino-kit")
    )));
    assert!(manifest.contains(&format!(
        "\n[patch.crates-io]\nxkbcommon = {{ path = {} }}\n",
        quoted("cupertino-xkbcommon")
    )));
    assert!(!manifest.contains("git ="), "{manifest}");
    // the quote in the directory's name is escaped as TOML needs
    assert!(
        manifest.contains("cupertino \\\"x\\\"/crates/cupertino-kit\" }"),
        "{manifest}"
    );
    let readme = fs::read_to_string(cwd.path().join("demo/README.md")).unwrap();
    assert!(readme.contains(&format!("taken from the checkout {checkout}.")));

    // without --source the project names the tag, and says what to do when it cannot be fetched
    let tagged = run_in(cwd.path(), &["sdk", "new", "--lang", "rust", "tagged"]);
    assert_eq!(tagged.code, 0, "{}", tagged.stderr);
    let version = env!("CARGO_PKG_VERSION");
    assert!(tagged.stdout.contains(&format!(
        "It takes cupertino-kit from the git tag v{version} of https://github.com/orafaelfragoso/cupertino.\n\
         Offline, or before that tag is published, give --source with a Cupertino checkout instead.\n"
    )));
    let readme = fs::read_to_string(cwd.path().join("tagged/README.md")).unwrap();
    assert!(readme.contains("--source CHECKOUT"));
}

#[test]
fn new_refuses_a_source_that_is_no_checkout_or_for_c() {
    let cwd = tempfile::tempdir().unwrap();
    fs::create_dir(cwd.path().join("empty")).unwrap();
    checkout(&cwd.path().join("cupertino"));
    for words in [
        &["sdk", "new", "--lang", "rust", "demo", "--source", "empty"][..],
        &[
            "sdk", "new", "--lang", "rust", "demo", "--source", "nowhere",
        ],
        &["sdk", "new", "--lang", "c", "demo", "--source", "cupertino"],
    ] {
        let mut line = vec!["--format", "json"];
        line.extend_from_slice(words);
        let out = run_in(cwd.path(), &line);
        assert_eq!(out.code, 2, "{words:?}");
        assert_eq!(json(&out)["error"]["code"], "invalid_value", "{words:?}");
    }
    assert!(!cwd.path().join("demo").exists());
}

#[test]
fn new_refuses_a_name_that_looks_like_an_option() {
    let cwd = tempfile::tempdir().unwrap();
    let out = run_in(
        cwd.path(),
        &["--format", "json", "sdk", "new", "--lang", "c", "--", "-rf"],
    );
    assert_eq!(out.code, 2);
    assert_eq!(json(&out)["error"]["code"], "invalid_name");
    assert_eq!(fs::read_dir(cwd.path()).unwrap().count(), 0);
}

#[test]
fn new_into_an_absolute_directory_names_the_program_by_that_path() {
    let cwd = tempfile::tempdir().unwrap();
    let dir = cwd.path().join("abs");
    let out = run_in(
        cwd.path(),
        &[
            "sdk",
            "new",
            "--lang",
            "c",
            "demo",
            "--dir",
            dir.to_str().unwrap(),
        ],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let shown = dir.display();
    assert!(
        out.stdout.ends_with(&format!(
            "Build and run it: make -C {shown} && {shown}/demo\n"
        )),
        "{}",
        out.stdout
    );
}
