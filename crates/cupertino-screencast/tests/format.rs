//! Offers, negotiation and the params a stream publishes, without a PipeWire daemon.

use cupertino_screencast::format::{
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
    assert!(size(1920, 1080).shareable());
    assert!(size(1, 1).shareable());
    assert!(!size(0, 1080).shareable());
    assert!(!size(1920, 0).shareable());
    // 2^31 - 4 bytes is the largest pool of four-byte pixels in whole pixels
    assert!(size(536_870_911, 1).shareable());
    assert!(!size(536_870_912, 1).shareable());
    assert!(!size(u32::MAX, u32::MAX).shareable());
}

#[test]
fn the_time_between_frames_is_the_inverse_of_the_rate() {
    assert_eq!(rate(60, 1).interval(), Duration::from_nanos(16_666_666));
    assert_eq!(rate(30, 1).interval(), Duration::from_nanos(33_333_333));
    assert_eq!(rate(1, 1).interval(), Duration::from_secs(1));
    assert_eq!(rate(2, 4).interval(), Duration::from_secs(2));
}

#[test]
fn a_fixed_format_that_matches_the_offer_becomes_the_stream_format() {
    let format = negotiate(&offer(), fixed(VideoFormat::BGRA, rate(0, 1), rate(25, 1))).unwrap();
    assert_eq!(
        format,
        Format {
            width: 1920,
            height: 1080,
            pixel: Pixel::Bgra,
            framerate: rate(25, 1)
        }
    );
}

#[test]
fn the_maximum_rate_wins_over_the_rate_and_a_missing_maximum_falls_back_to_it() {
    let negotiated = |framerate, max_framerate| {
        negotiate(&offer(), fixed(VideoFormat::BGRx, framerate, max_framerate))
            .unwrap()
            .framerate
    };
    assert_eq!(negotiated(rate(10, 1), rate(24, 1)), rate(24, 1));
    assert_eq!(negotiated(rate(10, 1), rate(0, 1)), rate(10, 1));
    assert_eq!(negotiated(rate(10, 1), rate(5, 0)), rate(10, 1));
}

#[test]
fn a_consumer_that_names_no_rate_gets_the_default() {
    let format = negotiate(&offer(), fixed(VideoFormat::BGRx, rate(0, 1), rate(0, 1))).unwrap();
    assert_eq!(format.framerate, rate(DEFAULT_RATE, 1));
    assert_eq!(DEFAULT_RATE, 30);
}

#[test]
fn a_rate_above_the_limit_is_lowered_to_it() {
    let negotiated = |max_framerate| {
        negotiate(
            &offer(),
            fixed(VideoFormat::BGRx, rate(0, 1), max_framerate),
        )
        .unwrap()
        .framerate
    };
    assert_eq!(negotiated(rate(144, 1)), rate(MAX_RATE, 1));
    assert_eq!(negotiated(rate(121, 2)), rate(MAX_RATE, 1));
    assert_eq!(negotiated(rate(120, 2)), rate(120, 2));
    assert_eq!(negotiated(rate(60, 1)), rate(60, 1));
    assert_eq!(MAX_RATE, 60);
}

#[test]
fn a_pixel_layout_the_offer_lacks_is_refused_with_its_number() {
    let narrow = Offer {
        pixels: vec![Pixel::Bgrx],
        ..offer()
    };
    assert_eq!(
        negotiate(&narrow, fixed(VideoFormat::BGRA, rate(30, 1), rate(0, 1))),
        Err(Mismatch::Pixel(VideoFormat::BGRA.as_raw()))
    );
    assert_eq!(
        negotiate(&offer(), fixed(VideoFormat::RGB, rate(30, 1), rate(0, 1))),
        Err(Mismatch::Pixel(VideoFormat::RGB.as_raw()))
    );
}

#[test]
fn a_size_other_than_the_sources_is_refused_with_that_size() {
    let mut other = fixed(VideoFormat::BGRx, rate(30, 1), rate(0, 1));
    other.size = size(1280, 720);
    assert_eq!(
        negotiate(&offer(), other),
        Err(Mismatch::Size(size(1280, 720)))
    );
}

/// The serialized `Format` object a consumer fixes.
fn format_param(
    media_type: MediaType,
    subtype: MediaSubtype,
    format: VideoFormat,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let object = object!(
        SpaTypes::ObjectParamFormat,
        ParamType::Format,
        property!(FormatProperties::MediaType, Id, media_type),
        property!(FormatProperties::MediaSubtype, Id, subtype),
        property!(FormatProperties::VideoFormat, Id, format),
        property!(
            FormatProperties::VideoSize,
            Rectangle,
            Rectangle { width, height }
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
    );
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object))
        .unwrap()
        .0
        .into_inner()
}

#[test]
fn a_fixed_raw_video_format_is_read_from_its_param() {
    let bytes = format_param(
        MediaType::Video,
        MediaSubtype::Raw,
        VideoFormat::BGRx,
        1920,
        1080,
    );
    let pod = Pod::from_bytes(&bytes).unwrap();
    assert_eq!(
        Fixed::parse(pod),
        Some(Fixed {
            format: VideoFormat::BGRx,
            size: size(1920, 1080),
            framerate: rate(0, 1),
            max_framerate: rate(45, 1)
        })
    );
}

#[test]
fn a_param_that_is_not_raw_video_is_not_a_fixed_format() {
    for (media_type, subtype) in [
        (MediaType::Audio, MediaSubtype::Raw),
        (MediaType::Video, MediaSubtype::H264),
    ] {
        let bytes = format_param(media_type, subtype, VideoFormat::BGRx, 16, 16);
        assert_eq!(Fixed::parse(Pod::from_bytes(&bytes).unwrap()), None);
    }
}

#[test]
fn a_pod_that_is_not_a_format_object_is_not_a_fixed_format() {
    let bytes = PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Int(3))
        .unwrap()
        .0
        .into_inner();
    assert_eq!(Fixed::parse(Pod::from_bytes(&bytes).unwrap()), None);
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
fn the_enum_format_lists_the_offered_layouts_in_order_at_the_exact_size() {
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
}

#[test]
fn the_enum_format_offers_a_variable_rate_up_to_the_limit() {
    let object = deserialize(&enum_format(&offer()));
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
    assert_eq!(*default, Fraction { num: 30, denom: 1 });
    assert_eq!(*min, Fraction { num: 1, denom: 1 });
    assert_eq!(*max, Fraction { num: 60, denom: 1 });
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
    assert_eq!(DAMAGE_REGIONS, 16);
}
