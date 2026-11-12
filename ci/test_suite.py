#!/usr/bin/env python3
"""One integration test program per crate.

Every `crates/*/tests/*.rs` file is a module of the crate's single test program, `tests/suite.rs`, which this script
writes. One program links the crate and its dependencies once, where a program per file linked them once per file.

    ci/test_suite.py --check            fail when a suite.rs is missing a test file or is out of date
    ci/test_suite.py --write            write every crate's suite.rs from the files on disk
    ci/test_suite.py --convert CRATE..  move crates to the layout: autotests off, one [[test]] named `suite`, a test's
                                        required features as a `#![cfg]` in its file, shared helpers declared once

A file's own `#![cfg(...)]` decides whether its module is built. Helper directories (`tests/common/`,
`tests/support/`, `tests/diag/`) are declared once in suite.rs, and the files reach them as `crate::common`.
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SHARED = ("common", "support", "diag")
HEADER = (
    "// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one\n"
    "// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.\n"
)
KEYWORDS = {"async", "await", "box", "crate", "dyn", "fn", "impl", "loop", "match", "mod", "move", "ref", "self",
            "static", "super", "trait", "type", "unsafe", "use", "where", "while", "abstract", "become", "do", "final",
            "macro", "override", "priv", "try", "typeof", "unsized", "virtual", "yield", "gen", "test", "core", "std"}


def ident(stem: str) -> str:
    name = re.sub(r"[^A-Za-z0-9_]", "_", stem)
    if name[0].isdigit() or name in KEYWORDS:
        name = "t_" + name
    return name


def converted(crate: pathlib.Path) -> bool:
    text = (crate / "Cargo.toml").read_text()
    return re.search(r"^autotests\s*=\s*false", text, re.M) is not None and (crate / "tests" / "suite.rs").exists()


def suite_text(tests: pathlib.Path) -> str:
    files = sorted(p for p in tests.glob("*.rs") if p.name != "suite.rs")
    lines = [HEADER, "\n"]
    shared = [d for d in SHARED if (tests / d / "mod.rs").exists() or (tests / f"{d}.rs").exists()]
    for d in shared:
        lines.append(f"#[allow(dead_code, unused_imports, unused_macros)]\nmod {d};\n")
    if shared:
        lines.append("\n")
    # in module name order, as rustfmt sorts them
    for p in sorted(files, key=lambda p: ident(p.stem)):
        if p.stem in SHARED:
            continue
        lines.append(f'#[path = "{p.name}"]\nmod {ident(p.stem)};\n')
    return "".join(lines)


def crates():
    return sorted(c for c in (ROOT / "crates").iterdir() if (c / "tests").is_dir() and list((c / "tests").glob("*.rs")))


def check() -> int:
    bad = []
    for c in crates():
        tests = c / "tests"
        if not converted(c):
            bad.append(f"{c.name}: not in the one-program layout (run ci/test_suite.py --convert {c.name})")
            continue
        if (tests / "suite.rs").read_text() != suite_text(tests):
            bad.append(f"{c.name}: tests/suite.rs is out of date (run ci/test_suite.py --write)")
    for b in bad:
        print(b, file=sys.stderr)
    return 1 if bad else 0


def write() -> int:
    for c in crates():
        if converted(c):
            (c / "tests" / "suite.rs").write_text(suite_text(c / "tests"))
    return 0


def convert(names) -> int:
    for name in names:
        c = ROOT / "crates" / name
        tests = c / "tests"
        toml = (c / "Cargo.toml").read_text()
        # required features of the old [[test]] entries move into their files
        blocks = re.findall(r"\[\[test\]\]\n((?:[^\[\n][^\n]*\n|\n)*)", toml)
        for block in blocks:
            tname = re.search(r'^name\s*=\s*"([^"]+)"', block, re.M)
            feats = re.search(r"^required-features\s*=\s*\[([^\]]*)\]", block, re.M)
            if not tname:
                continue
            path = re.search(r'^path\s*=\s*"([^"]+)"', block, re.M)
            file = c / path.group(1) if path else tests / f"{tname.group(1)}.rs"
            if feats and file.exists():
                fs = [f.strip().strip('"') for f in feats.group(1).split(",") if f.strip()]
                cond = ", ".join(f'feature = "{f}"' for f in fs)
                cond = cond if len(fs) == 1 else f"all({cond})"
                text = file.read_text()
                if f"#![cfg({cond})]" not in text:
                    file.write_text(f"#![cfg({cond})]\n" + text)
        toml = re.sub(r"\n*\[\[test\]\]\n(?:[^\[\n][^\n]*\n|\n)*", "\n\n", toml).rstrip() + "\n"
        if not re.search(r"^autotests\s*=", toml, re.M):
            toml = re.sub(r"(\[package\]\n(?:[^\[\n][^\n]*\n)*)", lambda m: m.group(1) + "autotests = false\n", toml, count=1)
        toml += '\n[[test]]\nname = "suite"\npath = "tests/suite.rs"\n'
        (c / "Cargo.toml").write_text(toml)
        # shared helpers: declared once in suite.rs, reached as crate::<name>
        for p in tests.glob("*.rs"):
            if p.name == "suite.rs":
                continue
            text = p.read_text()
            new = re.sub(r"^mod (common|support|diag);$", r"use crate::\1;", text, flags=re.M)
            if new != text:
                p.write_text(new)
        (tests / "suite.rs").write_text(suite_text(tests))
        print(f"converted {name}")
    return 0


if __name__ == "__main__":
    args = sys.argv[1:]
    if args[:1] == ["--check"]:
        sys.exit(check())
    if args[:1] == ["--write"]:
        sys.exit(write())
    if args[:1] == ["--convert"]:
        sys.exit(convert(args[1:]))
    print(__doc__, file=sys.stderr)
    sys.exit(2)
