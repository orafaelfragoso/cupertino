#!/usr/bin/env python3
"""Check a source tree. Usage: python3 ci/gate.py ROOT."""
import argparse
import ast
import hashlib
import os
from pathlib import Path
import re
import sys
from collections import Counter

FORBIDDEN_SUFFIXES = {".ttf", ".otf", ".ttc"}

ALLOWED_BINARIES = {
    "crates/cupertino-assets/fonts/InterVariable-subset.ttf": "a38802ab5d40c789b19864858e7329f79787a12cff0f8904933295729189dd6f",
}

BINARY_MAGIC: set[bytes] = set()

ID = r"\b[A-Z]{2,5}-(?:\d{3}[a-z]?|nnn|xxx)\b"

CLASS = r"\b(?:VERIFIED|DOCUMENTED|INFERRED|APPROXIMATED|UNKNOWN)\b"

CACHE_ADDR = r"@?\b0x(?:7ff[0-9a-fA-F]{9}|1[0-9a-fA-F]{8})\b(?:…|\.\.\.)?"

GATE = [
    ("evidence ID", re.compile(ID)),
    ("evidence class", re.compile(CLASS)),
    ("absolute address", re.compile(CACHE_ADDR)),
    ("provenance wording", re.compile(r"\brecovered\b|\bresearch\b|\b[Tt]he reference\b(?!(?:\s|//[/!]?)+(?:implementation|renderer|image|path|output|kernel))", re.I)),
    ("private document path", re.compile(r"\b(?:research|references|specifications|extraction)/\S|\b(?:REA|AGENTS|BACKLOG|ARCHITECTURE)\.md\b|\.\./\.\./\.\./|\bspecifications\b")),
    ("cleanup remnant", re.compile(r"\(`\)|\(full source\)|\( \)")),
]

GATE_EXEMPT = {
    "crates/cupertino-files/src/mime.rs": {"evidence class"},
    "crates/cupertino-theme/src/evidence.rs": {"evidence class"},
}

THIRD_PARTY_EXEMPT = {"private document path", "cleanup remnant"}

def policy_text(text: str) -> str:
    # Policy literals are data; all executable code and comments remain checked.
    lines = text.splitlines(keepends=True)
    for node in ast.parse(text).body:
        if isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id in {"ID", "CLASS", "CACHE_ADDR", "GATE"}
            for target in node.targets
        ):
            for index in range(node.lineno - 1, node.end_lineno):
                lines[index] = "\n"
    return "".join(lines)


def gate(root: Path) -> tuple[Counter, list[str]]:
    hits: Counter = Counter()
    problems: list[str] = []
    for directory, dirs, files in os.walk(root, followlinks=False):
        parent = Path(directory)
        if parent == root:
            dirs[:] = [name for name in dirs if name not in {".git", "target"}]
        for name in sorted(dirs + files):
            path = parent / name
            rel = path.relative_to(root).as_posix()
            if path.is_symlink():
                target = path.resolve(strict=True)
                if Path(os.readlink(path)).is_absolute() or not target.is_relative_to(root):
                    problems.append(f"source symlink escapes tree: {rel}")
                continue
            if path.suffix.lower() in FORBIDDEN_SUFFIXES and rel not in ALLOWED_BINARIES:
                problems.append(f"forbidden file type: {rel}")
                continue
            if not path.is_file():
                continue
            data = path.read_bytes()
            if rel in ALLOWED_BINARIES:
                if hashlib.sha256(data).hexdigest() != ALLOWED_BINARIES[rel]:
                    problems.append(f"allowed binary changed: {rel}")
                continue
            if data[:4] in BINARY_MAGIC:
                problems.append(f"forbidden file type: {rel}")
                continue
            try:
                text = data.decode("utf-8")
            except UnicodeDecodeError:
                # Matches the source policy; this is not a general binary allowlist.
                continue
            if rel == "ci/gate.py":
                text = policy_text(text)
            exempt = GATE_EXEMPT.get(rel, set())
            if path.relative_to(root).parts[0] == "third-party":
                exempt = exempt | THIRD_PARTY_EXEMPT
            for name, pattern in GATE:
                if name in exempt:
                    continue
                for match in pattern.finditer(text):
                    hits[name] += 1
                    line = text.count("\n", 0, match.start()) + 1
                    problems.append(f"{rel}:{line}: {name}: {match.group(0)!r}")
    return hits, problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    if not root.is_dir():
        parser.error("root must be a directory")
    hits, problems = gate(root)
    for problem in problems:
        print(problem, file=sys.stderr)
    print(f"gate: {len(problems)} problems ({dict(hits)})")
    return int(bool(problems))


if __name__ == "__main__":
    sys.exit(main())
