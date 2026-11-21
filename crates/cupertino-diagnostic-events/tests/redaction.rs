//! The redaction policy: what each class of value becomes, what free text loses, the salted hash, and a secret that
//! reaches neither the ring nor the store.

use crate::support;

use std::path::Path;
use std::time::Duration;

use cupertino_diagnostic_events::redact::{Policy, Salt};
use cupertino_diagnostic_events::{self as diag, AddressKind, Identity, Severity, Value};

/// What `value` becomes under the test policy, as text, and whether it was marked redacted.
fn reduce(value: &Value<'_>) -> (String, bool) {
    let reduced = support::policy().value(value);
    let shown = match reduced.value {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    };
    (shown, reduced.rewritten)
}

/// How a reduced value must look.
enum Shown {
    /// This text exactly.
    Is(&'static str),
    /// This prefix and a hash of eight hexadecimal digits.
    Hashed(&'static str),
}

#[test]
fn each_class_of_value_is_kept_dropped_or_rewritten() {
    use Shown::{Hashed, Is};
    let path = |p: &'static str| Value::Path(Path::new(p));
    let rows: Vec<(Value<'_>, Shown, bool)> = vec![
        // what the user wrote or typed, and secrets, never stay
        (Value::Secret("hunter2"), Is("[redacted]"), true),
        (Value::UserText("Invoice — Bank"), Is("[redacted]"), true),
        (
            Value::UserText("https://example.org/a?b=c"),
            Is("[redacted]"),
            true,
        ),
        // the desktop's own paths under the home stay, relative to it
        (
            path("/home/alex/.local/state/cupertino/server.log"),
            Is("~/.local/state/cupertino/server.log"),
            true,
        ),
        (
            path("/home/alex/.config/cupertino/settings.json"),
            Is("~/.config/cupertino/settings.json"),
            true,
        ),
        (path("/home/alex"), Is("~"), true),
        // any other path under the home keeps only a hash of its name
        (path("/home/alex/Documents/taxes.pdf"), Hashed("~/…#"), true),
        (
            path("/home/alex/.local/state/other/x"),
            Hashed("~/…#"),
            true,
        ),
        // system paths stay, less the user's name
        (
            path("/usr/share/cupertino/theme/x.toml"),
            Is("/usr/share/cupertino/theme/x.toml"),
            false,
        ),
        (path("/etc/os-release"), Is("/etc/os-release"), false),
        (
            path("/run/user/1000/wayland-1"),
            Is("/run/user/1000/wayland-1"),
            false,
        ),
        (path("/dev/dri/card1"), Is("/dev/dri/card1"), false),
        (
            path("/sys/class/drm/card1-DP-1"),
            Is("/sys/class/drm/card1-DP-1"),
            false,
        ),
        (
            path("/run/media/alex/USB"),
            Is("/run/media/[user]/USB"),
            true,
        ),
        // another home, /tmp, relative paths and paths that climb out are hashed
        (path("/home/bob/x.txt"), Hashed("/home/…#"), true),
        (path("/root/secret"), Hashed("/root/…#"), true),
        (path("/tmp/alex-notes.txt"), Hashed("…#"), true),
        (path("relative/name.txt"), Hashed("…#"), true),
        (path("/usr/../home/alex/x"), Hashed("…#"), true),
        // addresses and device names are hashed under their kind
        (
            Value::Address(AddressKind::Ip, "192.168.1.20"),
            Hashed("ip#"),
            true,
        ),
        (
            Value::Address(AddressKind::Ip, "fe80::1"),
            Hashed("ip#"),
            true,
        ),
        (
            Value::Address(AddressKind::Mac, "aa:bb:cc:dd:ee:ff"),
            Hashed("mac#"),
            true,
        ),
        (
            Value::Address(AddressKind::Bluetooth, "00:1A:7D:DA:71:13"),
            Hashed("bluetooth#"),
            true,
        ),
        (
            Value::Address(AddressKind::Ssid, "Home Wi-Fi"),
            Hashed("ssid#"),
            true,
        ),
        (
            Value::Address(AddressKind::DeviceName, "Alex's Headphones"),
            Hashed("device#"),
            true,
        ),
        (
            Value::Identifier(Identity::User, "alex"),
            Is("[user]"),
            true,
        ),
        (
            Value::Identifier(Identity::Host, "studio"),
            Is("[host]"),
            true,
        ),
        // public text stays as given, though some looks like a class
        (
            Value::Public("org.cupertino.Files"),
            Is("org.cupertino.Files"),
            false,
        ),
        (
            Value::Public("cupertino-netd.service"),
            Is("cupertino-netd.service"),
            false,
        ),
        (Value::Public("EACCES"), Is("EACCES"), false),
        (Value::Public("0000:01:00.0"), Is("0000:01:00.0"), false),
        (Value::Public("10de:2484"), Is("10de:2484"), false),
        (Value::Public("HDMI-A-2"), Is("HDMI-A-2"), false),
        (
            Value::Public("org.freedesktop.DBus.Error.AccessDenied"),
            Is("org.freedesktop.DBus.Error.AccessDenied"),
            false,
        ),
        (
            Value::Public("the GPU was reset at 12:30:45"),
            Is("the GPU was reset at 12:30:45"),
            false,
        ),
        // numbers stay; a duration is in milliseconds
        (Value::Count(7), Is("7"), false),
        (Value::Bool(false), Is("false"), false),
        (Value::Int(-2), Is("-2"), false),
        (Value::Duration(Duration::from_secs(2)), Is("2000"), false),
    ];
    for (value, expected, marked) in rows {
        let (shown, rewritten) = reduce(&value);
        assert_eq!(rewritten, marked, "{value:?} gave {shown}");
        match expected {
            Is(text) => assert_eq!(shown, text, "{value:?}"),
            Hashed(prefix) => assert!(
                shown
                    .strip_prefix(prefix)
                    .is_some_and(|h| h.len() == 8 && h.bytes().all(|b| b.is_ascii_hexdigit())),
                "{value:?} gave {shown}"
            ),
        }
    }
}

#[test]
fn free_text_loses_its_private_parts_and_keeps_the_rest() {
    let policy = support::policy();
    // what the text becomes starts with the second column
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
        ("fetch https://example.org/a?q=1 failed", "fetch [url] failed"),
        ("open file:///home/alex/x failed", "open [url]"),
        ("user alex has no seat", "user [user] has no seat"),
        ("host studio is unreachable", "host [host] is unreachable"),
        ("alexander and studious stay", "alexander and studious stay"),
        // a path that names a token or a key is a file, and the error after it stays
        (
            "cannot read /usr/share/cupertino/theme/tokens.json: No such file or directory (os error 2)",
            "cannot read /usr/share/cupertino/theme/tokens.json: No such file or directory (os error 2)",
        ),
        (
            "open /etc/ssh/ssh_host_ed25519_key: Permission denied",
            "open /etc/ssh/ssh_host_ed25519_key: Permission denied",
        ),
    ];
    for (input, start) in rows {
        let out = policy.text(input);
        assert!(out.starts_with(start), "{input:?} gave {out:?}");
    }
    // the words of a home path go up to the separator, and what follows it stays
    let out = policy.text("cannot open /home/alex/My Files/plan.odt: not found");
    assert!(
        out.ends_with(": not found") && !out.contains("plan") && !out.contains("My"),
        "{out}"
    );
    assert_eq!(policy.text("a /home/alex/x\nb").lines().count(), 2);
}

#[test]
fn planted_private_data_does_not_survive_free_text_wherever_it_sits() {
    // `zq` is in no replacement and no hash, so finding it means a planted value survived
    let home = "/home/zqmallory/zqplan";
    let email = "zqjane@example.org";
    let token = "0123456789abcdef0123456789abcdef01234567";
    let policy = Policy::new(
        Some(Path::new("/home/someone")),
        Some("someone"),
        Some("somehost"),
        Salt::from_bytes([9; 16]),
    );
    let parts = [home, email, token];
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        for (before, after) in [("", ""), ("a=(", ")."), ("\"", "\" "), ("X: ", " ,")] {
            let text = format!(
                "{before}{} and {} then {}{after}",
                parts[order[0]], parts[order[1]], parts[order[2]]
            );
            let out = policy.text(&text);
            for planted in [
                "zqmallory",
                "zqplan",
                "zqjane",
                &token[..diag::redact::TOKEN_RUN],
            ] {
                assert!(!out.contains(planted), "{text:?} gave {out:?}");
            }
        }
    }
}

