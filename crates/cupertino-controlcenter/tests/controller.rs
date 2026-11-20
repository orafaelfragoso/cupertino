//! The panel's state machine through its public interface: what opening reads, how readings and changes meet, how a
//! dragged level is written, and what presses, lists, editing and the clock ask the component to carry out.

use cupertino_controlcenter::controller::{Listed, Playing, REFRESH_INTERVAL, WRITE_INTERVAL};
use cupertino_controlcenter::model::{Device, Media, MediaCommand, Network, Output};
use cupertino_controlcenter::{
    Change, Controller, Detail, Effect, Module, OverlayKind, Phase, Read, Reading, Slider, Source,
    TileState, VolumeState, WifiStatus,
};

/// The sources an opening reads, in the order it asks for them.
const OPENING: [Source; 9] = [
    Source::Wifi,
    Source::Bluetooth,
    Source::Microphone,
    Source::DockHiding,
    Source::Keyboard,
    Source::Media,
    Source::Layout,
    Source::Brightness,
    Source::Volume,
];

fn reads(sources: &[Source]) -> Vec<Effect> {
    sources.iter().map(|s| Effect::Read(*s)).collect()
}

fn volume(level: f64, muted: bool) -> VolumeState {
    VolumeState { level, muted }
}

/// What every source of an opening answers in these tests.
fn answer(source: Source) -> Read {
    match source {
        Source::Wifi => Read::Wifi(Ok(WifiStatus {
            enabled: true,
            network: Some("Home".into()),
        })),
        Source::Bluetooth => Read::Bluetooth(Ok(false)),
        Source::Microphone => Read::Microphone(Ok(true)),
        Source::DockHiding => Read::DockHiding(Ok(false)),
        Source::Keyboard => Read::Keyboard(Ok(false)),
        Source::Media => Read::Media(Ok(None)),
        Source::Layout => Read::Layout(Ok(None)),
        Source::Brightness => Read::Brightness(Ok(0.5)),
        Source::Volume => Read::Volume(Ok(volume(0.5, false))),
        Source::Networks => Read::Networks(Ok(Vec::new())),
        Source::Devices => Read::Devices(Ok(Vec::new())),
        Source::Outputs => Read::Outputs(Ok(Vec::new())),
    }
}

/// A panel opened at 0 whose opening readings all came back.
fn opened() -> Controller {
    let mut panel = Controller::new();
    assert_eq!(panel.toggle(0.0), reads(&OPENING));
    for source in OPENING {
        assert_eq!(panel.read(0.0, answer(source)), Vec::new(), "{source:?}");
    }
    panel
}

#[test]
fn opening_reads_every_source_and_a_second_toggle_dismisses() {
    let mut panel = Controller::new();
    assert_eq!(panel.phase(0.0), Phase::Hidden);
    assert_eq!(panel.toggle(0.0), reads(&OPENING));
    assert_eq!(panel.phase(0.0), Phase::Shown);
    assert_eq!(panel.toggle(0.1), Vec::new());
    assert_eq!(panel.phase(0.1), Phase::Hiding);
}

#[test]
fn a_reading_is_shown_and_a_failed_one_disables_its_tile_with_the_reason() {
    let mut panel = Controller::new();
    panel.toggle(0.0);
    assert_eq!(
        panel.controls().tile(Module::Bluetooth),
        TileState::Disabled("not read yet".into())
    );
    panel.read(0.0, Read::Bluetooth(Ok(true)));
    panel.read(0.0, Read::failed(Source::Wifi, "no network service".into()));
    assert_eq!(panel.controls().tile(Module::Bluetooth), TileState::On);
    assert_eq!(
        panel.controls().tile(Module::WiFi),
        TileState::Disabled("no network service".into())
    );
    assert_eq!(panel.controls().subtitle(Module::WiFi), Some("Unavailable"));
    // a disabled tile ignores a press
    assert_eq!(panel.press(Module::WiFi), Vec::new());
}

#[test]
fn one_reading_of_a_source_is_under_way_and_one_asked_for_meanwhile_follows_it() {
    let mut panel = Controller::new();
    panel.toggle(0.0);
    // every source is still being read: nothing more is asked for
    assert_eq!(panel.refresh(0.5), Vec::new());
    assert_eq!(
        panel.read(0.6, answer(Source::Wifi)),
        reads(&[Source::Wifi])
    );
    assert_eq!(panel.read(0.7, answer(Source::Wifi)), Vec::new());
    // a reading nobody asked for is dropped
    assert_eq!(
        panel.read(0.7, Read::Bluetooth(Ok(true))),
        reads(&[Source::Bluetooth])
    );
    assert_eq!(panel.read(0.7, Read::Bluetooth(Ok(false))), Vec::new());
    assert_eq!(panel.read(0.8, Read::Bluetooth(Ok(true))), Vec::new());
    assert_eq!(panel.controls().tile(Module::Bluetooth), TileState::Off);
}

