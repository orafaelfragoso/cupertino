use crate::{
    Arg, Args, Command, Context, Failure, FailureKind, Group, Reply, Run, Stability, ValueHint,
};
use cupertino_assets::{AssetError, Assets, PackReport};
use std::path::Path;

pub(crate) const GROUP: Group = Group {
    name: "appearance",
    summary: "The look of the desktop: its asset packs",
    description: "Commands that read what the desktop looks like and what it draws with. An asset pack is a directory \
                  with a manifest.json that provides fonts, a cursor theme, wallpapers and symbols by logical name; \
                  whatever a pack does not provide comes from the built-in fallback.",
    commands: &[COMMANDS],
};

const COMMANDS: &[Command] = &[Command {
    path: &["appearance", "assets", "check"],
    summary: "Reads the asset pack in <dir> as the desktop processes do and reports its problems and what provides \
              each logical name, the pack or the built-in fallback",
    description: "Reads the pack in <dir> the way the desktop processes read it at start: the manifest, then every \
                  file it names. It lists the problems found inside the pack (unknown manifest keys, missing or \
                  unreadable files), which are reported but do not fail the check, and for each logical name whether \
                  the pack or the built-in fallback provides it.\n\n\
                  A pack that cannot be read at all fails with exit code 5: a missing directory or manifest \
                  (pack_unreadable), a manifest that is not valid (manifest_invalid) or a font that cannot be loaded \
                  (font_invalid). It replaces `cupertino-assets check <dir>`.",
    examples: &[
        (
            "cupertino appearance assets check ~/.local/share/cupertino/assets",
            "Check the pack installed for the current user.",
        ),
        (
            "cupertino --format json appearance assets check ./my-pack",
            "Report the pack in ./my-pack as one JSON envelope.",
        ),
    ],
    stability: Stability::Stable,
    args: &[Arg::positional("dir", "the pack directory, with its manifest.json").hint(ValueHint::Dir)],
    run: Run::Once(assets_check),
}];

/// Problems inside a readable pack are part of the report; only a pack that cannot be read at all fails.
/// The directory is read as given; the report names it the same way.
fn assets_check(args: &Args, _: &Context) -> Result<Reply, Failure> {
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
