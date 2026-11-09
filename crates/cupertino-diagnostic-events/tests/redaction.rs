//! The redaction policy: one row per class of the README's table, the free-text scrubber against planted private data,
//! the salt, and a secret that never reaches the ring, the store or the journal.

mod support;

use std::os::unix::net::UnixDatagram;
use std::path::Path;

use cupertino_diagnostic_events::journal;
use cupertino_diagnostic_events::redact::{Policy, Salt};
use cupertino_diagnostic_events::{self as diag, AddressKind, Identity, Severity, Value};
use proptest::prelude::*;

fn policy() -> Policy {
    Policy::new(
        Some(Path::new("/home/alex")),
        Some("alex"),
        Some("studio"),
        Salt::from_bytes([3; 16]),
    )
}

/// What `value` becomes, and whether it was marked redacted.
fn reduce(value: Value<'_>) -> (String, bool) {
    let reduced = policy().value(&value);
    let shown = match reduced.value {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    };
    (shown, reduced.rewritten)
}

/// `kind#` and eight hexadecimal digits.
fn hashed(shown: &str, prefix: &str) -> bool {
    shown
        .strip_prefix(prefix)
        .is_some_and(|h| h.len() == 8 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[test]
fn each_class_of_value_is_kept_dropped_or_rewritten_as_the_table_says() {
    let dropped = [
        Value::Secret("hunter2"),
        Value::Secret("Bearer abc.def"),
        Value::UserText("typed text"),
        Value::UserText("clipboard contents"),
        Value::UserText("Invoice — Bank"),
        Value::UserText("Your package shipped"),
        Value::UserText("search words"),
        Value::UserText("holiday.jpg"),
        Value::UserText("https://example.org/a?b=c"),
    ];
    for value in dropped {
        assert_eq!(reduce(value), ("[redacted]".to_owned(), true), "{value:?}");
    }
    let paths = [
        (
            "/home/alex/.local/state/cupertino/server.log",
            "~/.local/state/cupertino/server.log",
            true,
        ),
        (
            "/home/alex/.config/cupertino/settings.json",
            "~/.config/cupertino/settings.json",
            true,
        ),
        ("/home/alex", "~", true),
        (
            "/usr/share/cupertino/theme/x.toml",
            "/usr/share/cupertino/theme/x.toml",
            false,
        ),
        ("/etc/os-release", "/etc/os-release", false),
        (
            "/run/user/1000/wayland-1",
            "/run/user/1000/wayland-1",
            false,
        ),
        ("/run/media/alex/USB", "/run/media/[user]/USB", true),
        ("/dev/dri/card1", "/dev/dri/card1", false),
        (
            "/sys/class/drm/card1-DP-1",
            "/sys/class/drm/card1-DP-1",
            false,
        ),
    ];
    for (path, shown, rewritten) in paths {
        assert_eq!(
            reduce(Value::Path(Path::new(path))),
            (shown.to_owned(), rewritten),
            "{path}"
        );
    }
    for (path, prefix) in [
        ("/home/alex/Documents/taxes.pdf", "~/…#"),
        ("/home/alex/.local/state/other/x", "~/…#"),
        ("/home/bob/x.txt", "/home/…#"),
        ("/root/secret", "/root/…#"),
        ("/tmp/alex-notes.txt", "…#"),
        ("relative/name.txt", "…#"),
        ("/usr/../home/alex/x", "…#"),
    ] {
        let (shown, rewritten) = reduce(Value::Path(Path::new(path)));
        assert!(rewritten && hashed(&shown, prefix), "{path} gave {shown}");
    }
    for (kind, text, prefix) in [
        (AddressKind::Ip, "192.168.1.20", "ip#"),
        (AddressKind::Ip, "fe80::1", "ip#"),
        (AddressKind::Mac, "aa:bb:cc:dd:ee:ff", "mac#"),
        (AddressKind::Bluetooth, "00:1A:7D:DA:71:13", "bluetooth#"),
        (AddressKind::Ssid, "Home Wi-Fi", "ssid#"),
        (AddressKind::DeviceName, "Alex's Headphones", "device#"),
    ] {
        let (shown, rewritten) = reduce(Value::Address(kind, text));
        assert!(rewritten && hashed(&shown, prefix), "{text} gave {shown}");
    }
    assert_eq!(
        reduce(Value::Identifier(Identity::User, "alex")),
        ("[user]".to_owned(), true)
    );
    assert_eq!(
        reduce(Value::Identifier(Identity::Host, "studio")),
        ("[host]".to_owned(), true)
    );
    // kept, though some look like a class: app ids, components, units, error kinds, errno names, GPU ids, connectors
    for kept in [
        "org.cupertino.Files",
        "cupertino-dock",
        "cupertino-netd.service",
        "PermissionDenied",
        "EACCES",
        "amdgpu",
        "0000:01:00.0",
        "10de:2484",
        "DP-1",
        "eDP-1",
        "HDMI-A-2",
        "org.freedesktop.portal.ScreenCast",
        "org.freedesktop.DBus.Error.AccessDenied",
        "the GPU was reset at 12:30:45",
        "cupertino_server::gpu_reset",
    ] {
        assert_eq!(
            reduce(Value::Public(kept)),
            (kept.to_owned(), false),
            "{kept}"
        );
    }
    assert_eq!(reduce(Value::Count(7)), ("7".to_owned(), false));
    assert_eq!(reduce(Value::Bool(false)), ("false".to_owned(), false));
    assert_eq!(reduce(Value::Int(-2)), ("-2".to_owned(), false));
    assert_eq!(
        reduce(Value::Duration(std::time::Duration::from_secs(2))),
        ("2000".to_owned(), false)
    );
}

#[test]
fn the_scrubber_rewrites_private_parts_of_free_text() {
    let p = policy();
    let rows = [
        (
            "cannot open /home/alex/My Files/plan.odt: not found",
            "cannot open ~/…#",
        ),
        (
            "read ~/.config/cupertino/settings.json: bad",
            "read ~/.config/cupertino/settings.json: bad",
        ),
        ("connected as alex@studio", "connected as [user]@[host]"),
        ("mail alex.b@example.org now", "mail [email] now"),
        ("peer 10.0.0.7:22 refused", "peer ip#"),
        ("peer [2001:db8::7]:22 refused", "peer [ip#"),
        ("device aa:bb:cc:dd:ee:ff gone", "device mac#"),
        (
            "token 0123456789abcdef0123456789abcdef01234567 rejected",
            "token [token] rejected",
        ),
        (
            "key dGhpcyBpcyBhIHNlY3JldCB0b2tlbiB2YWx1ZTEyMw== rejected",
            "key [token] rejected",
        ),
        (
            "login password=hunter2 failed",
            "login password=[redacted] failed",
        ),
        ("api_key=abc retry", "api_key=[redacted] retry"),
        (
            "Authorization: Bearer abc.def.ghi",
            "Authorization: Bearer [redacted]",
        ),
        ("Cookie: session=xyz", "Cookie: [redacted]"),
        (
            "fetch https://example.org/a?q=1 failed",
            "fetch [url] failed",
        ),
        // a path under a home takes the words after it up to a separator: they may be its name
        ("open file:///home/alex/x failed", "open [url]"),
        ("user alex has no seat", "user [user] has no seat"),
        ("host studio is unreachable", "host [host] is unreachable"),
        ("alexander and studious stay", "alexander and studious stay"),
    ];
    for (input, start) in rows {
        let out = p.text(input);
        assert!(out.starts_with(start), "{input:?} gave {out:?}");
    }
    let out = p.text("cannot open /home/alex/My Files/plan.odt: not found");
    assert!(
        out.ends_with(": not found") && !out.contains("plan") && !out.contains("My"),
        "{out}"
    );
    // several lines stay several lines
    assert_eq!(p.text("a /home/alex/x\nb").lines().count(), 2);
}

#[test]
fn a_hash_is_stable_under_one_salt_and_differs_under_another() {
    let a = Salt::from_bytes([1; 16]);
    let b = Salt::from_bytes([2; 16]);
    assert_eq!(a.hash("Home Wi-Fi"), a.hash("Home Wi-Fi"));
    assert_ne!(a.hash("Home Wi-Fi"), b.hash("Home Wi-Fi"));
    assert_ne!(a.hash("Home Wi-Fi"), a.hash("Home Wi-Fj"));
    // the store keeps its salt: the same value hashes the same in a later process
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("salt");
    let first = Salt::load_or_create(&path).unwrap();
    assert_eq!(Salt::load_or_create(&path).unwrap(), first);
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions())
            & 0o777,
        0o600
    );
    let other = tempfile::tempdir().unwrap();
    assert_ne!(
        Salt::load_or_create(&other.path().join("salt")).unwrap(),
        first
    );
}

