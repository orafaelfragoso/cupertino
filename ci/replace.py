#!/usr/bin/env python3
"""Replace a literal or a regular expression in files and fail when a file does not match.

Usage: python3 ci/replace.py [--regex] [--count N] OLD NEW FILE...

Every file must contain OLD (with --count, exactly N times); otherwise nothing is written and the exit status is 1
with the files named. With --regex, NEW may use \\1 and \\g<name>. The changed lines are printed as a diff.
"""
import argparse
import difflib
import re
import sys
from pathlib import Path


def replace(text: str, old: str, new: str, regex: bool, count: int | None) -> tuple[str, int]:
    if regex:
        return re.subn(old, new, text, flags=re.MULTILINE)
    return text.replace(old, new), text.count(old)


def run(old: str, new: str, files: list[Path], regex: bool = False, count: int | None = None) -> list[str]:
    """Apply the replacement to every file, or to none; the returned problems are empty on success."""
    changes: dict[Path, tuple[str, str]] = {}
    problems = []
    for path in files:
        try:
            before = path.read_text()
            after, found = replace(before, old, new, regex, count)
        except (OSError, UnicodeDecodeError, re.error) as e:
            problems.append(f"{path}: {e}")
            continue
        if found == 0 or (count is not None and found != count):
            expected = "at least one match" if count is None else f"{count} matches"
            problems.append(f"{path}: expected {expected}, found {found}")
        else:
            changes[path] = (before, after)
    if problems:
        return problems
    for path, (before, after) in changes.items():
        path.write_text(after)
        sys.stdout.writelines(difflib.unified_diff(before.splitlines(True), after.splitlines(True), f"a/{path}", f"b/{path}", n=0))
    return []


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--regex", action="store_true", help="OLD is a regular expression (multiline)")
    p.add_argument("--count", type=int, help="the exact number of matches expected in each file")
    p.add_argument("old")
    p.add_argument("new")
    p.add_argument("files", nargs="+", type=Path)
    args = p.parse_args()
    problems = run(args.old, args.new, args.files, args.regex, args.count)
    for problem in problems:
        print(f"replace: {problem}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
