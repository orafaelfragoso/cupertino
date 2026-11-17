//! The uniform blocks as the floats and bytes the programs read.

use cupertino_gpu::{ClipBlock, ClipsBlock, FillBlock, TextureBlock};

#[test]
fn a_block_is_its_floats_and_nothing_else_and_starts_all_zero() {
    for (name, size, floats) in [
        ("fill", std::mem::size_of::<FillBlock>(), FillBlock::FLOATS),
        (
            "texture",
            std::mem::size_of::<TextureBlock>(),
            TextureBlock::FLOATS,
        ),
        ("clips", std::mem::size_of::<ClipsBlock>(), ClipsBlock::FLOATS),
        ("clip", std::mem::size_of::<ClipBlock>(), 12),
    ] {
        assert_eq!(size, floats * 4, "{name}");
    }
    assert!(FillBlock::default().floats().iter().all(|f| *f == 0.0));
    assert!(TextureBlock::default().floats().iter().all(|f| *f == 0.0));
    assert!(FillBlock::default().bytes().iter().all(|b| *b == 0));
}

#[test]
fn the_floats_follow_the_std140_layout_the_programs_read() {
    let mut fill = FillBlock {
        frame: [1.0, 2.0, 3.0, 4.0],
        shape_linear: [5.0, 6.0, 7.0, 8.0],
        outline: [13.0, 14.0, 15.0, 16.0],
        corners: [17.0, 18.0, 19.0, 20.0],
        ..FillBlock::default()
    };
    fill.colors[7] = [9.0, 10.0, 11.0, 12.0];
    fill.clips.info = [7.0, 0.0, 0.0, 0.0];
    // the analytic clips come after the clips' three leading vectors
    fill.clips.clips[0].linear = [21.0, 22.0, 23.0, 24.0];
    fill.clips.clips[3].corner = [25.0, 26.0, 27.0, 28.0];
    let floats = fill.floats();
    assert_eq!(floats.len(), 144);
    let clips = FillBlock::CLIPS;
    for (name, start, expected) in [
        ("frame and shape", 0, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0][..]),
        ("the eighth color", 44 + 4 * 7, &[9.0, 10.0, 11.0, 12.0]),
        ("the clips' info", clips, &[7.0]),
        ("the first clip", clips + 12, &[21.0, 22.0, 23.0, 24.0]),
        (
            "the fourth clip's corner",
            clips + 12 + 3 * 12 + 8,
            &[25.0, 26.0, 27.0, 28.0],
        ),
        (
            "the corners right after the clips",
            136,
            &[17.0, 18.0, 19.0, 20.0],
        ),
        ("the outline last", 140, &[13.0, 14.0, 15.0, 16.0]),
    ] {
        assert_eq!(&floats[start..start + expected.len()], expected, "{name}");
    }
    assert_eq!(FillBlock::CLIPS + ClipsBlock::FLOATS, 136);

    let mut texture = TextureBlock::default();
    texture.clips.info = [3.0, 0.0, 0.0, 0.0];
    assert_eq!(texture.floats()[TextureBlock::CLIPS], 3.0);
    assert_eq!(
        TextureBlock::CLIPS + ClipsBlock::FLOATS,
        TextureBlock::FLOATS,
        "the clips end the texture block"
    );
}

#[test]
fn writing_floats_in_place_changes_the_members() {
    let mut block = TextureBlock::default();
    block.floats_mut()[4] = 0.5;
    block.floats_mut()[TextureBlock::CLIPS + 12] = 9.0;
    assert_eq!(block.mode, [0.5, 0.0, 0.0, 0.0]);
    assert_eq!(block.clips.clips[0].linear[0], 9.0);
}

#[test]
fn the_bytes_are_the_floats_in_native_order() {
    let mut block = FillBlock::default();
    block.frame[0] = 1.5;
    block.outline[3] = -2.0;
    let bytes = block.bytes();
    assert_eq!(bytes.len(), FillBlock::FLOATS * 4);
    assert_eq!(bytes[..4], 1.5f32.to_ne_bytes());
    assert_eq!(bytes[143 * 4..], (-2.0f32).to_ne_bytes());
}