#[test]
fn a_change_is_shown_at_once_and_a_reading_from_before_it_waits_for_it() {
    let mut panel = opened();
    assert_eq!(panel.tick(REFRESH_INTERVAL), reads(&OPENING));
    assert_eq!(
        panel.press(Module::Bluetooth),
        vec![Effect::Change(Change::Bluetooth(true))]
    );
    assert_eq!(panel.controls().tile(Module::Bluetooth), TileState::On);
    // the reading that started before the change is not shown, nor taken again while the change is under way
    assert_eq!(
        panel.read(REFRESH_INTERVAL, Read::Bluetooth(Ok(false))),
        Vec::new()
    );
    assert_eq!(panel.controls().tile(Module::Bluetooth), TileState::On);
    // once the change ended the source is read again, and that reading is shown
    let done = panel.changed(REFRESH_INTERVAL, Change::Bluetooth(true), Ok(None));
    assert_eq!(done, reads(&[Source::Bluetooth]));
    panel.read(REFRESH_INTERVAL, Read::Bluetooth(Ok(true)));
    assert_eq!(panel.controls().tile(Module::Bluetooth), TileState::On);
}

#[test]
fn a_refused_change_disables_its_tile() {
    let mut panel = opened();
    panel.press(Module::DockHiding);
    // nothing was read meanwhile: the tile keeps the refusal until the next reading
    let effects = panel.changed(0.1, Change::DockHiding(true), Err("read-only".into()));
    assert_eq!(effects, Vec::new());
    assert_eq!(
        panel.controls().tile(Module::DockHiding),
        TileState::Disabled("read-only".into())
    );
    assert_eq!(panel.press(Module::DockHiding), Vec::new());
}

#[test]
fn switching_wi_fi_off_drops_the_network_until_the_next_reading() {
    let mut panel = opened();
    assert_eq!(
        panel.press(Module::WiFi),
        vec![Effect::Change(Change::Wifi(false))]
    );
    assert_eq!(
        panel.controls().wifi,
        Reading::Ready(WifiStatus {
            enabled: false,
            network: None
        })
    );
    assert_eq!(panel.controls().subtitle(Module::WiFi), Some("Off"));
}

#[test]
fn a_dragged_level_is_written_once_a_frame_one_write_at_a_time_the_latest_winning() {
    let mut panel = opened();
    let t = 10.0;
    assert_eq!(
        panel.slide(t, Slider::Sound, 0.2),
        vec![Effect::Change(Change::Volume(volume(0.2, false)))]
    );
    // a write is under way: the model follows the pointer, the service waits
    assert_eq!(panel.slide(t + 0.004, Slider::Sound, 0.3), Vec::new());
    assert_eq!(panel.slide(t + 0.008, Slider::Sound, 0.4), Vec::new());
    assert_eq!(panel.controls().fraction(Slider::Sound), Some(0.4));
    // a reading of the level during the drag leaves it where the pointer has it
    assert_eq!(panel.refresh(t + 0.009), {
        let mut sources = OPENING.to_vec();
        sources.retain(|s| !matches!(s, Source::Brightness | Source::Volume));
        reads(&sources)
    });
    // the write ended before a frame passed: the next one waits for the throttle
    let ended = panel.changed(
        t + 0.010,
        Change::Volume(volume(0.2, false)),
        Ok(Some(volume(0.2, false))),
    );
    assert_eq!(ended, Vec::new());
    assert_eq!(panel.controls().fraction(Slider::Sound), Some(0.4));
    let wait = panel.next_wakeup(t + 0.010).expect("a write waits");
    assert!((wait - (WRITE_INTERVAL - 0.010)).abs() < 1e-9, "{wait}");
    assert_eq!(
        panel.tick(t + WRITE_INTERVAL),
        vec![Effect::Change(Change::Volume(volume(0.4, false)))]
    );
    // letting go shows the level on the on-screen display
    assert_eq!(
        panel.release(t + 0.02),
        vec![Effect::Overlay(OverlayKind::Volume, 0.4)]
    );
}

