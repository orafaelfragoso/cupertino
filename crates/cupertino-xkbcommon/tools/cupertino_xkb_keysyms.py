#!/usr/bin/env python3
"""Add the keysyms of a newer libxkbcommon to the vendored xkbcommon-rs name tables.

The port generates `keysyms_generated_phf.rs` from the keysym constants of the `xkeysym` crate, which stop at
libxkbcommon 1.7. This script reads `xkbcommon-keysyms.h` of a current libxkbcommon and inserts every name the
tables lack, in the order the lookups rely on:

- `NAME_TO_KEYSYM`: `NoSymbol` first, then sorted case-insensitively by name (the case-insensitive lookup is a binary
  search);
- `KEYSYM_TO_NAME`: sorted by value, a new alias after the names the table already has for that value.

A name in both with a different value is an error. Running it twice changes nothing.

usage: cupertino_xkb_keysyms.py HEADER TABLES.rs LIBXKBCOMMON_VERSION
"""

import re
import sys

DEFINE = re.compile(r"^#define\s+XKB_KEY_(\w+)\s+(0x[0-9a-fA-F]+)", re.MULTILINE)
NAME_ENTRY = re.compile(r'^\s+"(\w+)" => (.+),$')
VALUE_ENTRY = re.compile(r'\(\s*([^"]+?),\s*"(\w+)",?\s*\)')
NEW_ENTRY = re.compile(r"Keysym::new\((0x[0-9a-f]+)\)")
SOURCE_NOTE = "// Names added from libxkbcommon "


def main(header_path: str, tables_path: str, version: str) -> None:
    header = {name: int(value, 16) for name, value in DEFINE.findall(open(header_path).read())}
    lines = open(tables_path).read().split("\n")

    map_start = next(i for i, l in enumerate(lines) if l.startswith("pub(crate) static NAME_TO_KEYSYM"))
    map_end = next(i for i in range(map_start, len(lines)) if lines[i] == "};")
    array_start = next(i for i, l in enumerate(lines) if l.startswith("pub(crate) static KEYSYM_TO_NAME"))
    array_end = next(i for i in range(array_start, len(lines)) if lines[i] == "];")

    names = [NAME_ENTRY.match(l).groups() for l in lines[map_start + 1 : map_end] if l.strip()]
    pairs = [(name, expr) for expr, name in VALUE_ENTRY.findall("\n".join(lines[array_start + 1 : array_end]))]

    known = {name for name, _ in names}
    absent = [name for name, _ in pairs if name not in header]
    if absent:
        sys.exit(f"the header lacks table names: {absent[:10]}")
    for name, expr in names:
        added = NEW_ENTRY.fullmatch(expr)
        if added and int(added.group(1), 16) != header[name]:
            sys.exit(f"{name}: table has {expr}, header has {header[name]:#x}")
    new = {name: value for name, value in header.items() if name not in known}
    if not new:
        print("the tables already hold every header name")
        return

    added_names = [(name, f"Keysym::new({value:#x})") for name, value in new.items()]
    sorted_names = names[:1] + sorted(names[1:] + added_names, key=lambda row: (row[0].lower(), row[0]))
    sorted_pairs = sorted(pairs + [(name, expr) for name, expr in added_names], key=lambda row: header.get(row[0], 0))

    out = [l for l in lines[:map_start] if not l.startswith(SOURCE_NOTE)]
    out.append(f"{SOURCE_NOTE}{version} (xkbcommon-keysyms.h) by tools/cupertino_xkb_keysyms.py.")
    out.append(lines[map_start])
    out += [f'        "{name}" => {expr},' for name, expr in sorted_names]
    out += lines[map_end:array_start]
    out.append(re.sub(r"; \d+\]", f"; {len(sorted_pairs)}]", lines[array_start]))
    out += [f'    ({expr}, "{name}"),' for name, expr in sorted_pairs]
    out += lines[array_end:]
    open(tables_path, "w").write("\n".join(out))
    print(f"added {len(new)} names to the tables: {', '.join(sorted(new))}")


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    main(*sys.argv[1:])