#[test]
fn a_salt_is_made_once_per_store_kept_private_and_hashes_stably_under_it() {
    let a = Salt::from_bytes([1; 16]);
    let b = Salt::from_bytes([2; 16]);
    assert_eq!(a.hash("Home Wi-Fi"), a.hash("Home Wi-Fi"));
    assert_ne!(a.hash("Home Wi-Fi"), b.hash("Home Wi-Fi"));
    assert_ne!(a.hash("Home Wi-Fi"), a.hash("Home Wi-Fj"));

    // a later process reads the same salt; another store has another
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("salt");
    let first = Salt::load_or_create(&path).unwrap();
    assert_eq!(Salt::load_or_create(&path).unwrap(), first);
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o600);
    let other = tempfile::tempdir().unwrap();
    assert_ne!(
        Salt::load_or_create(&other.path().join("salt")).unwrap(),
        first
    );

    // processes that make it at once all keep the one made first, and leave nothing else behind
    for _ in 0..20 {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("salt");
        let start = std::sync::Barrier::new(8);
        let salts: Vec<Salt> = std::thread::scope(|scope| {
            let made: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        start.wait();
                        Salt::load_or_create(&path).unwrap()
                    })
                })
                .collect();
            made.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let kept = Salt::load_or_create(&path).unwrap();
        assert!(salts.iter().all(|s| *s == kept));
        let names: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["salt"]);
    }

    // a broken salt is replaced, and the new one kept
    std::fs::write(&path, b"short").unwrap();
    let made = Salt::load_or_create(&path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap().len(), 16);
    assert_eq!(Salt::load_or_create(&path).unwrap(), made);
}

/// The secret every sink is checked for.
const SECRET: &str = "s3cr3t-Pa55w0rd-NEVER";

#[test]
fn a_secret_reaches_neither_the_ring_nor_any_file_of_the_store() {
    let state = tempfile::tempdir().unwrap();
    support::isolate(state.path());
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
    assert!(!ring.contains("Pa55w0rd"), "{ring}");
    let dir = state.path().join("cupertino/events");
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .collect();
    assert!(files.len() >= 2, "a segment and the salt: {files:?}");
    for file in files {
        let bytes = std::fs::read(&file).unwrap();
        assert!(
            !support::contains(&bytes, b"Pa55w0rd"),
            "{}",
            file.display()
        );
    }
}
