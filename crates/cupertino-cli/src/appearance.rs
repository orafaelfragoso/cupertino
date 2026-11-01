use crate::{Arg, ArgKind, Args, Command, Failure, FailureKind, Reply};
use cupertino_assets::{AssetError, Assets, PackReport};
use std::path::Path;

pub(crate) const COMMANDS: &[Command] = &[Command {
    path: &["appearance", "assets", "check"],
    summary: "Reads the asset pack in <dir> as the desktop processes do and reports its problems and what provides \
              each logical name, the pack or the built-in fallback",
    args: &[Arg {
        name: "dir",
        help: "the pack directory, with its manifest.json",
        kind: ArgKind::Positional,
    }],
    run: assets_check,
}];

/// Problems inside a readable pack are part of the report; only a pack that cannot be read at all fails.
fn assets_check(args: &Args) -> Result<Reply, Failure> {
    let dir = args.value("dir").expect("<dir> is a positional argument");
    let (assets, notes) = Assets::read_pack(Path::new(dir)).map_err(|e| {
        let code = match e {
            AssetError::Read { .. } => "pack_unreadable",
            AssetError::Manifest { .. } => "manifest_invalid",
            AssetError::Font { .. } => "font_invalid",
        };
        Failure::new(FailureKind::Validation, code, e.to_string())
    })?;
    let report = PackReport::new(&assets, &notes);
    Ok(Reply::new(&report, report.to_string()))
}
