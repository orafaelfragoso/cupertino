//! The theme directory a build compiles in follows its checkout when the checkout moves and the target stays, as a
//! gate's merge worktree shares its branch's target. A crate of one `main` with this crate's build script stands in for
//! the theme crate.

use std::fs;
use std::path::Path;
use std::process::Command;

const MANIFEST: &str = r#"[package]
name = "moved-checkout"
version = "0.0.0"
edition = "2024"

[workspace]
"#;

const MAIN: &str = r#"fn main() {
    print!("{}", env!("CUPERTINO_REPOSITORY_THEME_DIR"));
}
"#;

fn theme_dir(checkout: &Path, target: &Path) -> String {
    let out = Command::new(env!("CARGO"))
        .args(["run", "--quiet", "--offline"])
        .current_dir(checkout.join("crates/cupertino-theme"))
        .env("CARGO_TARGET_DIR", target)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn a_moved_checkout_compiles_in_its_new_theme_directory() {
    let root =
        std::env::temp_dir().join(format!("cupertino-moved-checkout-{}", std::process::id()));
    let (first, second, target) = (root.join("first"), root.join("second"), root.join("target"));
    let krate = first.join("crates/cupertino-theme");
    fs::create_dir_all(krate.join("src")).unwrap();
    fs::write(krate.join("Cargo.toml"), MANIFEST).unwrap();
    fs::write(krate.join("build.rs"), include_str!("../build.rs")).unwrap();
    fs::write(krate.join("src/main.rs"), MAIN).unwrap();

    let before = theme_dir(&first, &target);
    fs::rename(&first, &second).unwrap();
    let after = theme_dir(&second, &target);
    fs::remove_dir_all(&root).unwrap();

    assert_eq!(
        (before, after),
        (
            format!(
                "{}/crates/cupertino-theme/../../data/theme",
                first.display()
            ),
            format!(
                "{}/crates/cupertino-theme/../../data/theme",
                second.display()
            ),
        )
    );
}
