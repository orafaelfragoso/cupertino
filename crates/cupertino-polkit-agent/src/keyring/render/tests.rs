use cupertino_assets::Assets;
use cupertino_geometry::Point;
use cupertino_theme::{Appearance, DesignTokens};
use cupertino_ui_core::{AccessibilityUpdate, AccessibleValue, Role};
use cupertino_ui_platform::WindowId;

use super::*;
use crate::keyring::access::{keyring_tree, requested, KeyringNode};
use crate::keyring::model::tests::asking;
use crate::keyring::model::{Key, Prompter, Value};
use crate::render::Palette;

fn painter() -> Painter {
    let tokens = DesignTokens::from_path(cupertino_theme::spec_dir().join("tokens.json")).unwrap();
    Painter::new(
        Palette::from_tokens(&tokens, Appearance::Light).unwrap(),
        &Assets::builtin(),
    )
    .unwrap()
}

fn byte(c: [f32; 4]) -> [u8; 4] {
    c.map(|v| (v * 255.0).round() as u8)
}

fn rgba(pm: &Pixmap, at: Point, scale: f64) -> [u8; 4] {
    let p = pm
        .pixel((at.x * scale) as u32, (at.y * scale) as u32)
        .unwrap()
        .demultiply();
    [p.red(), p.green(), p.blue(), p.alpha()]
}

/// The unlock question the keyring daemon asks.
fn unlock(warning: &str) -> Prompter {
    asking(
        Kind::Password,
        vec![
            ("title", Value::Str("Unlock Keyring".into())),
            ("message", Value::Str("Authentication required".into())),
            (
                "description",
                Value::Str(
                    "An application wants access to the keyring “login”, but it is locked".into(),
                ),
            ),
            ("warning", Value::Str(warning.into())),
            ("continue-label", Value::Str("_Unlock".into())),
        ],
    )
    .0
}

fn draw(painter: &mut Painter, prompter: &Prompter, scale: f64) -> (Pixmap, Layout) {
    let prompt = prompter.prompt().unwrap();
    let layout = Layout::new(painter, prompt, false);
    let panel = Panel::new(prompt);
    let f = &layout.frames;
    let mut pixmap = Pixmap::new((f.width * scale) as u32, (f.height * scale) as u32).unwrap();
    painter.draw_keyring(&layout.view(&panel, prompt), &mut pixmap, scale, 0.0);
    (pixmap, layout)
}

#[test]
fn the_unlock_prompt_lays_out_its_heading_description_field_and_buttons() {
    let mut p = painter();
    let prompter = unlock("");
    let layout = Layout::new(&mut p, prompter.prompt().unwrap(), false);
    assert_eq!(layout.heading, vec!["Authentication required".to_owned()]);
    assert!(layout.description.len() >= 2, "{:?}", layout.description);
    assert_eq!(layout.continue_title, "Unlock");
    assert_eq!(layout.cancel_title, "Cancel");
    let f = &layout.frames;
    assert_eq!(
        (
            f.width,
            f.cancel.height(),
            f.continue_.x() - f.cancel.max_x()
        ),
        (260.0, 28.0, 8.0)
    );
    assert!(f.password.is_some() && f.confirm.is_none() && f.status.is_empty());
    let warned = Layout::new(
        &mut p,
        unlock("The unlock password was incorrect")
            .prompt()
            .unwrap(),
        true,
    );
    assert_eq!(warned.status.len(), 2, "the warning and Caps Lock");
    assert_eq!(warned.frames.height, f.height + 6.0 + 2.0 * 14.0);
}

#[test]
fn the_warning_is_red_and_outlines_the_field_and_continue_is_the_accent_capsule() {
    for scale in [1.0, 2.0] {
        let mut p = painter();
        let red = byte(p.palette.red);
        let accent = byte(p.palette.accent);
        let (plain, layout) = draw(&mut p, &unlock(""), scale);
        let field = layout.frames.password.unwrap();
        let outline = Point::new(field.x() - 1.0, field.center().y);
        assert_ne!(rgba(&plain, outline, scale), red);
        let button = Point::new(
            layout.frames.continue_.x() + 16.0,
            layout.frames.continue_.y() + 3.0,
        );
        assert_eq!(rgba(&plain, button, scale), accent, "at {scale}x");

        let (warned, layout) = draw(&mut p, &unlock("The unlock password was incorrect"), scale);
        assert_eq!(rgba(&warned, outline, scale), red, "at {scale}x");
        let line = layout.frames.status[0];
        let mut reds = 0;
        for x in (line.x() as u32)..(line.max_x() as u32) {
            for y in (line.y() as u32)..(line.max_y() as u32) {
                let px = rgba(&warned, Point::new(f64::from(x), f64::from(y)), scale);
                if px[3] > 0 && px[0] > 150 && px[1] < 100 {
                    reds += 1;
                }
            }
        }
        assert!(reds > 20, "the warning's text is red at {scale}x: {reds}");
    }
}