#[test]
fn letting_go_writes_the_last_level_and_shows_mute_at_the_end_of_the_track() {
    let mut panel = opened();
    panel.slide(1.0, Slider::Sound, 0.3);
    panel.changed(1.001, Change::Volume(volume(0.3, false)), Ok(None));
    assert_eq!(panel.slide(1.002, Slider::Sound, 0.0), Vec::new());
    assert_eq!(
        panel.release(1.003),
        vec![
            Effect::Change(Change::Volume(volume(0.0, true))),
            Effect::Overlay(OverlayKind::Mute, 0.0)
        ]
    );
    panel.slide(2.0, Slider::Display, 0.7);
    assert_eq!(
        panel.release(2.1),
        vec![Effect::Overlay(OverlayKind::Brightness, 0.7)]
    );
}

#[test]
fn a_disabled_level_does_not_move() {
    let mut panel = Controller::new();
    panel.toggle(0.0);
    assert_eq!(panel.slide(0.1, Slider::Display, 0.5), Vec::new());
    assert_eq!(panel.release(0.2), Vec::new());
}

#[test]
fn unmuting_a_level_dragged_to_the_end_brings_back_an_audible_level() {
    let mut panel = Controller::new();
    panel.toggle(0.0);
    panel.read(0.0, Read::Volume(Ok(volume(0.0, true))));
    assert_eq!(
        panel.press(Module::Mute),
        vec![Effect::Change(Change::Volume(volume(0.25, false)))]
    );
    assert_eq!(panel.controls().tile(Module::Mute), TileState::Off);
}

#[test]
fn the_focus_capsule_switches_do_not_disturb_as_its_owner_published_it() {
    let mut panel = opened();
    assert_eq!(panel.press(Module::FocusMode), Vec::new());
    panel.set_focus_mode(Reading::Ready(false));
    assert_eq!(panel.controls().subtitle(Module::FocusMode), None);
    assert_eq!(
        panel.press(Module::FocusMode),
        vec![Effect::FocusMode(true)]
    );
    assert_eq!(panel.controls().title(Module::FocusMode), "Do Not Disturb");
    panel.set_focus_mode(Reading::Unavailable("the notifications stopped".into()));
    assert_eq!(panel.press(Module::FocusMode), Vec::new());
}

#[test]
fn the_lock_button_asks_for_the_lock_and_stays_an_action() {
    let mut panel = opened();
    assert_eq!(
        panel.press(Module::LockScreen),
        vec![Effect::Change(Change::Lock)]
    );
    assert_eq!(Change::Lock.source(), None);
    panel.changed(0.1, Change::Lock, Err("no session".into()));
    assert_eq!(panel.controls().tile(Module::LockScreen), TileState::Off);
}

fn network(ssid: &str) -> Network {
    Network {
        ssid: ssid.into(),
        bars: Some(3),
        secured: true,
        known: true,
        connected: false,
    }
}

#[test]
fn a_network_is_joined_by_the_services_key_and_the_radio_and_list_are_read_again() {
    let mut panel = opened();
    assert_eq!(
        panel.show_detail(Some(Detail::WiFi)),
        reads(&[Source::Networks])
    );
    let listed: Vec<Listed> = vec![(network("Cafe"), "/net/1".into())];
    panel.read(0.1, Read::Networks(Ok(listed)));
    assert_eq!(
        panel.controls().networks,
        Reading::Ready(vec![network("Cafe")])
    );
    assert_eq!(
        panel.join("Cafe"),
        vec![Effect::Change(Change::Join("/net/1".into()))]
    );
    assert_eq!(
        panel.changed(0.2, Change::Join("/net/1".into()), Ok(None)),
        reads(&[Source::Wifi, Source::Networks])
    );
    // a refused join shows on the list
    panel.read(0.3, answer(Source::Wifi));
    panel.read(0.3, answer(Source::Networks));
    panel.join("Cafe");
    panel.changed(
        0.4,
        Change::Join("/net/1".into()),
        Err("wrong passphrase".into()),
    );
    assert_eq!(
        panel.controls().networks,
        Reading::Unavailable("wrong passphrase".into())
    );
}

