"""Writes noto-{arabic,devanagari,hebrew}-subset.ttf: Noto Sans (SIL Open Font License 1.1, OFL.txt beside this script)
cut to the characters of the script tests, with every layout feature kept so shaping joins, reorders and positions
them as the full fonts do.

Run with `python3 -I make-script-fonts.py NOTO_DIR` (fontTools), NOTO_DIR holding NotoSansArabic-Regular.ttf,
NotoSansDevanagari-Regular.ttf and NotoSansHebrew-Regular.ttf; the output is committed next to this script.
"""

import sys
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont

SCRIPTS = {
    "arabic": ("NotoSansArabic-Regular.ttf", "مَرْحبًالع "),
    "devanagari": ("NotoSansDevanagari-Regular.ttf", "किनमस्तेर्म "),
    "hebrew": ("NotoSansHebrew-Regular.ttf", "שלוםעאבג "),
}


def main():
    source = Path(sys.argv[1])
    for name, (file, chars) in SCRIPTS.items():
        options = subset.Options()
        options.layout_features = ["*"]
        options.name_IDs = ["*"]
        options.notdef_outline = True
        font = TTFont(source / file, recalcTimestamp=False)
        subsetter = subset.Subsetter(options)
        subsetter.populate(unicodes=sorted({ord(c) for c in chars}))
        subsetter.subset(font)
        # fixed dates, so the file and its hash in ci/gate.py are the same on every run
        font["head"].created = font["head"].modified = 0
        font.save(Path(__file__).with_name(f"noto-{name}-subset.ttf"))


main()