#[test]
fn processes_that_make_the_salt_at_the_same_time_all_keep_the_same_one() {
    for _ in 0..20 {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("salt");
        let start = std::sync::Barrier::new(8);
        let salts: Vec<Salt> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        start.wait();
                        Salt::load_or_create(&path).unwrap()
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let kept = Salt::load_or_create(&path).unwrap();
        assert!(salts.iter().all(|s| *s == kept));
        // nothing but the salt is left
        let names: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["salt"]);
    }
    // a broken salt is replaced, and the new one kept
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("salt");
    std::fs::write(&path, b"short").unwrap();
    let made = Salt::load_or_create(&path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap().len(), 16);
    assert_eq!(Salt::load_or_create(&path).unwrap(), made);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Random text with a home path, an e-mail address and a 40-digit token planted in it: none survives.
    #[test]
    fn planted_private_data_never_survives_the_scrubber(
        before in "[A-Z0-9 .,:;=()\\[\\]\"'/-]{0,40}",
        middle in "[A-Z0-9 .,:;=()\\[\\]\"'/-]{0,40}",
        after in "[A-Z0-9 .,:;=()\\[\\]\"'/-]{0,40}",
        user in "[a-z]{3,10}",
        file in "[a-z]{3,10}",
        local in "[a-z]{2,8}",
        domain in "[a-z]{2,8}",
        token in "[0-9a-f]{40}",
        order in 0usize..6,
    ) {
        // `zq` stands in no replacement text and no hash, so finding it means the planted value survived
        let (user, file) = (format!("zq{user}"), format!("zq{file}"));
        let home = format!("/home/{user}/{file}");
        let email = format!("{local}@{domain}.org");
        let mut parts = [home.clone(), email.clone(), token.clone()];
        parts.rotate_left(order % 3);
        if order >= 3 {
            parts.swap(0, 1);
        }
        let text = format!("{before}{}{middle}{}{after} {}", parts[0], parts[1], parts[2]);
        // the scrubber of a process whose own name is not planted
        let p = Policy::new(Some(Path::new("/home/someone")), Some("someone"), Some("somehost"), Salt::from_bytes([9; 16]));
        let out = p.text(&text);
        prop_assert!(!out.contains(&format!("/home/{user}")), "{text:?} gave {out:?}");
        prop_assert!(!out.contains(&file), "{text:?} gave {out:?}");
        prop_assert!(!out.contains(&email), "{text:?} gave {out:?}");
        prop_assert!(!out.contains(&format!("{local}@")), "{text:?} gave {out:?}");
        prop_assert!(!out.contains(&token), "{text:?} gave {out:?}");
        prop_assert!(!out.contains(&token[..32]), "{text:?} gave {out:?}");
    }
}

