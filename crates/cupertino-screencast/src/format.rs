//! What a stream offers and what the consumer fixed: the pixel layouts, the size of the source and the frame rate.

use std::time::Duration;

use pipewire::spa;
use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use spa::param::video::{VideoFormat, VideoInfoRaw};
use spa::param::ParamType;
use spa::pod::serialize::PodSerializer;
use spa::pod::{object, property, ChoiceValue, Object, Pod, Property, Value};
use spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Fraction, Id, Rectangle, SpaTypes};

/// The frame rate a consumer gets when it names none.
pub const DEFAULT_RATE: u32 = 30;
/// The highest frame rate a stream offers.
pub const MAX_RATE: u32 = 60;
/// Buffers a stream asks for, and the most it takes.
const BUFFERS: i32 = 4;
const MAX_BUFFERS: i32 = 8;
/// Damage rectangles one buffer carries; more are sent as one rectangle around the frame.
pub const DAMAGE_REGIONS: usize = 16;

/// A pixel layout PipeWire and `wl_shm` share, four bytes a pixel, named by its byte order in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pixel {
    /// Blue, green, red and an unused byte: `wl_shm` XRGB8888.
    Bgrx,
    /// Blue, green, red and alpha: `wl_shm` ARGB8888.
    Bgra,
}

impl Pixel {
    fn video_format(self) -> VideoFormat {
        match self {
            Pixel::Bgrx => VideoFormat::BGRx,
            Pixel::Bgra => VideoFormat::BGRA,
        }
    }

    fn from_video_format(format: VideoFormat) -> Option<Pixel> {
        [Pixel::Bgrx, Pixel::Bgra]
            .into_iter()
            .find(|p| p.video_format() == format)
    }
}

/// A size in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    /// Whether a frame of this size, four bytes a pixel, is a buffer Wayland can share: not empty, and at most
    /// `i32::MAX` bytes (the size of a `wl_shm` pool).
    pub fn shareable(self) -> bool {
        let bytes = u64::from(self.width) * 4 * u64::from(self.height);
        bytes > 0 && bytes <= i32::MAX as u64
    }
}

/// Frames a second, as a fraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    pub num: u32,
    pub denom: u32,
}

impl Rate {
    /// The time between two frames.
    pub fn interval(self) -> Duration {
        Duration::from_secs(u64::from(self.denom)) / self.num
    }
}

/// What the source of a stream delivers: its size and the pixel layouts it can be captured in, the preferred first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub size: Size,
    pub pixels: Vec<Pixel>,
}

/// The format a consumer fixed for a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub width: u32,
    pub height: u32,
    pub pixel: Pixel,
    /// The most frames a second the consumer takes; frames come only when the source changes.
    pub framerate: Rate,
}

impl Format {
    pub fn stride(&self) -> u32 {
        self.width * 4
    }

    /// Bytes of one frame.
    pub fn frame_len(&self) -> u32 {
        self.stride() * self.height
    }
}

/// Why a fixed format is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mismatch {
    /// A pixel layout the offer does not list.
    Pixel(u32),
    /// Another size than the source's, such as one fixed before the source was resized.
    Size(Size),
    /// Not a raw video format, or one PipeWire could not read.
    Unreadable,
}

/// What the consumer fixed, as PipeWire reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fixed {
    pub format: VideoFormat,
    pub size: Size,
    pub framerate: Rate,
    pub max_framerate: Rate,
}

impl Fixed {
    /// The fixed video format of a `Format` param; `None` when it is not raw video.
    pub fn parse(param: &Pod) -> Option<Fixed> {
        let (media_type, media_subtype) = spa::param::format_utils::parse_format(param).ok()?;
        if media_type != MediaType::Video || media_subtype != MediaSubtype::Raw {
            return None;
        }
        let mut info = VideoInfoRaw::new();
        info.parse(param).ok()?;
        let rate = |f: Fraction| Rate {
            num: f.num,
            denom: f.denom,
        };
        Some(Fixed {
            format: info.format(),
            size: Size {
                width: info.size().width,
                height: info.size().height,
            },
            framerate: rate(info.framerate()),
            max_framerate: rate(info.max_framerate()),
        })
    }
}

/// The stream's format once the consumer fixed `fixed` against `offer`. The frame rate is the consumer's highest
/// (a variable rate names it as the maximum), or [`DEFAULT_RATE`] when it names none, and at most [`MAX_RATE`].
pub fn negotiate(offer: &Offer, fixed: Fixed) -> Result<Format, Mismatch> {
    let pixel = Pixel::from_video_format(fixed.format)
        .filter(|p| offer.pixels.contains(p))
        .ok_or(Mismatch::Pixel(fixed.format.as_raw()))?;
    if fixed.size != offer.size {
        return Err(Mismatch::Size(fixed.size));
    }
    let named = |r: Rate| (r.num > 0 && r.denom > 0).then_some(r);
    let framerate = named(fixed.max_framerate)
        .or(named(fixed.framerate))
        .map_or(
            Rate {
                num: DEFAULT_RATE,
                denom: 1,
            },
            |r| {
                if u64::from(r.num) > u64::from(MAX_RATE) * u64::from(r.denom) {
                    Rate {
                        num: MAX_RATE,
                        denom: 1,
                    }
                } else {
                    r
                }
            },
        );
    Ok(Format {
        width: offer.size.width,
        height: offer.size.height,
        pixel,
        framerate,
    })
}

