//! The device-independent values of the API, and the synchronisation helpers that need no GPU.

use cupertino_gpu::syncobj::{eventfd, wait_readable, SyncNode};
use cupertino_gpu::{
    Dmabuf, DmabufPlane, Loss, TextureFormat, VertexFormat, FOURCC_ARGB8888, MODIFIER_LINEAR,
};
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::time::{Duration, Instant};

#[test]
fn a_texture_pixel_and_a_vertex_attribute_take_the_bytes_of_their_channels() {
    for (format, expected) in [
        (TextureFormat::R8, 1),
        (TextureFormat::Rgba8, 4),
        (TextureFormat::Rgba16Float, 8),
        (TextureFormat::Rgba32Float, 16),
    ] {
        assert_eq!(format.bytes_per_pixel(), expected, "{format:?}");
    }
    for (format, expected) in [
        (VertexFormat::Float32x2, 8),
        (VertexFormat::Float32x4, 16),
        (VertexFormat::Unorm8x4, 4),
    ] {
        assert_eq!(format.size(), expected, "{format:?}");
    }
}

#[test]
fn only_a_reset_counts_toward_falling_back_to_the_cpu() {
    for (loss, expected) in [
        (Loss::Reset { guilty: None }, true),
        (Loss::Reset { guilty: Some(true) }, true),
        (Loss::Purge, false),
    ] {
        assert_eq!(loss.counts_as_failure(), expected, "{loss:?}");
    }
}

#[test]
fn a_cloned_dmabuf_has_its_own_descriptors_of_the_same_files_for_every_plane() {
    let (reader, writer) = std::io::pipe().unwrap();
    let (_, second) = std::io::pipe().unwrap();
    let buffer = Dmabuf {
        size: (64, 32),
        fourcc: FOURCC_ARGB8888,
        modifier: MODIFIER_LINEAR,
        planes: [OwnedFd::from(writer), OwnedFd::from(second)]
            .into_iter()
            .enumerate()
            .map(|(i, fd)| DmabufPlane {
                fd,
                offset: 16 * i as u32,
                stride: 256,
            })
            .collect(),
    };
    let clone = buffer.try_clone().unwrap();
    assert_eq!(
        (clone.size, clone.fourcc, clone.modifier),
        ((64, 32), FOURCC_ARGB8888, MODIFIER_LINEAR)
    );
    assert_eq!(
        clone
            .planes
            .iter()
            .map(|p| (p.offset, p.stride))
            .collect::<Vec<_>>(),
        [(0, 256), (16, 256)]
    );
    for (copy, original) in clone.planes.iter().zip(&buffer.planes) {
        assert_ne!(copy.fd.as_raw_fd(), original.fd.as_raw_fd());
    }

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
fn an_eventfd_is_readable_only_once_signalled_and_never_blocks_a_read() {
    let fd = eventfd().unwrap();
    let mut file = std::fs::File::from(fd.try_clone().unwrap());
    let mut bytes = [0u8; 8];
    assert_eq!(
        file.read(&mut bytes).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(!wait_readable(fd.as_fd(), 0));
    let started = Instant::now();
    assert!(!wait_readable(fd.as_fd(), 50));
    assert!(started.elapsed() >= Duration::from_millis(40));

    file.write_all(&1u64.to_ne_bytes()).unwrap();
    assert!(wait_readable(fd.as_fd(), 0));
    assert!(wait_readable(fd.as_fd(), -1));
}

#[test]
fn a_sync_node_that_is_not_there_cannot_be_opened() {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-such-render-node");
    assert!(SyncNode::open(&path).is_err());
}
