//! The `clippy.toml` of every `shell` and `apps` crate: Clippy's `disallowed-methods`, so that a UI thread has no
//! blocking call within reach and the theme data is read only by the toolkit.

const THREADS: &str =
    "start work through the toolkit's task API, which reports its panics and results";
const BLOCK_ON: &str = "await through the toolkit's task API instead of blocking the UI thread";
const FILES: &str = "run file access as a task of the toolkit's task API, off the UI thread";
const BUS: &str = "use the toolkit's async bus API, ticked by the UI loop";
const THEME_DATA: &str =
    "name a theme colour with the toolkit's theme::system_color, or paint with the theme a widget is given";

/// The methods a `shell` or `apps` crate must not call, with the reason Clippy prints.
pub const DISALLOWED_METHODS: &[(&str, &str)] = &[
    ("std::thread::spawn", THREADS),
    ("std::thread::Builder::spawn", THREADS),
    ("std::thread::Builder::spawn_scoped", THREADS),
    ("std::thread::scope", THREADS),
    ("std::thread::Scope::spawn", THREADS),
    ("async_io::block_on", BLOCK_ON),
    ("futures_executor::block_on", BLOCK_ON),
    ("futures_lite::future::block_on", BLOCK_ON),
    ("pollster::block_on", BLOCK_ON),
    ("tokio::runtime::Handle::block_on", BLOCK_ON),
    ("tokio::runtime::Runtime::block_on", BLOCK_ON),
    ("zbus::block_on", BLOCK_ON),
    ("std::fs::canonicalize", FILES),
    ("std::fs::copy", FILES),
    ("std::fs::create_dir", FILES),
    ("std::fs::create_dir_all", FILES),
    ("std::fs::exists", FILES),
    ("std::fs::hard_link", FILES),
    ("std::fs::metadata", FILES),
    ("std::fs::read", FILES),
    ("std::fs::read_dir", FILES),
    ("std::fs::read_link", FILES),
    ("std::fs::read_to_string", FILES),
    ("std::fs::remove_dir", FILES),
    ("std::fs::remove_dir_all", FILES),
    ("std::fs::remove_file", FILES),
    ("std::fs::rename", FILES),
    ("std::fs::set_permissions", FILES),
    ("std::fs::symlink_metadata", FILES),
    ("std::fs::write", FILES),
    ("std::fs::DirBuilder::create", FILES),
    ("std::fs::File::create", FILES),
    ("std::fs::File::create_new", FILES),
    ("std::fs::File::open", FILES),
    ("std::fs::OpenOptions::open", FILES),
    ("std::fs::DirEntry::metadata", FILES),
    ("std::fs::File::metadata", FILES),
    ("std::path::Path::canonicalize", FILES),
    ("std::path::Path::exists", FILES),
    ("std::path::Path::is_dir", FILES),
    ("std::path::Path::is_file", FILES),
    ("std::path::Path::is_symlink", FILES),
    ("std::path::Path::metadata", FILES),
    ("std::path::Path::read_dir", FILES),
    ("std::path::Path::read_link", FILES),
    ("std::path::Path::symlink_metadata", FILES),
    ("std::path::Path::try_exists", FILES),
    ("zbus::blocking::Connection::call_method", BUS),
    ("zbus::blocking::Connection::session", BUS),
    ("zbus::blocking::Connection::system", BUS),
    ("zbus::blocking::Connection::emit_signal", BUS),
    ("zbus::blocking::Connection::object_server", BUS),
    ("zbus::blocking::Connection::request_name", BUS),
    ("zbus::blocking::Connection::send", BUS),
    ("zbus::blocking::connection::Builder::build", BUS),
    ("zbus::blocking::Proxy::new", BUS),
    ("zbus::blocking::Proxy::call", BUS),
    ("zbus::blocking::Proxy::call_method", BUS),
    ("zbus::blocking::Proxy::call_noreply", BUS),
    ("zbus::blocking::Proxy::get_property", BUS),
    ("zbus::blocking::Proxy::set_property", BUS),
    ("zbus::blocking::Proxy::introspect", BUS),
    ("zbus::blocking::Proxy::receive_signal", BUS),
    ("zbus::blocking::proxy::Builder::build", BUS),
    ("zbus::blocking::MessageIterator::for_match_rule", BUS),
    ("cupertino_assets::theme::load", THEME_DATA),
];

/// The `clippy.toml` that every `shell` and `apps` crate holds next to its manifest, made from `methods`.
pub fn clippy_toml(methods: &[(&str, &str)]) -> String {
    let mut s = String::from(
        "# Written by `cargo run -q -p cupertino-layers -- --write-clippy` from its list; never edit by hand.\n\
         too-many-lines-threshold = 150\n\
         disallowed-methods = [\n",
    );
    for (path, reason) in methods {
        s.push_str(&format!(
            "    {{ path = \"{path}\", reason = \"{reason}\" }},\n"
        ));
    }
    s.push_str("]\n");
    s
}
