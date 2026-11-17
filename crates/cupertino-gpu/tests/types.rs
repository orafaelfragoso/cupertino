//! The device-independent values and errors of the API, and the synchronisation helpers that need no GPU.

use cupertino_gpu::syncobj::{eventfd, wait_readable, SyncNode};
use cupertino_gpu::{
    Dmabuf, DmabufPlane, Error, Loss, Region, TextureFormat, VertexFormat, FOURCC_ABGR8888,
    FOURCC_ARGB8888, MODIFIER_INVALID, MODIFIER_LINEAR,
};
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};

#[test]
fn a_texture_pixel_takes_the_bytes_of_its_channels() {
    assert_eq!(TextureFormat::R8.bytes_per_pixel(), 1);
    assert_eq!(TextureFormat::Rgba8.bytes_per_pixel(), 4);
    assert_eq!(TextureFormat::Rgba16Float.bytes_per_pixel(), 8);
    assert_eq!(TextureFormat::Rgba32Float.bytes_per_pixel(), 16);
}

#[test]
fn a_vertex_attribute_takes_the_bytes_of_its_format() {
    assert_eq!(VertexFormat::Float32x2.size(), 8);
    assert_eq!(VertexFormat::Float32x4.size(), 16);
    assert_eq!(VertexFormat::Unorm8x4.size(), 4);
}

#[test]
fn a_region_keeps_its_corner_and_size() {
    let region = Region::new(1, 2, 3, 4);
    assert_eq!(
        (region.x, region.y, region.width, region.height),
        (1, 2, 3, 4)
    );
    assert_eq!(Region::default(), Region::new(0, 0, 0, 0));
}

#[test]
fn only_a_reset_counts_toward_falling_back_to_the_cpu() {
    assert!(Loss::Reset { guilty: None }.counts_as_failure());
    assert!(Loss::Reset { guilty: Some(true) }.counts_as_failure());
    assert!(!Loss::Purge.counts_as_failure());
}

#[test]
fn errors_name_their_kind_and_their_reason() {
    assert_eq!(
        Error::Unavailable("no EGL".to_owned()).to_string(),
        "unavailable: no EGL"
    );
    assert_eq!(
        Error::Unsupported("fences".to_owned()).to_string(),
        "unsupported: fences"
    );
    assert_eq!(
        Error::Invalid("size".to_owned()).to_string(),
        "invalid: size"
    );
    assert_eq!(
        Error::Driver("out of memory".to_owned()).to_string(),
        "driver: out of memory"
    );
    assert_eq!(
        Error::OverBudget {
            what: "atlas",
            need: 10,
            cap: 20
        }
        .to_string(),
        "atlas: 10 bytes more than the cap of 20 allows"
    );
    assert!(Error::Lost(Loss::Purge)
        .to_string()
        .starts_with("device lost: "));
}

#[test]
fn the_fourcc_codes_spell_their_formats_in_memory_order() {
    assert_eq!(FOURCC_ARGB8888.to_le_bytes(), *b"AR24");
    assert_eq!(FOURCC_ABGR8888.to_le_bytes(), *b"AB24");
    assert_eq!(MODIFIER_LINEAR, 0);
    assert_eq!(MODIFIER_INVALID, 0x00ff_ffff_ffff_ffff);
}

fn dmabuf_over(fds: Vec<OwnedFd>) -> Dmabuf {
    Dmabuf {
        size: (64, 32),
        fourcc: FOURCC_ARGB8888,
        modifier: MODIFIER_LINEAR,
        planes: fds
            .into_iter()
            .enumerate()
            .map(|(i, fd)| DmabufPlane {
                fd,
                offset: 16 * i as u32,
                stride: 256,
            })
            .collect(),
    }
}

#[test]
fn a_cloned_dmabuf_has_its_own_descriptors_of_the_same_files() {
    let (reader, writer) = std::io::pipe().unwrap();
    let buffer = dmabuf_over(vec![OwnedFd::from(writer)]);
    let clone = buffer.try_clone().unwrap();
    assert_eq!(clone.size, (64, 32));
    assert_eq!(clone.fourcc, FOURCC_ARGB8888);
    assert_eq!(clone.modifier, MODIFIER_LINEAR);
    assert_eq!(clone.planes.len(), 1);
    assert_eq!((clone.planes[0].offset, clone.planes[0].stride), (0, 256));
    assert_ne!(
        std::os::fd::AsRawFd::as_raw_fd(&clone.planes[0].fd),
        std::os::fd::AsRawFd::as_raw_fd(&buffer.planes[0].fd)
    );

    // the original can go and the copy still reaches the pipe
    drop(buffer);
    let mut writer = std::fs::File::from(clone.planes[0].fd.try_clone().unwrap());
    writer.write_all(b"frame").unwrap();
    drop(writer);
    drop(clone);
    let mut text = String::new();
    { reader }.read_to_string(&mut text).unwrap();
    assert_eq!(text, "frame");
}

#[test]
fn every_plane_of_a_dmabuf_is_cloned_with_its_offset() {
    let (_, first) = std::io::pipe().unwrap();
    let (_, second) = std::io::pipe().unwrap();
    let buffer = dmabuf_over(vec![OwnedFd::from(first), OwnedFd::from(second)]);
    let clone = buffer.try_clone().unwrap();
    assert_eq!(
        clone.planes.iter().map(|p| p.offset).collect::<Vec<_>>(),
        [0, 16]
    );
}

#[test]
fn an_eventfd_is_not_readable_until_it_is_signalled() {
    let fd = eventfd().unwrap();
    assert!(!wait_readable(fd.as_fd(), 0));
    let mut file = std::fs::File::from(fd.try_clone().unwrap());
    file.write_all(&1u64.to_ne_bytes()).unwrap();
    assert!(wait_readable(fd.as_fd(), 0));
    assert!(wait_readable(fd.as_fd(), -1));
}

#[test]
fn an_eventfd_does_not_block_a_read_that_has_nothing_to_read() {
    let fd = eventfd().unwrap();
    let mut file = std::fs::File::from(fd);
    let mut bytes = [0u8; 8];
    let error = file.read(&mut bytes).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
}

#[test]
fn waiting_on_a_quiet_descriptor_times_out() {
    let fd = eventfd().unwrap();
    let started = std::time::Instant::now();
    assert!(!wait_readable(fd.as_fd(), 50));
    assert!(started.elapsed() >= std::time::Duration::from_millis(40));
}

#[test]
fn a_sync_node_that_is_not_there_cannot_be_opened() {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-such-render-node");
    assert!(SyncNode::open(&path).is_err());
}