#[test]
fn a_device_is_connected_or_disconnected_and_read_again_whatever_the_outcome() {
    let mut panel = opened();
    assert_eq!(
        panel.show_detail(Some(Detail::Bluetooth)),
        reads(&[Source::Devices])
    );
    let device = Device {
        id: "AA:BB".into(),
        name: "Headphones".into(),
        connected: false,
    };
    panel.read(0.1, Read::Devices(Ok(vec![device])));
    assert_eq!(panel.connect("CC:DD"), Vec::new());
    assert_eq!(
        panel.connect("AA:BB"),
        vec![Effect::Change(Change::Connect("AA:BB".into(), true))]
    );
    assert_eq!(
        panel.changed(
            5.0,
            Change::Connect("AA:BB".into(), true),
            Err("timed out".into())
        ),
        reads(&[Source::Devices])
    );
}

#[test]
fn an_output_chosen_is_shown_at_once_and_the_volume_read_again() {
    let mut panel = opened();
    assert_eq!(
        panel.show_detail(Some(Detail::Sound)),
        reads(&[Source::Outputs])
    );
    let output = |id, current| Output {
        id,
        name: format!("Output {id}"),
        current,
    };
    panel.read(
        0.1,
        Read::Outputs(Ok(vec![output(1, true), output(2, false)])),
    );
    assert_eq!(panel.select_output(3), Vec::new());
    assert_eq!(
        panel.select_output(2),
        vec![Effect::Change(Change::Output(2))]
    );
    assert_eq!(
        panel.controls().outputs,
        Reading::Ready(vec![output(1, false), output(2, true)])
    );
    assert_eq!(
        panel.changed(0.2, Change::Output(2), Ok(None)),
        reads(&[Source::Volume])
    );
}

#[test]
fn the_media_buttons_go_to_the_player_of_the_last_reading() {
    let mut panel = opened();
    assert_eq!(panel.media(MediaCommand::PlayPause), Vec::new());
    let media = Media {
        title: "Song".into(),
        artist: Some("Band".into()),
        playing: true,
        can_previous: true,
        can_next: true,
        can_play_pause: true,
    };
    panel.tick(REFRESH_INTERVAL);
    let player = "org.mpris.MediaPlayer2.music".to_owned();
    panel.read(
        REFRESH_INTERVAL,
        Read::Media(Ok(Some(Playing {
            player: player.clone(),
            media,
        }))),
    );
    assert_eq!(panel.controls().tile(Module::NowPlaying), TileState::On);
    let change = Change::Media {
        player,
        command: MediaCommand::Next,
    };
    assert_eq!(
        panel.media(MediaCommand::Next),
        vec![Effect::Change(change.clone())]
    );
    assert_eq!(
        panel.changed(REFRESH_INTERVAL, change, Ok(None)),
        reads(&[Source::Media])
    );
}

#[test]
fn a_radio_that_changed_is_read_again_with_its_open_list() {
    let mut panel = opened();
    assert_eq!(
        panel.radios_changed(0.1, true, false),
        reads(&[Source::Wifi])
    );
    panel.read(0.1, answer(Source::Wifi));
    panel.show_detail(Some(Detail::Bluetooth));
    panel.read(0.1, answer(Source::Devices));
    assert_eq!(
        panel.radios_changed(0.2, false, true),
        reads(&[Source::Bluetooth, Source::Devices])
    );
    // a hidden panel reads nothing
    let mut hidden = Controller::new();
    assert_eq!(hidden.radios_changed(0.0, true, true), Vec::new());
}

#[test]
fn an_open_panel_reads_again_every_refresh_interval_and_a_hidden_one_does_not() {
    let mut panel = opened();
    assert_eq!(panel.tick(REFRESH_INTERVAL / 2.0), Vec::new());
    let wait = panel.next_wakeup(REFRESH_INTERVAL / 2.0).unwrap();
    assert!((wait - REFRESH_INTERVAL / 2.0).abs() < 1e-9, "{wait}");
    assert_eq!(panel.tick(REFRESH_INTERVAL), reads(&OPENING));
    for source in OPENING {
        panel.read(REFRESH_INTERVAL, answer(source));
    }
    panel.dismiss(1.5);
    let fade = panel.next_wakeup(1.5).expect("the fade-out ends");
    assert!(fade > 0.0);
    let gone = 1.5 + fade + 1e-9;
    assert_eq!(panel.phase(gone), Phase::Hidden);
    assert_eq!(panel.tick(gone + REFRESH_INTERVAL), Vec::new());
    assert_eq!(panel.next_wakeup(gone + REFRESH_INTERVAL), None);
}

#[test]
fn the_panel_opens_on_its_modules_whatever_it_closed_with() {
    let mut panel = opened();
    panel.show_detail(Some(Detail::Display));
    assert_eq!(panel.detail(), Some(Detail::Display));
    panel.toggle(0.1);
    panel.toggle(5.0);
    assert_eq!(panel.detail(), None);
    assert!(panel.edit_controls());
    panel.vanish();
    assert_eq!(panel.phase(5.0), Phase::Hidden);
    assert!(!panel.editing());
}

