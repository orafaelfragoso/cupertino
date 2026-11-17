//! The program cache's names and turns, and the generated programs' tables.

use cupertino_gpu::program_cache::{driver_key, program_key, take_turn, wait_turn, NoTurn};
use cupertino_gpu::shaders::{self, NamedBinding, ShaderSource};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn directory(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("gpu-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_driver_key_is_sixteen_hex_digits_of_its_three_strings() {
    let key = driver_key("Mesa", "llvmpipe", "OpenGL ES 3.2");
    assert_eq!(key.len(), 16);
    assert!(key.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(key, driver_key("Mesa", "llvmpipe", "OpenGL ES 3.2"));
}

#[test]
fn a_driver_update_or_another_card_gives_another_key() {
    let base = driver_key("Mesa", "AMD RADV", "OpenGL ES 3.2 Mesa 25.0");
    assert_ne!(
        base,
        driver_key("Mesa", "AMD RADV", "OpenGL ES 3.2 Mesa 25.1")
    );
    assert_ne!(base, driver_key("Mesa", "Intel", "OpenGL ES 3.2 Mesa 25.0"));
    assert_ne!(
        base,
        driver_key("NVIDIA", "AMD RADV", "OpenGL ES 3.2 Mesa 25.0")
    );
}

#[test]
fn the_parts_of_a_key_cannot_slide_into_each_other() {
    assert_ne!(driver_key("ab", "c", "d"), driver_key("a", "bc", "d"));
    assert_ne!(driver_key("a", "", "b"), driver_key("", "a", "b"));
    assert_ne!(program_key("p", "ab", "c"), program_key("p", "a", "bc"));
}

#[test]
fn a_program_file_is_named_by_its_cleaned_name_and_a_hash_of_its_sources() {
    let key = program_key("ui fill/1", "vertex", "fragment");
    assert!(key.starts_with("ui_fill_1-"), "{key}");
    assert!(key.ends_with(".bin"));
    assert_eq!(key.len(), "ui_fill_1-".len() + 16 + ".bin".len());
    assert_eq!(key, program_key("ui fill/1", "vertex", "fragment"));
}

#[test]
fn a_changed_source_or_name_is_another_file() {
    let base = program_key("blit", "v", "f");
    assert_ne!(base, program_key("blit", "v2", "f"));
    assert_ne!(base, program_key("blit", "v", "f2"));
    // names that clean to the same text still hash apart
    assert_ne!(program_key("a b", "v", "f"), program_key("a_b", "v", "f"));
}

#[test]
fn the_first_process_takes_the_turn_and_another_waits_for_it() {
    let dir = directory("turn");
    let turn = take_turn(&dir, Duration::from_secs(1)).unwrap();
    assert!(matches!(
        take_turn(&dir, Duration::ZERO),
        Err(NoTurn::TimedOut)
    ));
    drop(turn);
    assert!(take_turn(&dir, Duration::ZERO).is_ok());
}

#[test]
fn a_turn_creates_the_directory_it_needs() {
    let dir = directory("turn-new").join("deeper").join("still");
    assert!(take_turn(&dir, Duration::ZERO).is_ok());
    assert!(dir.is_dir());
}

#[test]
fn a_turn_that_cannot_be_held_is_reported_at_once_without_waiting() {
    let dir = directory("turn-unavailable");
    let file = dir.join("not-a-directory");
    std::fs::write(&file, b"").unwrap();
    let started = std::time::Instant::now();
    let error = take_turn(&file.join("driver"), Duration::from_secs(30)).unwrap_err();
    assert!(matches!(error, NoTurn::Unavailable(_)));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(error.to_string().starts_with("the turn cannot be held: "));
    assert_eq!(
        NoTurn::TimedOut.to_string(),
        "another process kept the turn"
    );
}

#[test]
fn waiting_for_a_turn_gives_up_when_asked_to_stop() {
    let dir = directory("turn-stop");
    let _held = take_turn(&dir, Duration::ZERO).unwrap();
    let stop = AtomicBool::new(true);
    assert!(wait_turn(&dir, Duration::from_secs(30), &stop).is_none());
}

#[test]
fn waiting_for_a_turn_gives_up_when_the_patience_runs_out() {
    let dir = directory("turn-patience");
    let _held = take_turn(&dir, Duration::ZERO).unwrap();
    let stop = AtomicBool::new(false);
    let started = std::time::Instant::now();
    assert!(wait_turn(&dir, Duration::from_millis(100), &stop).is_none());
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn waiting_for_a_free_turn_takes_it() {
    let dir = directory("turn-free");
    let stop = AtomicBool::new(false);
    assert!(wait_turn(&dir, Duration::from_secs(1), &stop).is_some());
    assert!(!stop.load(Ordering::Relaxed));
}

#[test]
fn a_waiting_process_gets_the_turn_when_the_holder_lets_go() {
    let dir = directory("turn-handoff");
    let held = take_turn(&dir, Duration::ZERO).unwrap();
    let waiter = {
        let dir = dir.clone();
        std::thread::spawn(move || {
            let stop = AtomicBool::new(false);
            wait_turn(&dir, Duration::from_secs(20), &stop).is_some()
        })
    };
    std::thread::sleep(Duration::from_millis(50));
    drop(held);
    assert!(waiter.join().unwrap());
}

#[test]
fn every_program_has_both_stages_and_spirv() {
    for program in shaders::ALL {
        assert!(
            program.vertex_glsl.starts_with("#version 300 es"),
            "{}",
            program.name
        );
        assert!(
            program.fragment_glsl.starts_with("#version 300 es"),
            "{}",
            program.name
        );
        assert!(program.spirv.len() % 4 == 0, "{}", program.name);
        assert_eq!(
            program.spirv[..4],
            0x0723_0203u32.to_le_bytes(),
            "{}",
            program.name
        );
    }
}

#[test]
fn program_names_are_unique_and_the_toolkits_programs_are_among_all() {
    let names: HashSet<&str> = shaders::ALL.iter().map(|p| p.name).collect();
    assert_eq!(names.len(), shaders::ALL.len());
    for program in shaders::TOOLKIT {
        assert!(names.contains(program.name), "{}", program.name);
    }
    assert!(shaders::TOOLKIT.contains(&shaders::UI_FILL));
    assert!(shaders::TOOLKIT.contains(&shaders::UI_TEXTURE));
}

#[test]
fn a_program_lists_its_uniform_blocks_and_samplers_by_name() {
    let bindings = shaders::UI_FILL.bindings();
    assert!(bindings.contains(&NamedBinding::Uniform {
        group: 0,
        binding: 0,
        block: "Draw_block_0Vertex".to_owned()
    }));
    assert!(bindings.contains(&NamedBinding::Uniform {
        group: 0,
        binding: 0,
        block: "Draw_block_0Fragment".to_owned()
    }));
    assert!(bindings.contains(&NamedBinding::Texture {
        group: 0,
        binding: 1,
        sampler: None,
        name: "_group_0_binding_1_fs".to_owned()
    }));
    assert_eq!(bindings.len(), 4);
}

fn source(bindings: &'static str) -> ShaderSource {
    ShaderSource {
        name: "test",
        vertex_glsl: "",
        fragment_glsl: "",
        spirv: &[],
        bindings_text: bindings,
    }
}

#[test]
fn binding_lines_with_a_sampler_name_it_and_comments_and_blanks_are_skipped() {
    let bindings =
        source("# a comment\n\nuniform 1 2 vertex Block\n  \ntexture 0 3 0 4 fragment image\n")
            .bindings();
    assert_eq!(
        bindings,
        [
            NamedBinding::Uniform {
                group: 1,
                binding: 2,
                block: "Block".to_owned()
            },
            NamedBinding::Texture {
                group: 0,
                binding: 3,
                sampler: Some((0, 4)),
                name: "image".to_owned()
            }
        ]
    );
}

#[test]
fn binding_lines_that_do_not_fit_a_layout_are_skipped() {
    let bindings = source(
        "uniform x 2 vertex Block\nuniform 1 2 vertex\nsampler 0 1 fragment s\ntexture 0 1 fragment s\n",
    )
    .bindings();
    assert!(bindings.is_empty());
}

#[test]
fn a_texture_with_half_a_sampler_has_none() {
    let bindings = source("texture 0 3 0 - fragment image\n").bindings();
    assert_eq!(
        bindings,
        [NamedBinding::Texture {
            group: 0,
            binding: 3,
            sampler: None,
            name: "image".to_owned()
        }]
    );
}
