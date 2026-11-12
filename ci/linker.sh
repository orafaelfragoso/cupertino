#!/bin/sh
# The C compiler driver Rust links with: through mold when it is installed, which links the large test and program
# binaries several times faster; otherwise the system linker.
m=$(command -v mold 2>/dev/null)
[ -z "$m" ] && [ -x "$HOME/.local/bin/mold" ] && m="$HOME/.local/bin/mold"
[ -n "$m" ] && exec cc -B"$(dirname "$m")" -fuse-ld=mold "$@"
exec cc "$@"
