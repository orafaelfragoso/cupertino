//! Offers, negotiation and the params a stream publishes, without a PipeWire daemon.

use cupertino_portal::screencast::format::{
    buffers, enum_format, metas, negotiate, Fixed, Format, Mismatch, Offer, Pixel, Rate, Size,
    DAMAGE_REGIONS, DEFAULT_RATE, MAX_RATE,
};
use pipewire::spa;
use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use spa::param::video::VideoFormat;
use spa::param::ParamType;
use spa::pod::deserialize::PodDeserializer;
use spa::pod::serialize::PodSerializer;
use spa::pod::{object, property, ChoiceValue, Pod, Value};
use spa::utils::{ChoiceEnum, Fraction, Id, Rectangle, SpaTypes};
use std::time::Duration;

fn size(width: u32, height: u32) -> Size {
    Size { width, height }
}

fn rate(num: u32, denom: u32) -> Rate {
    Rate { num, denom }
}

fn offer() -> Offer {
    Offer {
        size: size(1920, 1080),
        pixels: vec![Pixel::Bgrx, Pixel::Bgra],
    }
}

fn fixed(format: VideoFormat, framerate: Rate, max_framerate: Rate) -> Fixed {
    Fixed {
        format,
        size: size(1920, 1080),
        framerate,
        max_framerate,
    }
}

#[test]
fn a_frame_of_four_byte_pixels_has_a_stride_and_a_length() {
    let format = Format {
        width: 1920,
        height: 1080,
        pixel: Pixel::Bgrx,
        framerate: rate(30, 1),
    };
    assert_eq!(format.stride(), 7680);
    assert_eq!(format.frame_len(), 7680 * 1080);
}

#[test]
fn a_size_is_shareable_when_its_frame_is_not_empty_and_fits_a_wayland_pool() {
    for (width, height, expected) in [
        (1920, 1080, true),
        (1, 1, true),
        (0, 1080, false),
        (1920, 0, false),
        // 2^31 - 4 bytes is the largest pool of four-byte pixels in whole pixels
        (536_870_911, 1, true),
        (536_870_912, 1, false),
        // the product of a huge size must not wrap into a small one
        (u32::MAX, u32::MAX, false),
        (65_536, 65_537, false),
    ] {
        assert_eq!(
            size(width, height).shareable(),
            expected,
            "{width}x{height}"
        );
    }
}

#[test]
fn the_time_between_frames_is_the_inverse_of_the_rate() {
    for (num, denom, expected) in [
        (60, 1, Duration::from_nanos(16_666_666)),
        (30, 1, Duration::from_nanos(33_333_333)),
        (1, 1, Duration::from_secs(1)),
        (2, 4, Duration::from_secs(2)),
    ] {
        assert_eq!(rate(num, denom).interval(), expected, "{num}/{denom}");
    }
}

#[test]
fn a_fixed_format_that_matches_the_offer_becomes_the_stream_format_at_the_negotiated_rate() {
    assert_eq!(
        negotiate(&offer(), fixed(VideoFormat::BGRA, rate(0, 1), rate(25, 1))),
        Ok(Format {
            width: 1920,
            height: 1080,
            pixel: Pixel::Bgra,
            framerate: rate(25, 1)
        })
    );
    for (name, framerate, max_framerate, expected) in [
        ("the maximum wins", rate(10, 1), rate(24, 1), rate(24, 1)),
        ("no maximum", rate(10, 1), rate(0, 1), rate(10, 1)),
        ("a maximum over zero", rate(10, 1), rate(5, 0), rate(10, 1)),
        ("no rate", rate(0, 1), rate(0, 1), rate(DEFAULT_RATE, 1)),
        (
            "above the limit",
            rate(0, 1),
            rate(144, 1),
            rate(MAX_RATE, 1),
        ),
        (
            "a fraction above",
            rate(0, 1),
            rate(121, 2),
            rate(MAX_RATE, 1),
        ),
        ("a fraction at", rate(0, 1), rate(120, 2), rate(120, 2)),
        ("at the limit", rate(0, 1), rate(60, 1), rate(60, 1)),
    ] {
        let format = negotiate(&offer(), fixed(VideoFormat::BGRx, framerate, max_framerate));
        assert_eq!(format.map(|f| f.framerate), Ok(expected), "{name}");
    }
}

