//! The `cupertino` command; `cupertino help` lists its subcommands.

fn main() -> std::process::ExitCode {
    cupertino_cli::main_with(cupertino_cli::REGISTRY)
}
