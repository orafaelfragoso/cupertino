//! The volume keys on the default PipeWire sink: after each press the overlay shows what `wpctl get-volume` reports.

use cupertino_osd::{
    AlsaVolume, BackendError, Backlight, Command, Controller, Kind, MemoryBacklight, SessionVolume,
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

fn press(controller: &mut Controller, command: Command) -> (Kind, f64) {
    controller.apply(1.0, command).unwrap();
    let overlay = controller.view(1.0).expect("an overlay is shown").overlay;
    (overlay.kind(), overlay.level())
}

#[test]
fn each_volume_key_changes_the_default_sink_to_the_level_shown() {
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

    assert_eq!(press(&mut controller, up), (Kind::Volume, 0.56));
    assert_eq!(fake.volume(Target::DefaultSink).level, 0.56);
    assert_eq!(press(&mut controller, up), (Kind::Volume, 0.62));
    assert_eq!(fake.volume(Target::DefaultSink).level, 0.62);
    assert_eq!(press(&mut controller, fine_down), (Kind::Volume, 0.61));
    assert_eq!(fake.volume(Target::DefaultSink).level, 0.61);

    assert_eq!(
        press(&mut controller, Command::ToggleMute),
        (Kind::Mute, 0.0)
    );
    assert_eq!(
        fake.volume(Target::DefaultSink),
        Volume {
            level: 0.61,
            muted: true
        }
    );
    assert_eq!(
        fake.volume(Target::DefaultSource),
        half,
        "the source is untouched"
    );
}