#[test]
fn over_the_glass_the_body_is_left_clear() {
    let mut p = painter();
    p.glass = true;
    let (pixmap, layout) = draw(&mut p, &unlock(""), 1.0);
    let inside = Point::new(layout.frames.width - 30.0, layout.frames.symbol.center().y);
    assert_eq!(rgba(&pixmap, inside, 1.0)[3], 0);
    p.glass = false;
    let (pixmap, _) = draw(&mut p, &unlock(""), 1.0);
    assert_eq!(rgba(&pixmap, inside, 1.0), byte(p.palette.background));
}

#[test]
fn a_chosen_choice_is_an_accent_check_box() {
    let mut p = painter();
    let (mut prompter, _) = asking(
        Kind::Password,
        vec![
            ("password-new", Value::Bool(true)),
            ("choice-label", Value::Str("Remember".into())),
        ],
    );
    let (unchosen, layout) = draw(&mut p, &prompter, 1.0);
    assert_eq!(layout.choice, vec!["Remember".to_owned()]);
    let row = layout.frames.choice.unwrap();
    let corner = Point::new(row.x() + 2.0, row.center().y - CHECK_BOX / 2.0 + 2.0);
    assert_ne!(rgba(&unchosen, corner, 1.0), byte(p.palette.accent));
    prompter.toggle_choice();
    let (chosen, _) = draw(&mut p, &prompter, 1.0);
    assert_eq!(rgba(&chosen, corner, 1.0), byte(p.palette.accent));
}

fn tree(painter: &mut Painter, prompter: &Prompter) -> AccessibilityUpdate {
    let prompt = prompter.prompt().unwrap();
    let layout = Layout::new(painter, prompt, false);
    let panel = Panel::new(prompt);
    keyring_tree(WindowId(1), &layout.view(&panel, prompt))
}

fn audit(what: &str, update: &AccessibilityUpdate) {
    let mut problems = cupertino_ui_test::accessibility_audit(update);
    problems.extend(cupertino_ui_test::secret_leaks(
        update,
        &["hunter2", "hunter3"],
    ));
    assert!(
        problems.is_empty(),
        "{what}: {problems:#?}\n{}",
        cupertino_ui_test::semantic_outline(update).join("\n")
    );
}

#[test]
fn assistive_technology_hears_a_modal_alert_whose_fields_are_protected() {
    let mut p = painter();
    let (mut prompter, _) = asking(
        Kind::Password,
        vec![
            (
                "message",
                Value::Str("Choose password for new keyring".into()),
            ),
            ("password-new", Value::Bool(true)),
            ("choice-label", Value::Str("Remember".into())),
        ],
    );
    for c in "hunter2".chars() {
        prompter.key(Field::Password, Key::Char(c));
    }
    for c in "hunter3".chars() {
        prompter.key(Field::Confirm, Key::Char(c));
    }
    let update = tree(&mut p, &prompter);
    audit("typing", &update);
    let root = &update.nodes[0].1;
    assert_eq!(root.role, Role::AlertDialog);
    assert!(root.states.modal);
    let node = |n: KeyringNode| &update.nodes.iter().find(|(id, _)| *id == n.id()).unwrap().1;
    for field in [KeyringNode::Password, KeyringNode::Confirm] {
        assert_eq!(node(field).role, Role::PasswordInput);
        assert_eq!(node(field).value, Some(AccessibleValue::Protected));
    }
    assert_eq!(node(KeyringNode::Choice).role, Role::CheckBox);
    assert_eq!(update.focus, Some(KeyringNode::Password.id()));
    prompter.answer();
    let mismatch = tree(&mut p, &prompter);
    audit("mismatch", &mismatch);
    assert_eq!(
        requested(KeyringNode::Continue.id()),
        Some(Control::Continue)
    );
    assert_eq!(requested(KeyringNode::Heading.id()), None);
}

#[test]
fn the_daemons_long_choice_wraps_under_its_check_box_without_its_mnemonic() {
    let mut p = painter();
    let (prompter, _) = asking(
        Kind::Password,
        vec![(
            "choice-label",
            Value::Str("_Automatically unlock this keyring whenever I'm logged in".into()),
        )],
    );
    let layout = Layout::new(&mut p, prompter.prompt().unwrap(), false);
    assert_eq!(layout.choice.len(), 2, "{:?}", layout.choice);
    assert!(layout.choice[0].starts_with("Automatically"));
    assert!(!layout.choice.concat().contains('…'), "{:?}", layout.choice);
}
