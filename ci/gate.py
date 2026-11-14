#!/usr/bin/env python3
"""Check a source tree. Usage: python3 ci/gate.py ROOT."""
import argparse
import ast
import functools
import hashlib
import os
from pathlib import Path
import re
import subprocess
import sys
from collections import Counter

FORBIDDEN_SUFFIXES = {".ttf", ".otf", ".ttc"}

ALLOWED_BINARIES = {
    "crates/cupertino-assets/fonts/InterVariable-subset.ttf": "8082ab1587b6b1a0ee14e44287b204f3aae7f2021583ca76beb1f717a093dcea",
    # our own boxes for the fallback tests, written by make-fallback-font.py beside it
    "crates/cupertino-ui-text/tests/fixtures/fallback-test.ttf": "73b6c3d00132c6a20b18be6f8a3e13340e2e03c482544e30a5793ad6ded3e476",
    # our own colour-layer boxes for the emoji tests, written by make-colour-font.py beside it
    "crates/cupertino-ui-text/tests/fixtures/colour-test.ttf": "466f5457b35e2f111ab8384e3bf71cae279600f0e3f0faaef760cb43af95137a",
    # Noto Sans subsets (OFL-1.1, OFL.txt beside them) for the script tests, written by make-script-fonts.py beside them
    "crates/cupertino-ui-text/tests/fixtures/noto-arabic-subset.ttf": "c6da012fdf8f32e7cc5bbf8e5e359bce4a91df29e3c8649e44c8db4a492fa99e",
    "crates/cupertino-ui-text/tests/fixtures/noto-devanagari-subset.ttf": "d92eacd722dbe467ef8a0cf5d667a273cc1b3badfb1f1781c0d6c7ea48de8eea",
    "crates/cupertino-ui-text/tests/fixtures/noto-hebrew-subset.ttf": "c8aeb1cd9d81fb2c291c8edcf8f65f76e2baefd97dd77adfc0a2d49881877f62",
}


ID =r"\b[A-Z]{2,5}-(?:\d{3}[a-z]?|nnn|xxx)\b"

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

