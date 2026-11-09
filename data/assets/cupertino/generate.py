#!/usr/bin/env python3
"""Rebuild original SVG artwork and PNG renditions with the native icon loader."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent
# Symbols share a 24-unit canvas, 1.9-unit strokes and rounded terminals.
def path(d):
    return f'<path d="{d}"/>'
def rect(x, y, w, h, r=2):
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}"/>'
def circle(x, y, r):
    return f'<circle cx="{x}" cy="{y}" r="{r}"/>'

S = {}
S['folder'] = path('M3 8V6a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v10H3Z')
S['house'] = path('m3 11 9-8 9 8M5 10v11h5v-7h4v7h5V10')
S['document'] = path('M6 3h8l4 4v14H6ZM14 3v5h4M9 12h6m-6 4h6')
S['download'] = path('M12 3v12m-5-5 5 5 5-5M4 16v5h16v-5')
S['computer'] = rect(3,4,18,13)+path('M12 17v4m-5 0h10')
S['desktop'] = S['computer']+path('M6 8h5v5H6Z')
S['screen-mirroring'] = rect(2,3,15,12)+rect(8,9,14,12)
S['recents'] = circle(12,12,9)+path('M12 6v6l4 3')
S['search'] = circle(10.5,10.5,6.5)+path('m16 16 5 5')
S['magnifyingglass'] = S['search']
S['trash'] = path('M4 6h16M9 6V3h6v3M6 6l1 15h10l1-15M10 10v7m4-7v7')
S['share'] = path('M12 15V3m-4 4 4-4 4 4M7 10H4v11h16V10h-3')
S['tag'] = path('M3 4h8l10 10-7 7L3 10Z')+circle(7,8,1)
S['photo'] = rect(3,4,18,16)+circle(8,9,1.5)+path('m4 18 6-6 4 4 3-3 4 5')
S['gallery'] = S['photo']
S['film'] = rect(3,3,18,18)+path('M7 3v18M17 3v18M3 8h4m-4 8h4M17 8h4m-4 8h4')
S['music'] = path('M10 17V6l10-3v11M10 9l10-3')+circle(7,18,3)+circle(17,15,3)
S['grid'] = ''.join(rect(x,y,6,6,1) for x in (4,14) for y in (4,14))
S['applications'] = S['grid']
S['list'] = path('M9 5h12M9 12h12M9 19h12')+''.join(circle(4,y,.7) for y in (5,12,19))
S['columns'] = rect(3,4,18,16)+path('M9 4v16m6-16v16')
S['sidebar'] = rect(3,4,18,16)+path('M9 4v16M5.5 8h1m-1 4h1')
S['stage'] = rect(8,4,13,16)+rect(2,6,3,4,1)+rect(2,14,3,4,1)
for n,d in {'left':'m15 5-7 7 7 7','right':'m9 5 7 7-7 7','up':'m5 15 7-7 7 7','down':'m5 9 7 7 7-7','up.down':'m8 8 4-4 4 4m-8 8 4 4 4-4'}.items():
    S['chevron.'+n] = path(d)
S['checkmark'] = path('m4 12 5 5L20 6')
S['xmark.circle'] = circle(12,12,9)+path('m8 8 8 8m0-8-8 8')
S['ellipsis.circle'] = circle(12,12,9)+''.join(circle(x,12,.6) for x in (7,12,17))
S['eject'] = path('m5 14 7-10 7 10ZM5 19h14')
S['wifi'] = path('M2 8a16 16 0 0 1 20 0M5 12a11 11 0 0 1 14 0m-11 4a6 6 0 0 1 8 0')+circle(12,20,.8)
S['bluetooth'] = path('M12 2v20l7-6L6 6m6-4 7 6L6 18')
S['battery'] = rect(2,7,18,10)+path('M22 10v4M5 10v4m3-4v4m3-4v4m3-4v4')
S['sun'] = circle(12,12,4)+path('M12 2v2m0 16v2M2 12h2m16 0h2M5 5l1.5 1.5m11 11L19 19M5 19l1.5-1.5m11-11L19 5')
S['moon'] = path('M18 16A9 9 0 0 1 8 3a9 9 0 1 0 13 13Z')
S['moon.fill'] = '<g fill="currentColor">'+S['moon']+'</g>'
S['appearance'] = circle(12,12,9)+path('M12 3v18M12 3a9 9 0 0 1 0 18Z')
S['cloud'] = path('M7 19a5 5 0 1 1 0-10 6 6 0 0 1 12 1 4.5 4.5 0 0 1 0 9Z')
S['network'] = rect(8,3,8,5,1)+path('M12 8v5M4 17v-4h16v4')+rect(1,17,6,4,1)+rect(9,17,6,4,1)+rect(17,17,6,4,1)
S['lock'] = rect(5,10,14,11)+path('M8 10V7a4 4 0 0 1 8 0v3M12 14v3')
S['lock-screen'] = S['lock']
S['privacy'] = path('m12 2 8 3v6q0 7-8 11-8-4-8-11V5Z')+path('m8 11 3 3 5-6')
S['bell'] = path('M5 17h14l-2-4V9a5 5 0 0 0-10 0v4ZM10 21h4M12 2v2')
S['mic'] = rect(9,2,6,13,3)+path('M5 11v1a7 7 0 0 0 14 0v-1M12 19v3m-4 0h8')
S['mic.slash'] = S['mic']+path('M3 3l18 18')
S['speaker'] = path('M3 9h4l6-5v16l-6-5H3Z')
S['speaker.low'] = S['speaker']+path('M16 8a6 6 0 0 1 0 8')
S['speaker.high'] = S['speaker.low']+path('M19 4a11 11 0 0 1 0 16')
S['speaker.slash'] = S['speaker']+path('m17 9 5 6m0-6-5 6')
S['keyboard'] = rect(2,6,20,14)+path('M6 10h1m3 0h1m3 0h1m3 0h1M6 14h1m3 0h1m3 0h1m3 0h1M7 17h10')
S['keyboard-brightness'] = S['keyboard']+path('M8 3h8M12 1v2')
S['keyboard-brightness.off'] = S['keyboard']+path('M3 2l18 20')
S['mouse'] = rect(6,2,12,20,6)+path('M6 10h12M12 3v5')
S['trackpad'] = rect(3,4,18,16,3)+path('M3 15h18M12 15v5')
S['accessibility'] = circle(12,4,2)+path('M3 8l9 2 9-2M12 10v5m-5 7 5-7 5 7')
S['control-panel'] = path('M3 6h18M3 18h18')+circle(8,6,3)+circle(16,18,3)
S['externaldrive'] = rect(3,5,18,14,3)+path('M3 14h18M16 17h2')
S['flash-drive'] = rect(7,8,10,14)+path('M9 8V2h6v6m-4-3h2')
S['memory-card'] = path('M7 2h11v20H6V7ZM10 3v4m3-4v4m3-4v4')
S['file-sharing'] = S['folder']+path('M8 14h8m-3-3 3 3-3 3')
S['dock.hide'] = path('M3 17h18v4H3ZM8 6l4 4 4-4M12 2v8')
S['play'] = path('m7 3 14 9-14 9Z')
S['pause'] = path('M7 4v16M17 4v16')
S['forward'] = path('m3 5 9 7-9 7Zm10 0 9 7-9 7Z')
S['backward'] = path('m21 5-9 7 9 7Zm-10 0-9 7 9 7Z')

# Artwork has independent 16- and 32-pixel optical editions.
import sys
sys.dont_write_bytecode = True
from artwork import render


def build(renderer):
    if not renderer.is_file():
        raise SystemExit("Build the cupertino-icons render-artwork example before generating the pack")
    icons = {}
    (ROOT / 'symbols').mkdir(exist_ok=True)
    (ROOT / 'icons').mkdir(exist_ok=True)
    for name, body in sorted(S.items()):
        p = f'symbols/{name.replace(".", "-")}.svg'
        source = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><g fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">' + body + '</g></svg>\n'
        (ROOT / p).write_text(source)
        icons['symbol.' + name] = {'symbol': p}
    import re
    contract = (ROOT.parents[2] / 'crates/cupertino-assets/src/name.rs').read_text().split('pub const SYMBOL_NAMES')[0]
    names = re.findall(r'"([a-z][a-z.-]+)"', contract.split('pub const RASTER_NAMES')[1])
    for name in names:
        stem = name.replace('.', '-')
        p = f'icons/{stem}.svg'
        (ROOT / p).write_text(render(name, 0, S))
        rasters = {}
        for size in (16, 32, 64, 128, 256, 512, 1024):
            out = f'icons/{stem}-{size}.png'
            source = render(name, size if size <= 32 else 0, S)
            with tempfile.TemporaryDirectory(prefix='.render-', dir=ROOT) as staging:
                optical = Path(staging) / 'artwork.svg'
                optical.write_text(source)
                subprocess.run([str(renderer), str(optical), str(ROOT / out), str(size)], check=True)
            rasters[str(size)] = out
        icons[name] = {'svg': p, 'raster': rasters}
    (ROOT / 'manifest.json').write_text(json.dumps({'version': 2, 'name': 'Cupertino original icons', 'credits': 'CREDITS.md', 'icons': icons}, indent=2) + '\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--renderer', type=Path, default=ROOT.parents[2] / 'target/debug/examples/render-artwork')
    args = parser.parse_args()
    build(args.renderer.resolve())
