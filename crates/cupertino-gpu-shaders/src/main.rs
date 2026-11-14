//! `cupertino-gpu-shaders [--check]`: generate the GLSL ES 3.00, SPIR-V and binding names of every program in
//! `crates/cupertino-gpu/shaders/` and write them into its `generated/` directory, or, with `--check`, report the
//! files that differ from a fresh generation and fail. Development only; never packaged.

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
    let fresh = match cupertino_gpu_shaders::generate_all(&cupertino_gpu_shaders::source_dir()) {
        Ok(files) => files,
        Err(e) => {
            eprintln!("cupertino-gpu-shaders: {e}");
            return ExitCode::FAILURE;
        }
    };
    let generated = cupertino_gpu_shaders::generated_dir();
    if check {
        let problems = cupertino_gpu_shaders::differences(&fresh, &generated);
        for p in &problems {
            eprintln!("cupertino-gpu-shaders: {p}");
        }
        return if problems.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    match cupertino_gpu_shaders::write(&fresh, &generated) {
        Ok(()) => {
            println!("{} files in {}", fresh.len(), generated.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("cupertino-gpu-shaders: {e}");
            ExitCode::FAILURE
        }
    }
}
