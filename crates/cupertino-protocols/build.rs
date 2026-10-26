// The wayland-scanner macros read the XML at expansion time, which cargo does not track: declare it so
// an edited protocol regenerates the bindings.
fn main() {
    println!("cargo:rerun-if-changed=protocols/cupertino-shell-v1.xml");
    println!("cargo:rerun-if-changed=protocols/cupertino-screenshot-v1.xml");
    println!("cargo:rerun-if-changed=protocols/cupertino-scroll-v1.xml");
}