#[test]
fn edited_controls_are_shown_and_stored_when_done_and_dropped_when_cancelled() {
    let mut panel = opened();
    assert!(panel.edit_controls());
    assert!(!panel.edit_controls());
    assert!(panel.edit_remove(Module::Mute));
    assert!(!panel.arrangement().contains(Module::Mute));
    assert!(panel.cancel_edit());
    assert!(panel.arrangement().contains(Module::Mute));
    assert!(!panel.edit_remove(Module::Mute), "only while editing");

    assert!(panel.edit_controls());
    assert!(panel.edit_remove(Module::Mute));
    let ids: Vec<String> = Module::ALL
        .into_iter()
        .filter(|m| *m != Module::Mute)
        .map(|m| m.id().to_owned())
        .collect();
    assert_eq!(
        panel.finish_edit(),
        vec![Effect::Change(Change::Layout(ids.clone()))]
    );
    assert!(!panel.editing());
    assert!(!panel.arrangement().contains(Module::Mute));
    // a store that failed brings the stored controls back
    assert_eq!(
        panel.changed(0.2, Change::Layout(ids), Err("disk full".into())),
        reads(&[Source::Layout])
    );
    panel.read(0.3, Read::Layout(Ok(None)));
    assert!(panel.arrangement().contains(Module::Mute));
}

#[test]
fn a_stored_layout_is_followed_and_closes_a_view_whose_module_went() {
    let mut panel = opened();
    panel.show_detail(Some(Detail::Sound));
    panel.read(0.1, answer(Source::Outputs));
    panel.tick(REFRESH_INTERVAL);
    let stored = vec![
        "wifi".to_owned(),
        "bluetooth".to_owned(),
        "nothing".to_owned(),
    ];
    panel.read(REFRESH_INTERVAL, Read::Layout(Ok(Some(stored))));
    assert_eq!(
        panel.arrangement().shown(),
        [Module::WiFi, Module::Bluetooth]
    );
    assert_eq!(panel.detail(), None);
    // a layout that cannot be read keeps the controls shown
    panel.tick(2.0 * REFRESH_INTERVAL);
    panel.read(
        2.0 * REFRESH_INTERVAL,
        Read::Layout(Err("unreadable".into())),
    );
    assert_eq!(
        panel.arrangement().shown(),
        [Module::WiFi, Module::Bluetooth]
    );
}

#[test]
fn the_settings_application_dismisses_the_panel_once_it_started() {
    let mut panel = opened();
    assert_eq!(
        panel.open_settings("network"),
        vec![Effect::OpenSettings("network")]
    );
    panel.settings_opened(0.1, Err("not installed".into()));
    assert_eq!(panel.phase(0.1), Phase::Shown);
    panel.settings_opened(0.2, Ok(()));
    assert_eq!(panel.phase(0.2), Phase::Hiding);
}

#[test]
fn with_reduce_motion_the_panel_appears_and_goes_at_once() {
    let mut panel = Controller::new();
    panel.set_reduce_motion(true);
    panel.toggle(0.0);
    assert_eq!(panel.opacity(0.0), 1.0);
    assert!(!panel.opening(0.0));
    panel.dismiss(0.1);
    assert_eq!(panel.phase(0.1), Phase::Hidden);
}

#[test]
fn every_failed_reading_is_of_its_source_and_every_change_names_what_it_changes() {
    let sources = [
        Source::Wifi,
        Source::Bluetooth,
        Source::Microphone,
        Source::DockHiding,
        Source::Keyboard,
        Source::Brightness,
        Source::Volume,
        Source::Media,
        Source::Networks,
        Source::Devices,
        Source::Outputs,
        Source::Layout,
    ];
    for source in sources {
        assert_eq!(Read::failed(source, "x".into()).source(), source);
        assert_eq!(answer(source).source(), source);
        assert!(!source.what().is_empty());
    }
    assert_eq!(Change::Join("k".into()).source(), Some(Source::Networks));
    assert_eq!(
        Change::Connect("k".into(), true).source(),
        Some(Source::Devices)
    );
    assert_eq!(Change::Output(1).source(), Some(Source::Outputs));
    assert_eq!(Change::Brightness(0.5).what(), "display brightness");
    assert_eq!(Change::Lock.what(), "lock screen");
}