#[test]
fn a_fixed_format_the_offer_cannot_serve_is_refused_with_what_differs() {
    let narrow = Offer {
        pixels: vec![Pixel::Bgrx],
        ..offer()
    };
    let mut other_size = fixed(VideoFormat::BGRx, rate(30, 1), rate(0, 1));
    other_size.size = size(1280, 720);
    for (name, offer, fixed, expected) in [
        (
            "a layout not offered",
            narrow,
            fixed(VideoFormat::BGRA, rate(30, 1), rate(0, 1)),
            Mismatch::Pixel(VideoFormat::BGRA.as_raw()),
        ),
        (
            "a layout never offered",
            offer(),
            fixed(VideoFormat::RGB, rate(30, 1), rate(0, 1)),
            Mismatch::Pixel(VideoFormat::RGB.as_raw()),
        ),
        (
            "another size",
            offer(),
            other_size,
            Mismatch::Size(size(1280, 720)),
        ),
    ] {
        assert_eq!(negotiate(&offer, fixed), Err(expected), "{name}");
    }
}

fn serialize(value: &Value) -> Vec<u8> {
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), value)
        .unwrap()
        .0
        .into_inner()
}

/// The serialized `Format` object a consumer fixes.
fn format_param(media_type: MediaType, subtype: MediaSubtype) -> Vec<u8> {
    serialize(&Value::Object(object!(
        SpaTypes::ObjectParamFormat,
        ParamType::Format,
        property!(FormatProperties::MediaType, Id, media_type),
        property!(FormatProperties::MediaSubtype, Id, subtype),
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
            Fraction { num: 45, denom: 1 }
        ),
    )))
}

#[test]
fn only_a_raw_video_format_object_is_read_as_a_fixed_format() {
    let raw = Fixed {
        format: VideoFormat::BGRx,
        size: size(1920, 1080),
        framerate: rate(0, 1),
        max_framerate: rate(45, 1),
    };
    for (name, bytes, expected) in [
        (
            "raw video",
            format_param(MediaType::Video, MediaSubtype::Raw),
            Some(raw),
        ),
        (
            "audio",
            format_param(MediaType::Audio, MediaSubtype::Raw),
            None,
        ),
        (
            "encoded video",
            format_param(MediaType::Video, MediaSubtype::H264),
            None,
        ),
        ("not an object", serialize(&Value::Int(3)), None),
    ] {
        assert_eq!(
            Fixed::parse(Pod::from_bytes(&bytes).unwrap()),
            expected,
            "{name}"
        );
    }
}

fn deserialize(bytes: &[u8]) -> spa::pod::Object {
    match PodDeserializer::deserialize_any_from(bytes).unwrap().1 {
        Value::Object(object) => object,
        other => panic!("not an object: {other:?}"),
    }
}

fn property(object: &spa::pod::Object, key: u32) -> &Value {
    &object
        .properties
        .iter()
        .find(|p| p.key == key)
        .unwrap_or_else(|| panic!("no property {key}"))
        .value
}

