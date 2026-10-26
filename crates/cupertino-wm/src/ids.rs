//! Opaque identifiers. The compositor chooses the numeric values (for windows and outputs it
//! typically reuses its own surface/output counters); the model never interprets them.

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub u64);
    };
}

id_type!(
    /// A running application (grouping of toplevels by application identity string).
    AppId
);
id_type!(
    /// A toplevel window.
    WindowId
);
id_type!(
    /// A physical output (display).
    OutputId
);
id_type!(
    /// A workspace on one output.
    WorkspaceId
);
