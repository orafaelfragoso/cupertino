//! The program cache's names and turns, and the generated programs' tables.

use cupertino_gpu::program_cache::{driver_key, program_key, take_turn, wait_turn, NoTurn};
use cupertino_gpu::shaders::{self, NamedBinding, ShaderSource};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

fn directory(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("gpu-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_key_is_a_stable_hash_of_its_parts_and_a_program_file_carries_its_cleaned_name() {
    let key = driver_key("Mesa", "llvmpipe", "OpenGL ES 3.2");
    assert_eq!(key.len(), 16);
    assert!(key.bytes().all(|b| b.is_ascii_hexdigit()), "{key}");
    assert_eq!(key, driver_key("Mesa", "llvmpipe", "OpenGL ES 3.2"));

    let file = program_key("ui fill/1", "vertex", "fragment");
    assert!(file.starts_with("ui_fill_1-") && file.ends_with(".bin"), "{file}");
    assert_eq!(file.len(), "ui_fill_1-".len() + 16 + ".bin".len());
    assert_eq!(file, program_key("ui fill/1", "vertex", "fragment"));
}

#[test]
fn a_change_to_any_part_gives_another_key_and_parts_cannot_slide_into_each_other() {
    let driver = driver_key("Mesa", "AMD RADV", "OpenGL ES 3.2 Mesa 25.0");
    let program = program_key("blit", "v", "f");
    for (name, a, b) in [
        (
            "a driver update",
            driver.clone(),
            driver_key("Mesa", "AMD RADV", "OpenGL ES 3.2 Mesa 25.1"),
        ),
        (
            "another card",
            driver.clone(),
            driver_key("Mesa", "Intel", "OpenGL ES 3.2 Mesa 25.0"),
        ),
        (
            "another vendor",
            driver,
            driver_key("NVIDIA", "AMD RADV", "OpenGL ES 3.2 Mesa 25.0"),
        ),
        (
            "driver parts sliding",
            driver_key("ab", "c", "d"),
            driver_key("a", "bc", "d"),
        ),
        (
            "an empty driver part",
            driver_key("a", "", "b"),
            driver_key("", "a", "b"),
        ),
        ("a vertex change", program.clone(), program_key("blit", "v2", "f")),
        ("a fragment change", program, program_key("blit", "v", "f2")),
        (
            "program parts sliding",
            program_key("p", "ab", "c"),
            program_key("p", "a", "bc"),
        ),
        (
            "names that clean alike",
            program_key("a b", "v", "f"),
            program_key("a_b", "v", "f"),
        ),
    ] {
        assert_ne!(a, b, "{name}");
    }
}

#[test]
fn one_process_holds_the_turn_in_a_directory_it_creates_and_an_unusable_place_fails_at_once() {
    let dir = directory("turn");
    let turn = take_turn(&dir, Duration::from_secs(1)).unwrap();
    assert!(matches!(
        take_turn(&dir, Duration::ZERO),
        Err(NoTurn::TimedOut)
    ));
    drop(turn);
    assert!(take_turn(&dir, Duration::ZERO).is_ok());

    let deeper = directory("turn-new").join("deeper").join("still");
    assert!(take_turn(&deeper, Duration::ZERO).is_ok());
    assert!(deeper.is_dir());

    let file = directory("turn-unavailable").join("not-a-directory");
    std::fs::write(&file, b"").unwrap();
    let started = Instant::now();
    let error = take_turn(&file.join("driver"), Duration::from_secs(30)).unwrap_err();
    assert!(matches!(error, NoTurn::Unavailable(_)), "{error:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn waiting_for_a_turn_takes_it_when_free_or_let_go_and_gives_up_on_stop_or_patience() {
    for (name, held, patience, stop, let_go, expected) in [
        ("free", false, Duration::from_secs(1), false, false, true),
        ("let go", true, Duration::from_secs(20), false, true, true),
        ("stopped", true, Duration::from_secs(30), true, false, false),
        (
            "out of patience",
            true,
            Duration::from_millis(100),
            false,
            false,
            false,
        ),
    ] {
        let dir = directory(&format!("turn-wait-{}", name.replace(' ', "-")));
        let holder = held.then(|| take_turn(&dir, Duration::ZERO).unwrap());
        let started = Instant::now();
        let waiter = {
            let dir = dir.clone();
            std::thread::spawn(move || {
                wait_turn(&dir, patience, &AtomicBool::new(stop)).is_some()
            })
        };
        if let_go {
            std::thread::sleep(Duration::from_millis(50));
            drop(holder);
            assert_eq!(waiter.join().unwrap(), expected, "{name}");
        } else {
            assert_eq!(waiter.join().unwrap(), expected, "{name}");
            drop(holder);
        }
        assert!(started.elapsed() < Duration::from_secs(10), "{name}");
    }
}

#[test]
fn every_generated_program_has_both_stages_and_spirv_under_a_unique_name() {
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
    for expected in [
        NamedBinding::Uniform {
            group: 0,
            binding: 0,
            block: "Draw_block_0Vertex".to_owned(),
        },
        NamedBinding::Uniform {
            group: 0,
            binding: 0,
            block: "Draw_block_0Fragment".to_owned(),
        },
        NamedBinding::Texture {
            group: 0,
            binding: 1,
            sampler: None,
            name: "_group_0_binding_1_fs".to_owned(),
        },
    ] {
        assert!(bindings.contains(&expected), "{expected:?}");
    }
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
fn binding_lines_are_read_and_lines_that_fit_no_layout_skipped() {
    let image = |sampler| NamedBinding::Texture {
        group: 0,
        binding: 3,
        sampler,
        name: "image".to_owned(),
    };
    for (text, expected) in [
        (
            "# a comment\n\nuniform 1 2 vertex Block\n  \ntexture 0 3 0 4 fragment image\n",
            vec![
                NamedBinding::Uniform {
                    group: 1,
                    binding: 2,
                    block: "Block".to_owned(),
                },
                image(Some((0, 4))),
            ],
        ),
        ("texture 0 3 0 - fragment image\n", vec![image(None)]),
        (
            "uniform x 2 vertex Block\nuniform 1 2 vertex\nsampler 0 1 fragment s\ntexture 0 1 fragment s\n",
            vec![],
        ),
    ] {
        assert_eq!(source(text).bindings(), expected, "{text:?}");
    }
}