# Listed terms no file may hold, kept as sha256 digests of their text so that this file does not spell them out.
# Product names: exact case, one to three words joined by single spaces.
LISTED_NAMES = {
    "08c9188c5d641e944ca3c674f027a1c0186d00f69d1079c8ba1417aa6bfa2d51",
    "1a9a658b5d023f73ec331216cf4069a4eeb9e4e85c1ccc6587e3c4ab5455cb64",
    "23cbcfc3e80a6fe5d4f319b08c0921d2b979f2db8ecb178dec4fbff034485b1c",
    "257c97330f448883b45b1053e0c4d956d12df58cbb2597314356a2ba3e70ac42",
    "29f87af4fb46de9584d7ff40f33d2d4713821c2031520cf08a7652d08c400c45",
    "2d05f5afece7167c0d91c06ab1a4abd7487a303e8f1901ded7bf5ef91f4dfa09",
    "355f5e74848522db142812706d3559c35daf9f66536507b4f5bf7ad3e1f68575",
    "38fdf519314e3151d7e7f6ef456f327b78ddb84bc457bdb0d49bce0b1fc3c959",
    "3a0e2e624aa20ff2159ebbff57a527546a99e427a2d3bad774c6313613253b8a",
    "421dac2ceec30e00569df2509acb0237b0a84e7ddf8e9b52d989f70b6fcbfafa",
    "527859c0ad13e24ca8c9b65edd36de55aaf1b66a5c4a1419865a5e77a9447184",
    "5ab413d93c4a9ab5e8e95974f09e475e48a6dcc8497d1d332afc139b98a9f1a1",
    "5f0330392292a13a29ae269a1efe6f7c2a8210572d884d8c2392a9105663a41b",
    "7a4721a879cb49bf793cbf62a831c2e70ae16da6a6a56fc83fe0dcdccaacee97",
    "7cdcbd87852f9d125c8d7f71559429c15d185d17c9df4eb3b80582d33383bb63",
    "815eca977c7d75f72bd23bccc91bc6f9931a42fbf03451eb0d7db8d2a704d94f",
    "8f5302cbdf238881c06b53a87ef86fee7e536559f174a9972bee01a461e46ec7",
    "a7b5855eb126881212efa2f6e52dc15003f34229f1e9ee3660f119f0f8cab27a",
    "aca3c1364979e0358fdb430f51ca6796d352c139c76ca40858f75fec58016e8f",
    "aed6b7aa2a0511a9bbcaae2a10127a3139fc6494c57769b31b1a694c14483ce7",
    "b05d344f153ca91b245cd82f943f071e51a4529103e0b5d0277f162e89e9d32e",
    "b4bbd684e4c8babe32714f13b91135586575b2947fef65b348ce354df1d69619",
    "b81bbeeb7ed2ce377642c0daa3e315f89f41f96779b080324f9d3a06ea7b9fb5",
    "ba40ff241db36ac39b47c1f9d338a9a48909deb1b4dc2af4eb7ab91522e9fb6a",
    "bf5521de93439c3b5613f4b87d60e9ff4596a660768a241b5685c1673f5a1dd3",
    "c8bdc6fa86fc7a67deaf0fc456e609ada8511f9f598ac893b4c2166356e0fecc",
    "cdd89742cf8b5d378b96dc7cfa363c6cc62334a9c7d52720dd7bad1f0bdfe77d",
    "cff8ef95912352a3146bef4b8f5dd2d81c16393b2dbede6fcbe2eba242d68efe",
    "e42df0232d77711b5578526bb9b952da23ba9a8c046a39309e68628b17058a06",
    "e68bcce7f0f46cd769630b1725c90a41b7c7e3de27012354d2f52401c82dfaa3",
    "ecf4bed079f79dd889594a919c224ca535d491edc7a4f019f842e270f7b2ed46",
    "f06783f44c0c4d216d786235877fa9c174e63a71e7a8c57d0b7c2ae733d8afff",
    "f09f6d1442e8399c655171906e8da403098d305484a085b70d35c62f93dc73ee",
    "fca7aa83d276c8bd69f53641339b55cfd8846130d20011a7eb8ec924cb59e655",
    "fdbdc46d32fef7e79d43fd88a71521d741b74170cce38e296a76825a1ad653bc",
}
# A product name at the start of an identifier, in any case (lowercased before hashing), unless a lowercase letter
# follows: it counts in `name_cursor` and `NameCursor`, not inside a longer lowercase word.
LISTED_NAME_HEADS = {
    "3a7bd3e2360a3d29eea436fcfb7e44c735d117c42d1c1835420b6b9942dd4f1b",
}
LISTED_NAME_HEAD_LENGTHS = (5,)
# Provenance words, whole words in any case (lowercased before hashing).
LISTED_WORDS = {
    "25eb6591c84eb015825dd839dc9d2916147ddc145f6c4346717b33ec015e0246",
    "4f361b43bd568fc2f1b9974cb3882464277e62865a733d8da86c5637051e7eb4",
    "59a2ea36c20b30a30f3aa251b346188d5afbeddfca2d496e67b0c2d9eb74a345",
    "74371f0025ee1a6211a4c8f277448020c81faa771ca0589a7819d8adc3f70b95",
    "c2179a0a39c15793be3827210219e3fc066df9ea8b89a62c8f686839ee4a507b",
}
# Provenance word beginnings in any case (lowercased before hashing).
LISTED_WORD_HEADS = {
    "2b3d4ca3d1124eceded88275ec7af1f75486c233e617c48f99297f0ee8f4e6fc",
    "c521018fc36a56f8a64df42af0df52a50a6e3c13e44edc570c3f662f1db3ee35",
}
LISTED_WORD_HEAD_LENGTHS = (8, 10)
# Analysis phrases: two words in any case joined by a space or a hyphen (lowercased, joined by a space, before hashing).
LISTED_PHRASES = {
    "042ed30a47d9c5cbf634c046ee2e3f4ca0d6332e203ff2566f3f7ccd6568a48d",
    "07207cfadff51b649932bf438c4467e3b6ef6600376dd23bce52d60a16ce2341",
    "290c916689affd6a373c8e16f4b31f781e68a3c20e9420f2a367c64c4a53b51e",
    "2fcd0d1d332dbe5fc8af394e51cb57ccc9a16c50a9672eb1b5e64e97dde80665",
    "7221ba7689f6a52fdcef47830b86f6c78980d34fe8b93f47a573bb3c1be812c7",
    "7d2c5e2ba439501247242285b0039b1c9fa3e141276a68e9aaa1e508d1915ead",
    "9aed1062091007bb3f090bddacc9482d4ee80c0cd3cd6a500f933ec4046e5e8e",
    "d9cbe6ab94ebd22cc69b0d86bc72f90a525253d13b81a69866e7532a9b1fd074",
    "df0cf82b19f44c150203827d039c912c37806c28fefc2e42f3cabf2252b0209e",
    "e84ea07273cd05ac5961e788f723b6a7b9c80a58c5ac6e7d859269a6b7ca6ebc",
}
# File suffixes (lowercase, with the dot) and first four bytes of files that may not be committed.
LISTED_SUFFIXES = {
    "52d170b579a69cd6c7a742ed7d9c71cbda30eaef1d8dfcb97324e0d67688be1d",
    "62ccfe9faa79e5aaa913d52b93fbae306ad73f91c5f6a3de63365fba5f943d3d",
    "67b38016c0e503110fd437753c683dd74fdaedff2ebcb464d9be6a1fa64d081f",
    "6e7a0da7ec8955b9e470c7121019538d40050af8624c346c8a59a0cea86bad29",
    "7ac82063e983328f696d356278a5553585e08127368f395bc741eea7f36876bd",
    "94644341556428956155e7f5073cf8d95003c54b03fbfe0fdf6cbdde34fd3ea8",
    "9b7c5ca4f780888a81a4933325c025f1b8363c5fbac1db484c96e60be6d69b56",
    "c6192a7fc9fd706744f38e2ab21e37cad9bbe83f77bf0d55872878ba70c41ed1",
}
LISTED_MAGIC = {
    "1c67b14f68145a0e93cb09fdf8c79365f0e33b28ce0c1a58f19b6ac99659a399",
    "3684d4581255ca55e94c2cb89affc7d0dc914b7462c1f496780d7f2214877709",
    "65ab12a8ff3263fbc257e5ddf0aa563c64573d0bab1f1115b9b107834cfa6971",
    "67a524103ffac2fcb99209bc8e3b7c3ee4ad27980975e322967bfc01eb2e6d1f",
    "d2238b6d7c34ad608a5f6caddf95928d741307fea41f0336302e390109311d72",
}