/// The secret every sink is checked for.
const SECRET: &str = "s3cr3t-Pa55w0rd-NEVER";

#[test]
fn a_secret_never_reaches_the_ring_or_the_store() {
    if !support::is_child() {
        let state = tempfile::tempdir().unwrap();
        let out = support::child(
            "a_secret_never_reaches_the_ring_or_the_store",
            state.path(),
            &[],
        );
        assert!(out.status.success(), "{}", support::stderr(&out));
        let mut found = 0;
        for entry in walk(state.path()) {
            let bytes = std::fs::read(&entry).unwrap();
            assert!(!contains(&bytes, SECRET.as_bytes()), "{}", entry.display());
            assert!(!contains(&bytes, b"Pa55w0rd"), "{}", entry.display());
            found += 1;
        }
        assert!(found >= 2, "the store has its segment and its salt");
        assert!(!support::stderr(&out).contains(SECRET));
        return;
    }
    let emitter = diag::init(diag::component!(), "test", diag::Sink::UserStore);
    diag::event(
        Severity::Error,
        "test.secret.kept_out",
        "authentication failed",
    )
    .field("password", Value::Secret(SECRET))
    .field("typed", Value::UserText(SECRET))
    .field("error", Value::Public(&format!("password={SECRET}")))
    .emit();
    let ring = emitter.recent().concat();
    assert!(ring.contains("test.secret.kept_out"));
    assert!(
        !ring.contains(SECRET) && !ring.contains("Pa55w0rd"),
        "{ring}"
    );
}

#[test]
fn a_secret_never_reaches_the_journal() {
    if !support::is_child() {
        let state = tempfile::tempdir().unwrap();
        let socket_path = state.path().join("journal.socket");
        let socket = UnixDatagram::bind(&socket_path).unwrap();
        let out = support::child(
            "a_secret_never_reaches_the_journal",
            state.path(),
            &[(journal::SOCKET_ENV, socket_path.to_str().unwrap())],
        );
        assert!(out.status.success(), "{}", support::stderr(&out));
        socket.set_nonblocking(true).unwrap();
        let mut buf = vec![0u8; 65536];
        let n = socket.recv(&mut buf).unwrap();
        let datagram = &buf[..n];
        assert!(contains(
            datagram,
            b"CUPERTINO_DIAG_ID=test.secret.kept_out"
        ));
        assert!(!contains(datagram, SECRET.as_bytes()) && !contains(datagram, b"Pa55w0rd"));
        return;
    }
    diag::init(diag::component!(), "test", diag::Sink::Journal);
    diag::event(
        Severity::Error,
        "test.secret.kept_out",
        "authentication failed",
    )
    .field("password", Value::Secret(SECRET))
    .field("error", Value::Public(&format!("token: {SECRET}")))
    .emit();
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}