fn bytes(object: Object) -> Vec<u8> {
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object))
        .expect("a pod of fixed fields serializes")
        .0
        .into_inner()
}

/// The `EnumFormat` param of `offer`: its pixel layouts at its exact size, a variable frame rate up to [`MAX_RATE`].
pub fn enum_format(offer: &Offer) -> Vec<u8> {
    let formats: Vec<Id> = offer
        .pixels
        .iter()
        .map(|p| Id(p.video_format().as_raw()))
        .collect();
    let format = Property::new(
        FormatProperties::VideoFormat.as_raw(),
        Value::Choice(ChoiceValue::Id(Choice(
            ChoiceFlags::empty(),
            ChoiceEnum::Enum {
                default: formats[0],
                alternatives: formats,
            },
        ))),
    );
    bytes(object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        property!(FormatProperties::MediaType, Id, MediaType::Video),
        property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        format,
        property!(
            FormatProperties::VideoSize,
            Rectangle,
            Rectangle {
                width: offer.size.width,
                height: offer.size.height,
            }
        ),
        property!(
            FormatProperties::VideoFramerate,
            Fraction,
            Fraction { num: 0, denom: 1 }
        ),
        property!(
            FormatProperties::VideoMaxFramerate,
            Choice,
            Range,
            Fraction,
            Fraction {
                num: DEFAULT_RATE,
                denom: 1
            },
            Fraction { num: 1, denom: 1 },
            Fraction {
                num: MAX_RATE,
                denom: 1
            }
        ),
    ))
}

fn int(key: u32, value: i32) -> Property {
    Property::new(key, Value::Int(value))
}

/// The `Buffers` param of `format`: memfd buffers of one frame each, which the stream allocates.
pub fn buffers(format: &Format) -> Vec<u8> {
    let buffers = Property::new(
        spa::sys::SPA_PARAM_BUFFERS_buffers,
        Value::Choice(ChoiceValue::Int(Choice(
            ChoiceFlags::empty(),
            ChoiceEnum::Range {
                default: BUFFERS,
                min: 2,
                max: MAX_BUFFERS,
            },
        ))),
    );
    bytes(Object {
        type_: SpaTypes::ObjectParamBuffers.as_raw(),
        id: ParamType::Buffers.as_raw(),
        properties: vec![
            buffers,
            int(spa::sys::SPA_PARAM_BUFFERS_blocks, 1),
            int(spa::sys::SPA_PARAM_BUFFERS_size, format.frame_len() as i32),
            int(spa::sys::SPA_PARAM_BUFFERS_stride, format.stride() as i32),
            Property::new(
                spa::sys::SPA_PARAM_BUFFERS_dataType,
                // Consumers may offer several memory types; negotiate their common bits.
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Flags {
                        default: 1 << spa::sys::SPA_DATA_MemFd,
                        flags: vec![1 << spa::sys::SPA_DATA_MemFd],
                    },
                ))),
            ),
        ],
    })
}

fn meta(kind: u32, size: usize) -> Vec<u8> {
    bytes(Object {
        type_: SpaTypes::ObjectParamMeta.as_raw(),
        id: ParamType::Meta.as_raw(),
        properties: vec![
            Property::new(spa::sys::SPA_PARAM_META_type, Value::Id(Id(kind))),
            int(spa::sys::SPA_PARAM_META_size, size as i32),
        ],
    })
}

