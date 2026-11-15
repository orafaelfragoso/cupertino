#!/usr/bin/env python3
"""Whether this machine has a GPU the render-node tests can run on.

    ci/render_node.py           print the first render node that opens and exit 0, or exit 1 when there is none
    ci/render_node.py --export  print the shell line that sets CUPERTINO_GPU_REQUIRED=1 when there is one (and the
                                caller's environment does not set it), and say on stderr what the GPU tests will do

scripts/test --slow and scripts/done set CUPERTINO_GPU_REQUIRED=1 when there is one, so the tests that need a render
node fail instead of skipping there. A forced software renderer (LIBGL_ALWAYS_SOFTWARE) counts as none, as the tests
then draw on it and the GPU tier refuses a software device.
"""
import os
import pathlib
import sys

REQUIRED = "CUPERTINO_GPU_REQUIRED"


def usable(root: pathlib.Path = pathlib.Path("/dev/dri"), env=None):
    """The first render node under `root` this process can open for reading and writing, or None."""
    env = os.environ if env is None else env
    if env.get("LIBGL_ALWAYS_SOFTWARE"):
        return None
    try:
        nodes = sorted(p for p in root.iterdir() if p.name.startswith("renderD"))
    except OSError:
        return None
    for node in nodes:
        try:
            fd = os.open(node, os.O_RDWR | os.O_CLOEXEC)
        except OSError:
            continue
        os.close(fd)
        return node
    return None


def require(env, root: pathlib.Path = pathlib.Path("/dev/dri")) -> str:
    """Set CUPERTINO_GPU_REQUIRED=1 in `env` when the machine has a usable render node and the caller did not set it
    already; the line that says what the GPU tests will do."""
    if env.get(REQUIRED) is not None:
        return f"GPU tests: {REQUIRED}={env[REQUIRED]} as given"
    node = usable(root, env)
    if node is None:
        return "GPU tests: no usable render node, so the tests that need one skip"
    env[REQUIRED] = "1"
    return f"GPU tests: render node {node} opens, so {REQUIRED}=1 and the tests that need one cannot skip"


def main(args) -> int:
    if args == ["--export"]:
        env = dict(os.environ)
        print(require(env), file=sys.stderr)
        if REQUIRED not in os.environ and REQUIRED in env:
            print(f"export {REQUIRED}={env[REQUIRED]}")
        return 0
    if args:
        print(__doc__, file=sys.stderr)
        return 2
    found = usable()
    if found is None:
        return 1
    print(found)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