WORD = re.compile(r"[^\W_]+")
NAME_HEAD = {length: re.compile(rf"(?<![A-Za-z0-9])[A-Za-z]{{{length}}}(?![a-z])") for length in LISTED_NAME_HEAD_LENGTHS}


@functools.lru_cache(maxsize=None)
def digest(term: str | bytes) -> str:
    return hashlib.sha256(term.encode() if isinstance(term, str) else term).hexdigest()


def listed_matches(text: str) -> list[tuple[str, int, str]]:
    """(rule name, offset, matched text) of every listed term in `text`."""
    found = []
    words = list(WORD.finditer(text))
    for index, word in enumerate(words):
        lower = word.group(0).lower()
        if digest(lower) in LISTED_WORDS or any(
            len(lower) >= length and digest(lower[:length]) in LISTED_WORD_HEADS for length in LISTED_WORD_HEAD_LENGTHS
        ):
            found.append(("provenance wording", word.start(), word.group(0)))
        if index + 1 < len(words) and text[word.end() : words[index + 1].start()] in (" ", "-"):
            if digest(f"{lower} {words[index + 1].group(0).lower()}") in LISTED_PHRASES:
                found.append(("analysis wording", word.start(), text[word.start() : words[index + 1].end()]))
        end = index
        while end < len(words) and end - index < 3:
            if end > index and text[words[end - 1].end() : words[end].start()] != " ":
                break
            phrase = text[word.start() : words[end].end()]
            if digest(phrase) in LISTED_NAMES:
                found.append(("vendor or product name", word.start(), phrase))
                break
            end += 1
    for pattern in NAME_HEAD.values():
        for match in pattern.finditer(text):
            if digest(match.group(0).lower()) in LISTED_NAME_HEADS:
                found.append(("vendor or product name", match.start(), match.group(0)))
    return found

