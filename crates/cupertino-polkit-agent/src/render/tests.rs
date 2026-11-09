use super::*;
use crate::agent::{Request, User};
use crate::helper::Line;
use crate::model::{Agent, Key};
use crate::panel::{Content, Frames, Panel};

fn tokens() -> DesignTokens {
    DesignTokens::from_path(cupertino_theme::spec_dir().join("tokens.json")).unwrap()
}

fn painter() -> Painter {
    Painter::new(
        Palette::from_tokens(&tokens(), Appearance::Light).unwrap(),
        &Assets::builtin(),
    )
    .unwrap()
}

const MESSAGE: &str = "Authentication is required to mount the disk";

fn asking() -> Agent {
    let mut agent = Agent::new(User(1000));
    agent.begin(Request {
        cookie: "c".into(),
        action: "org.example.mount".into(),
        message: MESSAGE.into(),
        details: Vec::new(),
        users: vec![User(1000)],
    });
    agent.helper(Line::Prompt {
        text: "Password: ".into(),
        echo: false,
    });
    agent
}

fn draw(painter: &mut Painter, agent: &Agent) -> (Pixmap, Frames) {
    let prompt = agent.prompt().unwrap();
    let status = Status::of(prompt, false);
    let (cancel_title, authenticate_title) = painter.button_titles();
    let message = painter.wrap(MESSAGE, 312.0);
    let frames = Frames::new(&Content {
        message_lines: message.len(),
        users: false,
        status_lines: status.len(),
        details: None,
        cancel_title,
        authenticate_title,
    });
    let mut pixmap = Pixmap::new(frames.width as u32, frames.height as u32).unwrap();
    painter.draw(
        &View {
            frames: &frames,
            panel: &Panel::new(false),
            prompt,
            message: &message,
            user: None,
            status: &status,
            details: &[],
        },
        &mut pixmap,
        1.0,
        0.0,
    );
    (pixmap, frames)
}

fn rgba(pm: &Pixmap, at: Point) -> [u8; 4] {
    let p = pm.pixel(at.x as u32, at.y as u32).unwrap().demultiply();
    [p.red(), p.green(), p.blue(), p.alpha()]
}

fn byte(c: Rgba) -> [u8; 4] {
    c.map(|v| (v * 255.0).round() as u8)
}

/// The scrim fades in over 200 ms, easing out, from nothing to its colour's own alpha, each frame of the fade more
/// opaque than the one before; under Reduce Motion it shows its whole alpha from the first frame. What reaches the
/// screen is the colour at that opacity, the same whether it is one stretched pixel or a filled frame.
#[test]
fn the_scrim_fades_in_to_its_colour_s_alpha_easing_out_and_at_once_under_reduce_motion() {
    let palette = Palette::from_tokens(&tokens(), Appearance::Dark).unwrap();
    let full = palette.scrim[3];
    let alpha = |ms: u64, reduce: bool| {
        scrim_color(&palette, scrim_opacity(Duration::from_millis(ms), reduce))[3]
    };
    assert_eq!(alpha(0, false), 0.0, "nothing at the start");
    // ease-out: 1 - (1 - t)^3
    for (ms, opacity) in [(50, 0.578125), (100, 0.875), (150, 0.984375)] {
        assert!(
            (f64::from(alpha(ms, false)) - f64::from(full) * opacity).abs() < 1e-6,
            "{ms} ms in: {opacity} of the scrim's alpha"
        );
    }
    let frames: Vec<f32> = (0..13).map(|i| alpha(i * 16, false)).collect();
    assert!(
        frames.windows(2).all(|w| w[0] < w[1]),
        "each frame darker: {frames:?}"
    );
    for ms in [200, 216, 5_000] {
        assert_eq!(alpha(ms, false), full, "settled at {ms} ms");
    }
    for ms in [0, 16, 200] {
        assert_eq!(alpha(ms, true), full, "no fade under Reduce Motion");
    }
    let shown = scrim_color(&palette, 1.0);
    assert_eq!(shown, palette.scrim, "settled, it is the scrim's colour");
}

