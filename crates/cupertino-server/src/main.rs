use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("info,smithay=warn,usvg=error,resvg=error,smithay::backend::egl::ffi=off")
    });
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    cupertino_server::cli::run(cupertino_server::cli::Cli::parse())
}
