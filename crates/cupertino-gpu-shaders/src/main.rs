//! `cupertino-gpu-shaders [--check]`: generate the GLSL ES 3.00, SPIR-V and binding names of every program of every
//! set ([`cupertino_gpu_shaders::sets`]: `crates/cupertino-gpu/shaders/` and `crates/cupertino-glass/shaders/`) and
//! write them into the set's `generated/` directory, or, with `--check`, report the files that differ from a fresh
//! generation and fail. Development only; never packaged.

use std::process::ExitCode;

fn main() -> ExitCode {
    let check = match std::env::args().nth(1).as_deref() {
        None => false,
        Some("--check") => true,
        Some(_) => {
            eprintln!("usage: cupertino-gpu-shaders [--check]");
            return ExitCode::FAILURE;
        }
    };
    let mut failed = false;
    for set in cupertino_gpu_shaders::sets() {
        let fresh = match cupertino_gpu_shaders::generate_set(&set) {
            Ok(files) => files,
            Err(e) => {
                eprintln!("cupertino-gpu-shaders: {e}");
                return ExitCode::FAILURE;
            }
        };
        if check {
            let problems = cupertino_gpu_shaders::differences(&fresh, &set.generated);
            for p in &problems {
                eprintln!("cupertino-gpu-shaders: {}: {p}", set.generated.display());
            }
            failed |= !problems.is_empty();
            continue;
        }
        match cupertino_gpu_shaders::write(&fresh, &set.generated) {
            Ok(()) => println!("{} files in {}", fresh.len(), set.generated.display()),
            Err(e) => {
                eprintln!("cupertino-gpu-shaders: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
