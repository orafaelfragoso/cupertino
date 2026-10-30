"""Writes fallback-test.ttf: a font of plain boxes for a few CJK and emoji code points, for the fallback tests.

Run with `python3 -I make-fallback-font.py` (fontTools); the output is committed next to this script.
"""

from pathlib import Path

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen

UPEM = 1000
CHARS = {"uni65E5": 0x65E5, "uni672C": 0x672C, "uni2764": 0x2764, "u1F600": 0x1F600}


def box(inset):
    pen = TTGlyphPen(None)
    pen.moveTo((inset, 0))
    pen.lineTo((inset, 800))
    pen.lineTo((UPEM - inset, 800))
    pen.lineTo((UPEM - inset, 0))
    pen.closePath()
    return pen.glyph()


def main():
    names = [".notdef", "space", *CHARS]
    builder = FontBuilder(UPEM, isTTF=True)
    builder.setupGlyphOrder(names)
    builder.setupCharacterMap({0x20: "space", **{code: name for name, code in CHARS.items()}})
    glyphs = {name: box(100) for name in names}
    glyphs["space"] = TTGlyphPen(None).glyph()
    builder.setupGlyf(glyphs)
    builder.setupHorizontalMetrics({name: (UPEM, 100) for name in names})
    builder.setupHorizontalHeader(ascent=880, descent=-120)
    builder.setupNameTable({"familyName": "Fallback Test", "styleName": "Regular"})
    builder.setupOS2(sTypoAscender=880, sTypoDescender=-120, usWinAscent=880, usWinDescent=120)
    builder.setupPost()
    # fixed dates, so the file and its hash in ci/gate.py are the same on every run
    builder.font["head"].created = builder.font["head"].modified = 0
    builder.save(Path(__file__).with_name("fallback-test.ttf"))


main()