GATE_EXEMPT = {
    "crates/cupertino-files/src/mime.rs": {"evidence class"},
    "crates/cupertino-theme/src/evidence.rs": {"evidence class"},
    # The vendored smithay release, unchanged: an EGL error code and comments about Rust references.
    "third-party/smithay/src/backend/egl/ffi.rs": {"evidence class"},
    "third-party/smithay/src/backend/renderer/element/utils/elements.rs": {"provenance wording"},
    "third-party/smithay/src/desktop/space/utils.rs": {"provenance wording"},
    "third-party/smithay/src/input/keyboard/mod.rs": {"provenance wording"},
    "third-party/smithay/src/wayland/shm/mod.rs": {"provenance wording"},
}

# Third-party copyright holders whose names trip a rule; the literal is blanked before matching, the rest of the file is checked.
GATE_HOLDERS = {
    # The name is split so that this file passes the gate itself.
    "packaging/licenses/THIRD-PARTY-LICENSES": ("Status " + "Re" + "search & Development GmbH",),
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


def git_visible_paths(root: Path) -> set[str] | None:
    """The paths git would track under `root`: tracked plus untracked-not-ignored. None outside a repository."""
    try:
        result = subprocess.run(
            ["git", "-C", str(root), "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return None
    return {path for path in result.stdout.split("\0") if path}


def gate(root: Path) -> tuple[Counter, list[str]]:
    hits: Counter = Counter()
    problems: list[str] = []
    # Build output is gitignored: scanning it would flag strings in generated assets. Outside a repository
    # (the tests, an unpacked archive) every file is visible.
    visible = git_visible_paths(root)
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
            if visible is not None and rel not in visible:
                continue
            suffix = path.suffix.lower()
            if (suffix in FORBIDDEN_SUFFIXES or digest(suffix) in LISTED_SUFFIXES) and rel not in ALLOWED_BINARIES:
                problems.append(f"forbidden file type: {rel}")
                continue
            if not path.is_file():
                continue
            data = path.read_bytes()
            if rel in ALLOWED_BINARIES:
                if hashlib.sha256(data).hexdigest() != ALLOWED_BINARIES[rel]:
                    problems.append(f"allowed binary changed: {rel}")
                continue
            if digest(data[:4]) in LISTED_MAGIC:
                problems.append(f"forbidden file type: {rel}")
                continue
            try:
                text = data.decode("utf-8")
            except UnicodeDecodeError:
                # Matches the source policy; this is not a general binary allowlist.
                continue
            if rel == "ci/gate.py":
                text = policy_text(text)
            for holder in GATE_HOLDERS.get(rel, ()):
                text = text.replace(holder, " " * len(holder))
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
            for name, start, matched in listed_matches(text):
                if name in exempt:
                    continue
                hits[name] += 1
                line = text.count("\n", 0, start) + 1
                problems.append(f"{rel}:{line}: {name}: {matched!r}")
    return hits, problems


SCANNER_XML = re.compile(r'\bgenerate_(?:interfaces|client_code|server_code)!\s*\(\s*"([^"]+)"')
INCLUDED = re.compile(r'\binclude_bytes!\s*\(\s*"([^"]+)"')


def protocol_xml_problems(root: Path) -> list[str]:
    """Library sources whose wayland-scanner XML is not also `include_bytes!`-ed in the crate: the macros read it
    without telling rustc, so a build cache keyed on rustc's dep-info would keep stale bindings after an edit."""
    problems: list[str] = []
    for crate in sorted(path.parent for path in root.glob("crates/*/Cargo.toml")):
        src = crate / "src"
        sources = [path for path in sorted(src.rglob("*.rs")) if path != src / "main.rs" and src / "bin" not in path.parents]
        named: list[tuple[Path, int, str]] = []
        included: set[str] = set()
        for path in sources:
            text = path.read_text(encoding="utf-8")
            for match in SCANNER_XML.finditer(text):
                named.append((path, text.count("\n", 0, match.start()) + 1, os.path.normpath(match.group(1))))
            for match in INCLUDED.finditer(text):
                included.add(os.path.relpath(os.path.normpath(path.parent / match.group(1)), crate))
        for path, line, xml in named:
            if xml not in included:
                rel = path.relative_to(root).as_posix()
                problems.append(f"{rel}:{line}: protocol XML read by wayland-scanner is not include_bytes!-ed in its crate: {xml}")
    return problems


def metadata_problems(root: Path) -> list[str]:
    release = root / "packaging" / "release.py"
    if not release.is_file():
        return []
    result = subprocess.run([sys.executable, "-I", str(release), "check-metadata"], cwd=root, capture_output=True, text=True, timeout=300)
    if result.returncode == 0:
        return []
    lines = [line for line in (result.stderr or result.stdout).splitlines() if line.strip()]
    if not lines:
        return [f"packaging: check-metadata: exit {result.returncode} with no output"]
    return [f"packaging: check-metadata: {line}" for line in lines]


def contracts_problems(root: Path) -> list[str]:
    checker = root / "ci" / "contracts.py"
    if not checker.is_file():
        return []
    result = subprocess.run([sys.executable, "-I", str(checker), str(root)], cwd=root, capture_output=True, text=True, timeout=300)
    if result.returncode == 0:
        return []
    lines = [line for line in (result.stderr or result.stdout).splitlines() if line.strip()]
    if not lines:
        return [f"contracts: exit {result.returncode} with no output"]
    return [f"contracts: {line}" for line in lines]


# Crates whose lockfile version is held exactly: name, the one version allowed, and why.
PINNED_LOCK_VERSIONS = {
    # 1.7.0 keeps an idle pool thread for good, waking it twice a second in every process with a bus connection.
    "blocking": "1.6.2",
}


def lockfile_problems(root: Path) -> list[str]:
    """`Cargo.lock` entries of a pinned crate at any version other than the pinned one."""
    lock = root / "Cargo.lock"
    if not lock.is_file():
        return []
    problems: list[str] = []
    name = None
    for number, line in enumerate(lock.read_text(encoding="utf-8").splitlines(), 1):
        if line == "[[package]]":
            name = None
        elif line.startswith("name = "):
            name = line.removeprefix("name = ").strip('"')
        elif line.startswith("version = ") and name in PINNED_LOCK_VERSIONS:
            version = line.removeprefix("version = ").strip('"')
            if version != PINNED_LOCK_VERSIONS[name]:
                problems.append(f"Cargo.lock:{number}: {name} {version} is locked; only {PINNED_LOCK_VERSIONS[name]} is allowed")
    return problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    if not root.is_dir():
        parser.error("root must be a directory")
    hits, problems = gate(root)
    problems += protocol_xml_problems(root)
    problems += lockfile_problems(root)
    problems += metadata_problems(root)
    problems += contracts_problems(root)
    for problem in problems:
        print(problem, file=sys.stderr)
    print(f"gate: {len(problems)} problems ({dict(hits)})")
    return int(bool(problems))


if __name__ == "__main__":
    sys.exit(main())
