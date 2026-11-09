# cupertino-ui-abi

CupertinoKit's C ABI, version 1.0, built as `libcupertinokit.so` with the soname `libcupertinokit.so.1`. The header is
`abi/cupertinokit.h`. MPL-2.0.

## Versions

Functions carry no symbol versions. rustc's own version script makes every symbol except the `ck_*` exports local, and
GNU ld refuses a second, named script next to it. So the version check happens at run time:

- every application passes the minor version it needs as `abi_minor` to `ck_runtime_new`, and a library older than
  that returns `CK_ERROR_VERSION`;
- before the first call of a function added after 1.0, an application checks that minor version or
  `ck_has_capability`. With lazy binding, a function the installed library lacks fails when it is called, not when
  the application loads.

The library is linked with `-z nodelete`, because Rust thread-local destructors that ran after a `dlclose` would crash
the host.

## Rules of the ABI

- Every function returns a `ck_status` and writes its out-parameters only on `CK_OK`. The calling thread's last error
  message is read with `ck_last_error_message`, which NUL-terminates its copy and cuts it at a UTF-8 boundary.
- Handles are 64-bit values: a kind, a slot and a generation. A zero, released or foreign handle returns
  `CK_ERROR_STALE_HANDLE`, so a handle is never dereferenced.
- Input structs start with `struct_size`. The library copies `min(struct_size, sizeof)` bytes into a zeroed struct of
  its own, at any alignment, and refuses a size below 1.0's or above its own. Input structs have no `bool`, enum or
  `char` field; an unknown enum value returns `CK_ERROR_INVALID_ARGUMENT`.
- Strings are `CkStr`, UTF-8 copied during the call. An id or a key may not hold a NUL or be over 1 MiB.
- Output goes into the caller's buffers through raw pointers. `ck_poll_events` refuses overlapping buffers and clamps
  the byte capacity to `u32::MAX`. The library never hands out memory, so nothing needs to be freed.
- Every function body runs in `catch_unwind`. A panic returns `CK_ERROR_INTERNAL` and poisons the runtime: every later
  call on it returns that too, except `ck_runtime_release`. A panic payload whose destructor panics is forgotten, and a
  runtime whose destructor panics is leaked. The crate's `rust-version` is 1.81, from which a panic that escapes an
  `extern "C"` function aborts.

## Threads

There is one runtime per process. It is not `Send`, so it lives in a thread-local of the thread that created it, and
every other function on it returns `CK_ERROR_WRONG_THREAD` from another thread. The process-wide registry holds only the
runtime's generation and owner, its posted messages and its wake fd.

- `ck_waker_post` may be called from any thread. It queues a `CK_EVENT_MESSAGE`, up to 1024, then
  `CK_ERROR_QUEUE_FULL`, and writes the wake fd. Both happen under the registry lock, which closing the wake fd also
  takes, so a post never writes to a closed or reused fd.
- If the owner thread exits without `ck_runtime_release`, its thread-local destructor drops the runtime inside
  `catch_unwind`, leaking it on a panic, and frees the slot. Posts then return `CK_ERROR_STALE_HANDLE`, and a new
  runtime can start.

## The installed SDK

The `cupertino-kit` package (`packaging/release.py`'s `sdk` manifest) installs:

| File | What it is |
|---|---|
| `/usr/lib/libcupertinokit.so.1.<minor>.0` | the library; `<minor>` is `CUPERTINOKIT_ABI_MINOR` of its header |
| `/usr/lib/libcupertinokit.so.1` | the soname, a link to the library |
| `/usr/lib/libcupertinokit.so` | the name a program links with (`-lcupertinokit`), a link to the soname |
| `/usr/include/cupertinokit-1/cupertinokit.h` | `abi/cupertinokit.h`, byte for byte |
| `/usr/lib/pkgconfig/cupertinokit.pc` | from `cupertinokit.pc.in`, with `Version: 1.<minor>.0` |
| `/usr/share/cupertino-kit/build-info.json` | the package, its version and the commit it was built from |

A new major version would get a new soname and include directory, so both can be installed at once. A program builds
with `pkg-config cupertinokit` alone and needs no path of its own:

```sh
cc -std=c11 $(pkg-config --cflags cupertinokit) app.c -o app $(pkg-config --libs cupertinokit)
```

`release.py verify --package sdk` reads the packaged library with `readelf`. It fails unless the exports equal
`abi/symbols-1.txt`, no symbol version is defined, the soname is `libcupertinokit.so.1` and `FLAGS_1` has `NODELETE`, and
unless the header and every other copied file equal the checkout's.

## C samples

- `examples/c/headless.c` negotiates the version (a minor version newer than the library's gives `CK_ERROR_VERSION` and
  a message naming it), asks for capabilities, starts a headless runtime, posts two messages from another thread through
  a waker and polls them, and shows the thread and stale-handle rules. It prints the trace in
  `examples/c/headless.expected` and exits 0. It compiles as C11 and as C++17. Until the runtime has a pollable fd,
  it polls `ck_poll_events` in steps of 1 ms.
- `examples/c/Makefile` builds the samples with `pkg-config cupertinokit` only.
- `tests/c/layout.c` prints every struct's size, alignment and field offsets as the C compiler sees them through the
  installed header, in the format of `abi/layout-1.txt`.

`ci/c-sdk.py` installs the SDK from a build into a temporary root, compiles the samples there with
`-Wall -Wextra -Werror` (also as C++), runs them against the trace, diffs the layout probe with `abi/layout-1.txt`, and
builds and runs the project `cupertino sdk new --lang c` writes. The package's install test does the same against
`/usr` in a fresh container, with the `cupertino-kit` package installed alone.

## Tests

- `src/tests.rs`: the hostile inputs, the thread rules, the post queue and injected panics, calling the functions
  directly.
- `tests/symbols.rs` builds the library and reads it with the `object` crate. It checks that the exports equal
  `abi/symbols-1.txt` and are all `ck_*`, that no symbol version node is defined, the soname, and `DF_1_NODELETE`.
- `tests/header.rs` regenerates the header with cbindgen. `tests/layout.rs` compares every struct's size, alignment and
  field offsets with `abi/layout-1.txt`.

`CUPERTINO_UPDATE_GOLDENS=1` rewrites the three files under `abi/`. Review the diff: a removed or changed line breaks
ABI 1.

## Known limitations

- Only the headless backend exists. `CK_BACKEND_WAYLAND` returns `CK_ERROR_UNSUPPORTED`.
- There are no windows, views or UI events yet. `ck_poll_events` returns only posted messages, and nothing waits on the
  wake fd until `ck_runtime_dispatch` exists; the samples poll in steps instead.
