//! The shared library's soname, and `-z nodelete`: Rust thread-local destructors that ran after a `dlclose` would crash
//! the host, so the library is never unloaded. rustc's own version script keeps every symbol but the `ck_*` exports
//! local; there are no symbol version nodes in v1.

fn main() {
    println!("cargo:rustc-cdylib-link-arg=-Wl,-soname,libcupertinokit.so.1");
    println!("cargo:rustc-cdylib-link-arg=-Wl,-z,nodelete");
}