#[test]
fn every_appearance_has_the_prompts_colours_and_the_scrim_darkens_more_in_dark_and_high_contrast() {
    let tokens = tokens();
    let scrim = |a: Appearance| Palette::from_tokens(&tokens, a).unwrap().scrim[3];
    for a in Appearance::ALL {
        Palette::from_tokens(&tokens, a).unwrap();
    }
    assert!(scrim(Appearance::Light) < scrim(Appearance::Dark));
    assert!(scrim(Appearance::Dark) < scrim(Appearance::DarkHighContrast));
    assert_eq!(
        scrim(Appearance::LightHighContrast),
        scrim(Appearance::DarkHighContrast)
    );
    let mut pm = Pixmap::new(4, 4).unwrap();
    let palette = Palette::from_tokens(&tokens, Appearance::Light).unwrap();
    draw_scrim(&mut pm, &palette, 0.5);
    assert_eq!(pm.pixel(2, 2).unwrap().alpha(), 58);
    let shown = scrim_color(&palette, 0.5);
    assert_eq!(
        (shown[..3].to_vec(), shown[3]),
        (palette.scrim[..3].to_vec(), palette.scrim[3] * 0.5),
        "the colour shown without a frame is the one drawn"
    );
}

#[test]
fn the_message_wraps_to_six_lines_at_most_and_says_when_it_is_cut() {
    let mut p = painter();
    assert_eq!(p.wrap(MESSAGE, 312.0), [MESSAGE]);
    let long = "word ".repeat(200);
    let lines = p.wrap(&long, 120.0);
    assert_eq!(lines.len(), MAX_MESSAGE_LINES);
    assert!(lines[5].ends_with('…'), "{lines:?}");
    assert!(lines
        .iter()
        .all(|l| p.measure(l, TEXT_SIZE, WEIGHT) <= 120.0));
    let unbroken = p.wrap(&"x".repeat(400), 120.0);
    assert_eq!(unbroken.len(), 1);
    assert!(unbroken[0].ends_with('…'));
}

#[test]
fn a_long_path_keeps_its_start_and_its_file_name() {
    let mut p = painter();
    let path = "/usr/lib/a/very/long/directory/that/goes/on/and/on/bin/program";
    let fitted = p.fit_middle(path, 140.0);
    assert!(
        fitted.starts_with("/usr") && fitted.ends_with("program"),
        "{fitted}"
    );
    assert!(fitted.contains('…'));
    assert!(p.measure(&fitted, CAPTION_SIZE, WEIGHT) <= 140.0);
    assert_eq!(p.fit_middle("/usr/bin/pkexec", 140.0), "/usr/bin/pkexec");
}

#[test]
fn a_failed_attempt_outlines_the_field_in_red_and_authenticate_waits_for_typing() {
    let mut p = painter();
    let red = byte(p.palette.red);
    let accent = byte(p.palette.accent);
    let mut agent = asking();
    let (fresh, frames) = draw(&mut p, &agent);
    let outline = Point::new(frames.field.x() - 1.0, frames.field.center().y);
    assert_ne!(rgba(&fresh, outline), red);
    let button = Point::new(frames.authenticate.x() + 4.0, frames.authenticate.y() + 4.0);
    assert_ne!(rgba(&fresh, button), accent, "nothing typed yet");

    for c in "wrong".chars() {
        agent.key(Key::Char(c));
    }
    let (typed, _) = draw(&mut p, &agent);
    assert_eq!(rgba(&typed, button), accent);

    agent.key(Key::Return);
    agent.helper(Line::Failure);
    let (failed, frames) = draw(&mut p, &agent);
    let outline = Point::new(frames.field.x() - 1.0, frames.field.center().y);
    assert_eq!(rgba(&failed, outline), red);
}
