//! Generated bindings of the public protocols `protocols/cupertino-app-effects-v1.xml` and
//! `protocols/cupertino-scroll-v1.xml`: enable `client` in clients and `server` in the display server.

#![allow(clippy::all)]

// The scanner macros read the XML without telling rustc; including it puts it in the dep-info that caches such as
// sccache key on, so an edited protocol regenerates the bindings.
const _: &[u8] = include_bytes!("../protocols/cupertino-app-effects-v1.xml");
const _: &[u8] = include_bytes!("../protocols/cupertino-scroll-v1.xml");
const _: &[u8] = include_bytes!("../protocols/cupertino-semantics-v1.xml");

#[cfg(feature = "client")]
pub mod client {
    #![allow(
        dead_code,
        non_camel_case_types,
        unused_unsafe,
        unused_variables,
        non_upper_case_globals,
        non_snake_case,
        unused_imports,
        missing_docs
    )]

    /// Native surface-owned semantic publications.
    pub mod semantics {
        use wayland_client;
        use wayland_client::protocol::*;
        pub mod __interfaces {
            use wayland_client::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("protocols/cupertino-semantics-v1.xml");
        }
        use self::__interfaces::*;
        wayland_scanner::generate_client_code!("protocols/cupertino-semantics-v1.xml");
    }

    /// Bindings of `protocols/cupertino-scroll-v1.xml`.
    pub mod scroll {
        use wayland_client;
        use wayland_client::protocol::*;

        pub mod __interfaces {
            use wayland_client::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("protocols/cupertino-scroll-v1.xml");
        }
        use self::__interfaces::*;

        wayland_scanner::generate_client_code!("protocols/cupertino-scroll-v1.xml");
    }

    /// Bindings of `protocols/cupertino-app-effects-v1.xml`.
    pub mod app_effects {
        use wayland_client;
        use wayland_client::protocol::*;

        pub mod __interfaces {
            use wayland_client::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("protocols/cupertino-app-effects-v1.xml");
        }
        use self::__interfaces::*;

        wayland_scanner::generate_client_code!("protocols/cupertino-app-effects-v1.xml");
    }
}

#[cfg(feature = "server")]
pub mod server {
    #![allow(
        dead_code,
        non_camel_case_types,
        unused_unsafe,
        unused_variables,
        non_upper_case_globals,
        non_snake_case,
        unused_imports,
        missing_docs
    )]

    /// Native surface-owned semantic publications.
    pub mod semantics {
        use wayland_server;
        use wayland_server::protocol::*;
        pub mod __interfaces {
            use wayland_server::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("protocols/cupertino-semantics-v1.xml");
        }
        use self::__interfaces::*;
        wayland_scanner::generate_server_code!("protocols/cupertino-semantics-v1.xml");
    }

    /// Bindings of `protocols/cupertino-scroll-v1.xml`.
    pub mod scroll {
        use wayland_server;
        use wayland_server::protocol::*;

        pub mod __interfaces {
            use wayland_server::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("protocols/cupertino-scroll-v1.xml");
        }
        use self::__interfaces::*;

        wayland_scanner::generate_server_code!("protocols/cupertino-scroll-v1.xml");
    }

    /// Bindings of `protocols/cupertino-app-effects-v1.xml`.
    pub mod app_effects {
        use wayland_server;
        use wayland_server::protocol::*;

        pub mod __interfaces {
            use wayland_server::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("protocols/cupertino-app-effects-v1.xml");
        }
        use self::__interfaces::*;

        wayland_scanner::generate_server_code!("protocols/cupertino-app-effects-v1.xml");
    }
}
