//! Generated bindings of the private `protocols/cupertino-shell-v1.xml` and `protocols/cupertino-screenshot-v1.xml`: enable
//! `client` in shell processes and `server` in the display server. The public protocols are `cupertino-app-protocols`,
//! re-exported here as `app_effects` and `scroll`.

#![allow(clippy::all)]

// The scanner macros read the XML without telling rustc; including it puts it in the dep-info that caches such as
// sccache key on, so an edited protocol regenerates the bindings.
const _: &[u8] = include_bytes!("../protocols/cupertino-shell-v1.xml");
const _: &[u8] = include_bytes!("../protocols/cupertino-screenshot-v1.xml");
const _: &[u8] = include_bytes!("../protocols/cupertino-control-v1.xml");

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
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/cupertino-shell-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/cupertino-shell-v1.xml");

    /// Bindings of `protocols/cupertino-screenshot-v1.xml`, in a module of their own because each scanner run defines the
    /// same helper items.
    pub mod screenshot {
        use wayland_client;
        use wayland_client::protocol::*;

        pub mod __interfaces {
            use wayland_client::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("protocols/cupertino-screenshot-v1.xml");
        }
        use self::__interfaces::*;

        wayland_scanner::generate_client_code!("protocols/cupertino-screenshot-v1.xml");
    }

    /// Private desktop-control discovery, separated because scanner runs define the same helper items.
    pub mod control {
        use wayland_client;
        use wayland_client::protocol::*;

        pub mod __interfaces {
            use wayland_client::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("protocols/cupertino-control-v1.xml");
        }
        use self::__interfaces::*;

        wayland_scanner::generate_client_code!("protocols/cupertino-control-v1.xml");
    }

    pub use cupertino_app_protocols::client::{app_effects, scroll};
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
    use wayland_server;
    use wayland_server::protocol::*;

    pub mod __interfaces {
        use wayland_server::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/cupertino-shell-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("protocols/cupertino-shell-v1.xml");

    /// Bindings of `protocols/cupertino-screenshot-v1.xml`, in a module of their own because each scanner run defines the
    /// same helper items.
    pub mod screenshot {
        use wayland_server;
        use wayland_server::protocol::*;

        pub mod __interfaces {
            use wayland_server::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("protocols/cupertino-screenshot-v1.xml");
        }
        use self::__interfaces::*;

        wayland_scanner::generate_server_code!("protocols/cupertino-screenshot-v1.xml");
    }

    /// Private desktop-control discovery, separated because scanner runs define the same helper items.
    pub mod control {
        use wayland_server;
        use wayland_server::protocol::*;

        pub mod __interfaces {
            use wayland_server::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("protocols/cupertino-control-v1.xml");
        }
        use self::__interfaces::*;

        wayland_scanner::generate_server_code!("protocols/cupertino-control-v1.xml");
    }

    pub use cupertino_app_protocols::server::{app_effects, scroll};
}
