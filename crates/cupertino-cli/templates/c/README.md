# {{name}}

A CupertinoKit application in C, started with `cupertino sdk new --lang c {{name}}`.

It builds against the installed C SDK, the `cupertino-kit` package, and takes every path from
`pkg-config cupertinokit`:

```sh
make
./{{name}}
```

`cupertino sdk info` shows what is installed, and `cupertino sdk check` builds and runs a small program against it.
The rules of the C ABI (versions, handles, threads, errors) are at the top of
`/usr/include/cupertinokit-1/cupertinokit.h`.

`{{name}}.desktop` is the application's desktop entry; its file name is the app id the program passes to
`ck_runtime_new`. Install it with the program to show the application in the desktop's launcher.
