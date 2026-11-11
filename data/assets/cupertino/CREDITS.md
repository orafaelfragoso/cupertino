# Cupertino original icons

Original first-party artwork created for Cupertino contributors, 2026.
Licensed under GPL-3.0-or-later; see LICENSE.

The SVG files are generated from the parametric constructions in `artwork.py`.
Build the native renderer with `cargo build --locked -p cupertino-icons --example render-artwork`,
then run `python3 -B data/assets/cupertino/generate.py` from the checkout. The PNG renditions
use the shell's image loader; `--renderer PATH` selects a renderer built in another profile.
Apps share an 824-unit continuous-curvature tile on a 1024-unit canvas, frosted glass foregrounds,
soft bevels and directional highlights. Sky-blue folders have bold recessed marks. Documents share
one folded white page with restrained printed content; lettering is drawn as paths. Neutral ribbed bins,
a metal display and free-standing overlay badges use the same lighting. Separate 16- and 32-pixel
optical editions simplify content, strengthen impressions and preserve overlay contrast.
Symbols use a 24-unit grid, rounded terminals and a 1.9-unit regular stroke. Each `.fill` symbol is the filled
edition of its outline for sidebar rows and tiles: the same silhouette as one solid shape, its inner marks cut out.
