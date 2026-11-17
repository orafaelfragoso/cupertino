//! The uniform blocks as the floats and bytes the programs read.

use cupertino_gpu::{ClipBlock, ClipsBlock, FillBlock, TextureBlock};

#[test]
fn the_blocks_have_the_sizes_of_their_std140_layouts() {
    assert_eq!(FillBlock::FLOATS, 144);
    assert_eq!(TextureBlock::FLOATS, 96);
    assert_eq!(ClipsBlock::FLOATS, 60);
    assert_eq!(std::mem::size_of::<FillBlock>(), 144 * 4);
    assert_eq!(std::mem::size_of::<TextureBlock>(), 96 * 4);
    assert_eq!(std::mem::size_of::<ClipsBlock>(), 60 * 4);
    assert_eq!(std::mem::size_of::<ClipBlock>(), 12 * 4);
}

#[test]
fn a_new_block_is_all_zero() {
    assert!(FillBlock::default().floats().iter().all(|f| *f == 0.0));
    assert!(TextureBlock::default().floats().iter().all(|f| *f == 0.0));
    assert!(FillBlock::default().bytes().iter().all(|b| *b == 0));
}

#[test]
fn the_floats_follow_the_members_in_order() {
    let mut block = FillBlock {
        frame: [1.0, 2.0, 3.0, 4.0],
        shape_linear: [5.0, 6.0, 7.0, 8.0],
        outline: [13.0, 14.0, 15.0, 16.0],
        ..FillBlock::default()
    };
    block.colors[7] = [9.0, 10.0, 11.0, 12.0];
    let floats = block.floats();
    assert_eq!(floats[..8], [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
    assert_eq!(floats[44 + 4 * 7..44 + 4 * 8], [9.0, 10.0, 11.0, 12.0]);
    assert_eq!(floats[140..144], [13.0, 14.0, 15.0, 16.0]);
}

#[test]
fn the_clips_start_where_the_block_says() {
    let mut fill = FillBlock::default();
    fill.clips.info = [7.0, 0.0, 0.0, 0.0];
    assert_eq!(fill.floats()[FillBlock::CLIPS], 7.0);
    // the clips fill the floats from there up to the corner radii
    assert_eq!(FillBlock::CLIPS + ClipsBlock::FLOATS, 136);
    fill.corners = [1.0, 2.0, 3.0, 4.0];
    assert_eq!(
        fill.floats()[FillBlock::CLIPS + ClipsBlock::FLOATS..][..4],
        [1.0, 2.0, 3.0, 4.0]
    );

    let mut texture = TextureBlock::default();
    texture.clips.info = [3.0, 0.0, 0.0, 0.0];
    assert_eq!(texture.floats()[TextureBlock::CLIPS], 3.0);
    assert_eq!(
        TextureBlock::CLIPS + ClipsBlock::FLOATS,
        TextureBlock::FLOATS
    );
}

#[test]
fn the_analytic_clips_come_after_the_three_leading_vectors() {
    let mut clips = ClipsBlock::default();
    clips.clips[0].linear = [1.0, 2.0, 3.0, 4.0];
    clips.clips[3].corner = [5.0, 6.0, 7.0, 8.0];
    let fill = FillBlock {
        clips,
        ..FillBlock::default()
    };
    let floats = &fill.floats()[FillBlock::CLIPS..];
    assert_eq!(floats[12..16], [1.0, 2.0, 3.0, 4.0]);
    assert_eq!(
        floats[12 + 3 * 12 + 8..12 + 3 * 12 + 12],
        [5.0, 6.0, 7.0, 8.0]
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
    assert_eq!(bytes.len(), 144 * 4);
    assert_eq!(bytes[..4], 1.5f32.to_ne_bytes());
    assert_eq!(bytes[143 * 4..], (-2.0f32).to_ne_bytes());
}
