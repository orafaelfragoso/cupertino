// The wayland-scanner macros read the XML at expansion time, which cargo does not track: declare the
// protocol directory so any edited or added protocol regenerates the bindings.
fn main() {
    println!("cargo:rerun-if-changed=protocols");
}