/// The `Meta` params: a header with the timestamp, and the damage of each frame.
pub fn metas() -> [Vec<u8>; 2] {
    [
        meta(
            spa::sys::SPA_META_Header,
            std::mem::size_of::<spa::sys::spa_meta_header>(),
        ),
        meta(
            spa::sys::SPA_META_VideoDamage,
            std::mem::size_of::<spa::sys::spa_meta_region>() * DAMAGE_REGIONS,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Size = Size {
        width: 1920,
        height: 1080,
    };

    fn offer() -> Offer {
        Offer {
            size: SIZE,
            pixels: vec![Pixel::Bgrx, Pixel::Bgra],
        }
    }

    fn fixed(format: VideoFormat, size: Size, framerate: (u32, u32), max: (u32, u32)) -> Fixed {
        Fixed {
            format,
            size,
            framerate: Rate {
                num: framerate.0,
                denom: framerate.1,
            },
            max_framerate: Rate {
                num: max.0,
                denom: max.1,
            },
        }
    }

    #[test]
    fn a_frame_past_the_largest_shm_pool_or_an_empty_one_is_not_shareable() {
        let size = |width, height| Size { width, height };
        assert!(SIZE.shareable());
        assert!(size(16384, 32767).shareable());
        assert!(!size(16384, 32768).shareable());
        assert!(!size(65536, 65536).shareable());
        assert!(!size(0, 1080).shareable());
    }

    #[test]
    fn a_variable_rate_takes_the_consumers_maximum_and_none_takes_30() {
        let format = negotiate(&offer(), fixed(VideoFormat::BGRA, SIZE, (0, 1), (24, 1))).unwrap();
        assert_eq!(
            format,
            Format {
                width: 1920,
                height: 1080,
                pixel: Pixel::Bgra,
                framerate: Rate { num: 24, denom: 1 },
            }
        );
        assert_eq!(format.stride(), 7680);
        assert_eq!(format.frame_len(), 7680 * 1080);
        let unnamed = negotiate(&offer(), fixed(VideoFormat::BGRx, SIZE, (0, 1), (0, 1))).unwrap();
        assert_eq!(unnamed.framerate, Rate { num: 30, denom: 1 });
        assert_eq!(
            unnamed.framerate.interval(),
            Duration::from_nanos(33_333_333)
        );
        let fixed_rate = negotiate(
            &offer(),
            fixed(VideoFormat::BGRx, SIZE, (60000, 1001), (0, 1)),
        )
        .unwrap();
        assert_eq!(
            fixed_rate.framerate,
            Rate {
                num: 60000,
                denom: 1001
            }
        );
    }

    #[test]
    fn a_rate_over_60_is_cut_to_60() {
        let format = negotiate(&offer(), fixed(VideoFormat::BGRx, SIZE, (0, 1), (144, 1))).unwrap();
        assert_eq!(format.framerate, Rate { num: 60, denom: 1 });
        assert_eq!(
            format.framerate.interval(),
            Duration::from_nanos(16_666_666)
        );
    }

    #[test]
    fn a_layout_or_size_the_offer_lacks_is_refused() {
        let opaque = Offer {
            size: SIZE,
            pixels: vec![Pixel::Bgrx],
        };
        assert_eq!(
            negotiate(&opaque, fixed(VideoFormat::BGRA, SIZE, (0, 1), (30, 1))),
            Err(Mismatch::Pixel(VideoFormat::BGRA.as_raw()))
        );
        assert_eq!(
            negotiate(&offer(), fixed(VideoFormat::RGBA, SIZE, (0, 1), (30, 1))),
            Err(Mismatch::Pixel(VideoFormat::RGBA.as_raw()))
        );
        let before_resize = Size {
            width: 1280,
            height: 720,
        };
        assert_eq!(
            negotiate(
                &offer(),
                fixed(VideoFormat::BGRx, before_resize, (0, 1), (30, 1))
            ),
            Err(Mismatch::Size(before_resize))
        );
    }

    #[test]
    fn the_offer_reads_back_as_the_formats_a_consumer_can_fix() {
        let pod = enum_format(&offer());
        let pod = Pod::from_bytes(&pod).unwrap();
        let (_, Value::Object(object)) =
            spa::pod::deserialize::PodDeserializer::deserialize_any_from(pod.as_bytes()).unwrap()
        else {
            panic!("an object");
        };
        let value = |key: FormatProperties| {
            object
                .properties
                .iter()
                .find(|p| p.key == key.as_raw())
                .map(|p| p.value.clone())
                .unwrap()
        };
        assert_eq!(
            value(FormatProperties::VideoFormat),
            Value::Choice(ChoiceValue::Id(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Enum {
                    default: Id(VideoFormat::BGRx.as_raw()),
                    alternatives: vec![
                        Id(VideoFormat::BGRx.as_raw()),
                        Id(VideoFormat::BGRA.as_raw())
                    ],
                }
            )))
        );
        assert_eq!(
            value(FormatProperties::VideoSize),
            Value::Rectangle(Rectangle {
                width: 1920,
                height: 1080
            })
        );
        assert_eq!(
            value(FormatProperties::VideoMaxFramerate),
            Value::Choice(ChoiceValue::Fraction(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Range {
                    default: Fraction { num: 30, denom: 1 },
                    min: Fraction { num: 1, denom: 1 },
                    max: Fraction { num: 60, denom: 1 },
                }
            )))
        );
    }

    #[test]
    fn a_fixed_format_param_parses_to_what_negotiate_reads() {
        let pod = bytes(object!(
            SpaTypes::ObjectParamFormat,
            ParamType::Format,
            property!(FormatProperties::MediaType, Id, MediaType::Video),
            property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
            property!(FormatProperties::VideoFormat, Id, VideoFormat::BGRx),
            property!(
                FormatProperties::VideoSize,
                Rectangle,
                Rectangle {
                    width: 1920,
                    height: 1080
                }
            ),
            property!(
                FormatProperties::VideoFramerate,
                Fraction,
                Fraction { num: 0, denom: 1 }
            ),
            property!(
                FormatProperties::VideoMaxFramerate,
                Fraction,
                Fraction { num: 30, denom: 1 }
            ),
        ));
        let fixed = Fixed::parse(Pod::from_bytes(&pod).unwrap()).unwrap();
        assert_eq!(
            negotiate(&offer(), fixed).unwrap(),
            Format {
                width: 1920,
                height: 1080,
                pixel: Pixel::Bgrx,
                framerate: Rate { num: 30, denom: 1 },
            }
        );
    }
}