#[test]
fn the_enum_format_lists_the_offered_layouts_in_order_at_the_exact_size_and_a_variable_rate() {
    let object = deserialize(&enum_format(&offer()));
    assert_eq!(object.type_, SpaTypes::ObjectParamFormat.as_raw());
    assert_eq!(object.id, ParamType::EnumFormat.as_raw());
    assert_eq!(
        property(&object, FormatProperties::MediaType.as_raw()),
        &Value::Id(Id(MediaType::Video.as_raw()))
    );
    assert_eq!(
        property(&object, FormatProperties::MediaSubtype.as_raw()),
        &Value::Id(Id(MediaSubtype::Raw.as_raw()))
    );
    assert_eq!(
        property(&object, FormatProperties::VideoSize.as_raw()),
        &Value::Rectangle(Rectangle {
            width: 1920,
            height: 1080
        })
    );
    let Value::Choice(ChoiceValue::Id(choice)) =
        property(&object, FormatProperties::VideoFormat.as_raw())
    else {
        panic!("the layouts are a choice of ids");
    };
    let ChoiceEnum::Enum {
        default,
        alternatives,
    } = &choice.1
    else {
        panic!("an enumeration");
    };
    assert_eq!(*default, Id(VideoFormat::BGRx.as_raw()));
    assert_eq!(
        alternatives,
        &[
            Id(VideoFormat::BGRx.as_raw()),
            Id(VideoFormat::BGRA.as_raw())
        ]
    );

    assert_eq!(
        property(&object, FormatProperties::VideoFramerate.as_raw()),
        &Value::Fraction(Fraction { num: 0, denom: 1 })
    );
    let Value::Choice(ChoiceValue::Fraction(choice)) =
        property(&object, FormatProperties::VideoMaxFramerate.as_raw())
    else {
        panic!("the maximum rate is a choice of fractions");
    };
    let ChoiceEnum::Range { default, min, max } = &choice.1 else {
        panic!("a range");
    };
    assert_eq!(
        (*default, *min, *max),
        (
            Fraction {
                num: DEFAULT_RATE,
                denom: 1
            },
            Fraction { num: 1, denom: 1 },
            Fraction {
                num: MAX_RATE,
                denom: 1
            }
        )
    );
}

#[test]
fn the_buffers_param_asks_for_one_memfd_block_of_one_frame() {
    let format = Format {
        width: 640,
        height: 480,
        pixel: Pixel::Bgra,
        framerate: rate(30, 1),
    };
    let object = deserialize(&buffers(&format));
    assert_eq!(object.id, ParamType::Buffers.as_raw());
    assert_eq!(
        property(&object, spa::sys::SPA_PARAM_BUFFERS_blocks),
        &Value::Int(1)
    );
    assert_eq!(
        property(&object, spa::sys::SPA_PARAM_BUFFERS_size),
        &Value::Int(640 * 4 * 480)
    );
    assert_eq!(
        property(&object, spa::sys::SPA_PARAM_BUFFERS_stride),
        &Value::Int(640 * 4)
    );
    let Value::Choice(ChoiceValue::Int(count)) =
        property(&object, spa::sys::SPA_PARAM_BUFFERS_buffers)
    else {
        panic!("the buffer count is a choice");
    };
    let ChoiceEnum::Range { default, min, max } = &count.1 else {
        panic!("a range");
    };
    assert_eq!((*default, *min, *max), (4, 2, 8));
    let Value::Choice(ChoiceValue::Int(types)) =
        property(&object, spa::sys::SPA_PARAM_BUFFERS_dataType)
    else {
        panic!("the data type is a choice");
    };
    let ChoiceEnum::Flags { default, flags } = &types.1 else {
        panic!("flags");
    };
    assert_eq!(*default, 1 << spa::sys::SPA_DATA_MemFd);
    assert_eq!(flags, &[1 << spa::sys::SPA_DATA_MemFd]);
}

#[test]
fn the_meta_params_are_a_header_and_room_for_the_damage_of_a_frame() {
    let [header, damage] = metas();
    let header = deserialize(&header);
    let damage = deserialize(&damage);
    assert_eq!(header.id, ParamType::Meta.as_raw());
    assert_eq!(
        property(&header, spa::sys::SPA_PARAM_META_type),
        &Value::Id(Id(spa::sys::SPA_META_Header))
    );
    assert_eq!(
        property(&header, spa::sys::SPA_PARAM_META_size),
        &Value::Int(std::mem::size_of::<spa::sys::spa_meta_header>() as i32)
    );
    assert_eq!(
        property(&damage, spa::sys::SPA_PARAM_META_type),
        &Value::Id(Id(spa::sys::SPA_META_VideoDamage))
    );
    assert_eq!(
        property(&damage, spa::sys::SPA_PARAM_META_size),
        &Value::Int((std::mem::size_of::<spa::sys::spa_meta_region>() * DAMAGE_REGIONS) as i32)
    );
}
