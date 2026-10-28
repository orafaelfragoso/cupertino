# cupertino-ui-test

CupertinoKit test harness: `HeadlessWindowSystem`, event `Trace` record and replay, and the `Inspector`'s
deterministic dumps. MPL-2.0. Today the crate declares the public types; the harness follows.

`Trace::record` takes `protected_focus` from its caller for now. The `TestDriver` that follows with the runtime (row 159) derives it from core's focus and never from the test, so a test cannot record a secret field as an ordinary one.
