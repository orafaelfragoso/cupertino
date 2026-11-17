//! What `wpctl` and `pactl` print, read into values.

use cupertino_pipewire::{
    parse_balance, parse_nodes, parse_sinks, parse_volume, Error, Sink, Volume,
};

const STATUS: &str = "\
PipeWire 'pipewire-0' [1.4.2, user@host, cookie:123]
 └─ Clients:
        33. WirePlumber                         [1.4.2, user@host, pid:900]

Audio
 ├─ Devices:
 │      45. Built-in Audio                      [alsa]
 │
 ├─ Sinks:
 │      46. Built-in Audio Analog Stereo        [vol: 0.40]
 │  *   52. HDMI / DisplayPort 1                   [vol: 1.00]
 │
 ├─ Sink endpoints:
 │
 ├─ Sources:
 │  *   47. Built-in Audio Analog Stereo        [vol: 1.00]
 │      48. Webcam Microphone
 │
 └─ Streams:
        60. Firefox
             61. output_FL       > Built-in Audio:playback_FL

Video
 ├─ Devices:
 │      41. Webcam                              [v4l2]
 │
 ├─ Sinks:
 │
 ├─ Sources:
 │      42. Webcam: Webcam (V4L2)
";

fn sink(id: u32, name: &str, default: bool) -> Sink {
    Sink {
        id,
        name: name.to_owned(),
        default,
    }
}

#[test]
fn a_volume_line_is_a_level_in_zero_to_one_and_a_mute_state() {
    for (text, level, muted) in [
        ("Volume: 0.42\n", 0.42, false),
        ("Volume: 0.30 [MUTED]\n", 0.30, true),
        ("Volume: 1.50", 1.0, false),
        ("Volume: -0.20", 0.0, false),
    ] {
        assert_eq!(
            parse_volume(text).unwrap(),
            Volume { level, muted },
            "{text:?}"
        );
    }
}

#[test]
fn output_that_is_not_a_volume_line_is_unparsed_and_quotes_the_text() {
    for text in [
        "",
        "Volume:",
        "Volume: loud",
        "volume: 0.5",
        "Volume: 0.5 [MUTE]",
        "Volume: 0.5 [MUTED] extra",
        "Volume: NaN",
        "Volume: inf",
        "Volume: 0.5 0.5",
        "0.5",
    ] {
        match parse_volume(text) {
            Err(Error::Unparsed(quoted)) => assert_eq!(quoted, text.trim(), "{text:?}"),
            other => panic!("{text:?} gave {other:?}"),
        }
    }
}

#[test]
fn the_audio_nodes_of_a_section_are_listed_in_order_with_the_default_marked() {
    for (name, nodes, expected) in [
        (
            "sinks, video sinks left out",
            parse_sinks(STATUS),
            vec![
                sink(46, "Built-in Audio Analog Stereo", false),
                sink(52, "HDMI / DisplayPort 1", true),
            ],
        ),
        (
            "sources, the bracket group cut off",
            parse_nodes(STATUS, "Sources:"),
            vec![
                sink(47, "Built-in Audio Analog Stereo", true),
                sink(48, "Webcam Microphone", false),
            ],
        ),
        (
            "devices",
            parse_nodes(STATUS, "Devices:"),
            vec![sink(45, "Built-in Audio", false)],
        ),
        ("a heading not listed", parse_nodes(STATUS, "Filters:"), vec![]),
        ("empty output", parse_sinks(""), vec![]),
        (
            "an empty section",
            parse_sinks("Audio\n ├─ Sinks:\n │\n └─ Streams:\n"),
            vec![],
        ),
        (
            "video only",
            parse_sinks("Video\n ├─ Sinks:\n │      42. Camera\n"),
            vec![],
        ),
        (
            "lines that are not nodes skipped",
            parse_sinks(
                "Audio\n ├─ Sinks:\n │      not-a-number. Name\n │      7. Kept\n │      8.NoSpace\n",
            ),
            vec![sink(7, "Kept", false)],
        ),
        (
            "brackets inside a name kept",
            parse_sinks("Audio\n ├─ Sinks:\n │      9. Name [with] brackets inside\n"),
            vec![sink(9, "Name [with] brackets inside", false)],
        ),
    ] {
        assert_eq!(nodes, expected, "{name}");
    }
    assert!(
        parse_nodes(STATUS, "Streams:").iter().any(|s| s.id == 60),
        "streams"
    );
}

#[test]
fn two_channels_give_a_balance_toward_the_louder_one_and_its_level() {
    for (text, balance, level) in [
        (
            "Volume: front-left: 32768 /  50% / -18.06 dB,   front-right: 32768 /  50% / -18.06 dB\n",
            0.0,
            0.5,
        ),
        (
            "Volume: front-left: 1 / 100% / 0 dB, front-right: 1 / 50% / 0 dB",
            -0.5,
            1.0,
        ),
        (
            "Volume: front-left: 1 / 20% / 0 dB, front-right: 1 / 80% / 0 dB",
            0.75,
            0.8,
        ),
        (
            "Volume: front-left: 1 / 120% / 0 dB, front-right: 1 / 120% / 0 dB",
            0.0,
            1.0,
        ),
        (
            "Volume: front-left: 0 / 0% / -inf dB, front-right: 0 / 0% / -inf dB",
            0.0,
            0.0,
        ),
        (
            "Volume: front-left: 1 / 50% / 0 dB, front-right: 1 / 50% / 0 dB\nBalance: 0.00\n",
            0.0,
            0.5,
        ),
    ] {
        let (got_balance, got_level) = parse_balance(text).unwrap();
        assert!((got_balance - balance).abs() < 1e-9, "{text:?}: {got_balance}");
        assert!((got_level - level).abs() < 1e-9, "{text:?}: {got_level}");
    }
}

#[test]
fn an_output_that_is_not_two_channels_has_no_balance() {
    for text in [
        "",
        "Volume: mono: 65536 / 100% / 0.00 dB",
        "Volume: a: 1 / 10% / 0 dB, b: 1 / 20% / 0 dB, c: 1 / 30% / 0 dB",
        "Volume: nothing",
    ] {
        assert!(
            matches!(parse_balance(text), Err(Error::Unparsed(_))),
            "{text:?}"
        );
    }
}
