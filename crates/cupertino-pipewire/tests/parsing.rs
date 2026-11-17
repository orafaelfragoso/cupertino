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
fn a_volume_line_is_a_level() {
    assert_eq!(
        parse_volume("Volume: 0.42\n").unwrap(),
        Volume {
            level: 0.42,
            muted: false
        }
    );
}

#[test]
fn a_muted_marker_is_the_mute_state() {
    assert_eq!(
        parse_volume("Volume: 0.30 [MUTED]\n").unwrap(),
        Volume {
            level: 0.30,
            muted: true
        }
    );
}

#[test]
fn a_level_above_one_reads_as_one_and_a_negative_one_as_zero() {
    assert_eq!(parse_volume("Volume: 1.50").unwrap().level, 1.0);
    assert_eq!(parse_volume("Volume: -0.20").unwrap().level, 0.0);
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
fn the_audio_sinks_are_listed_in_order_with_the_default_marked() {
    assert_eq!(
        parse_sinks(STATUS),
        [
            sink(46, "Built-in Audio Analog Stereo", false),
            sink(52, "HDMI / DisplayPort 1", true),
        ]
    );
}

#[test]
fn the_audio_sources_are_listed_and_a_trailing_bracket_group_is_cut_off() {
    assert_eq!(
        parse_nodes(STATUS, "Sources:"),
        [
            sink(47, "Built-in Audio Analog Stereo", true),
            sink(48, "Webcam Microphone", false),
        ]
    );
}

#[test]
fn video_nodes_and_other_sections_are_not_audio_nodes() {
    assert!(parse_sinks(STATUS).iter().all(|s| s.id != 42 && s.id != 41));
    assert!(parse_nodes(STATUS, "Streams:").iter().any(|s| s.id == 60));
    assert!(parse_nodes(STATUS, "Devices:").iter().any(|s| s.id == 45));
    assert!(parse_nodes(STATUS, "Filters:").is_empty());
}

#[test]
fn a_heading_that_is_not_listed_or_empty_output_gives_no_nodes() {
    assert!(parse_sinks("").is_empty());
    assert!(parse_sinks("Audio\n ├─ Sinks:\n │\n └─ Streams:\n").is_empty());
    assert!(parse_sinks("Video\n ├─ Sinks:\n │      42. Camera\n").is_empty());
}

#[test]
fn a_line_that_is_not_a_node_is_skipped() {
    let text =
        "Audio\n ├─ Sinks:\n │      not-a-number. Name\n │      7. Kept\n │      8.NoSpace\n";
    assert_eq!(parse_sinks(text), [sink(7, "Kept", false)]);
}

#[test]
fn a_name_without_a_trailing_bracket_group_is_kept_whole() {
    let text = "Audio\n ├─ Sinks:\n │      9. Name [with] brackets inside\n";
    assert_eq!(
        parse_sinks(text),
        [sink(9, "Name [with] brackets inside", false)]
    );
}

#[test]
fn equal_channels_have_no_balance() {
    let (balance, level) = parse_balance(
        "Volume: front-left: 32768 /  50% / -18.06 dB,   front-right: 32768 /  50% / -18.06 dB\n",
    )
    .unwrap();
    assert_eq!(balance, 0.0);
    assert_eq!(level, 0.5);
}

#[test]
fn the_quieter_channel_gives_the_side_and_the_louder_one_the_level() {
    let (balance, level) =
        parse_balance("Volume: front-left: 1 / 100% / 0 dB, front-right: 1 / 50% / 0 dB").unwrap();
    assert_eq!(balance, -0.5);
    assert_eq!(level, 1.0);
    let (balance, level) =
        parse_balance("Volume: front-left: 1 / 20% / 0 dB, front-right: 1 / 80% / 0 dB").unwrap();
    assert!((balance - 0.75).abs() < 1e-9);
    assert!((level - 0.8).abs() < 1e-9);
}

#[test]
fn a_level_above_one_hundred_percent_is_capped_at_one() {
    let (balance, level) =
        parse_balance("Volume: front-left: 1 / 120% / 0 dB, front-right: 1 / 120% / 0 dB").unwrap();
    assert_eq!(balance, 0.0);
    assert_eq!(level, 1.0);
}

#[test]
fn silence_has_no_balance_and_no_level() {
    assert_eq!(
        parse_balance("Volume: front-left: 0 / 0% / -inf dB, front-right: 0 / 0% / -inf dB")
            .unwrap(),
        (0.0, 0.0)
    );
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

#[test]
fn only_the_first_line_of_the_balance_is_read() {
    let (balance, _) = parse_balance(
        "Volume: front-left: 1 / 50% / 0 dB, front-right: 1 / 50% / 0 dB\nBalance: 0.00\n",
    )
    .unwrap();
    assert_eq!(balance, 0.0);
}
