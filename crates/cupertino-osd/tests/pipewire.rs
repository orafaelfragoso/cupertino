//! The volume keys on the default PipeWire sink and the microphone key on the default source: after each press the
//! overlay shows the level stepped to, which the sink takes once the background write is done.

use cupertino_osd::{
    AlsaVolume, BackendError, Backlight, Command, Controller, Kind, MemoryBacklight,
    SessionMicrophone, SessionVolume,
};
use cupertino_pipewire::fake::FakeWpctl;
use cupertino_pipewire::{Target, Volume, Wpctl};

struct Absent;

impl Backlight for Absent {
    fn level(&mut self) -> Result<f64, BackendError> {
        Err("no backlight device".into())
    }

    fn set_level(&mut self, _: f64) -> Result<f64, BackendError> {
        Err("no backlight device".into())
    }
}

/// The sink's volume once it is `expected`, or what it is after 5 s.
fn sink_settles(fake: &FakeWpctl, expected: Volume) -> Volume {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let sink = fake.volume(Target::DefaultSink);
        if sink == expected || std::time::Instant::now() > deadline {
            return sink;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn press(controller: &mut Controller, command: Command) -> (Kind, f64) {
    controller.apply(1.0, command).unwrap();
    let overlay = controller.view(1.0).expect("an overlay is shown").overlay;
    (overlay.kind(), overlay.level())
}

#[test]
fn each_volume_key_changes_the_default_sink_to_the_level_shown_and_the_microphone_key_the_source() {
    let half = Volume {
        level: 0.5,
        muted: false,
    };
    let fake = FakeWpctl::new(half, half);
    let alsa = tempfile::tempdir().unwrap();
    let mut controller = Controller::new(
        Box::new(SessionVolume::new(
            Wpctl::new(fake.program()),
            AlsaVolume::new(alsa.path().to_path_buf(), None),
        )),
        Box::new(SessionMicrophone::new(Wpctl::new(fake.program()))),
        Box::new(MemoryBacklight::new(0.5)),
        Box::new(Absent),
    );
    let up = Command::VolumeStep {
        direction: 1,
        fine: false,
    };
    let fine_down = Command::VolumeStep {
        direction: -1,
        fine: true,
    };

    let sink = |level| Volume {
        level,
        muted: false,
    };
    assert_eq!(press(&mut controller, up), (Kind::Volume, 0.5625));
    assert_eq!(press(&mut controller, up), (Kind::Volume, 0.625));
    assert_eq!(press(&mut controller, fine_down), (Kind::Volume, 0.609375));
    assert_eq!(
        sink_settles(&fake, sink(0.61)),
        sink(0.61),
        "the presses step from the level asked, not from the sink's rounded reading"
    );

    assert_eq!(
        press(&mut controller, Command::ToggleMute),
        (Kind::Mute, 0.0)
    );
    let muted = Volume {
        level: 0.61,
        muted: true,
    };
    assert_eq!(sink_settles(&fake, muted), muted);
    assert_eq!(
        fake.volume(Target::DefaultSource),
        half,
        "the source is untouched"
    );

    assert_eq!(
        press(&mut controller, Command::ToggleMicrophoneMute),
        (Kind::MicrophoneMute, 0.0)
    );
    assert_eq!(
        fake.volume(Target::DefaultSource),
        Volume {
            level: 0.5,
            muted: true
        }
    );
    assert_eq!(
        press(&mut controller, Command::ToggleMicrophoneMute),
        (Kind::Microphone, 0.5)
    );
    assert_eq!(fake.volume(Target::DefaultSource), half);
    assert!(
        fake.volume(Target::DefaultSink).muted,
        "the output stays muted"
    );
}
