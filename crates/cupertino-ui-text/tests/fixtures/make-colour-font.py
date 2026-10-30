"""Writes colour-test.ttf: a COLR v0 font whose heart and grin are a red box layered on a box, for the fallback tests."""

from pathlib import Path

from fontTools.colorLib.builder import buildCOLR, buildCPAL
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen

UPEM = 1000
CHARS = {"uni2764": 0x2764, "u1F600": 0x1F600}


def box(inset):
    pen = TTGlyphPen(None)
    pen.moveTo((inset, 0))
    pen.lineTo((inset, 800))
    pen.lineTo((UPEM - inset, 800))
    pen.lineTo((UPEM - inset, 0))
    pen.closePath()
    return pen.glyph()


def main():
    names = [".notdef", "space", *CHARS, "layer"]
    builder = FontBuilder(UPEM, isTTF=True)
    builder.setupGlyphOrder(names)
    builder.setupCharacterMap({0x20: "space", **{code: name for name, code in CHARS.items()}})
    glyphs = {name: box(100) for name in names}
    glyphs["space"] = TTGlyphPen(None).glyph()
    glyphs["layer"] = box(300)
    builder.setupGlyf(glyphs)
    builder.setupHorizontalMetrics({name: (UPEM, 100) for name in names})
    builder.setupHorizontalHeader(ascent=880, descent=-120)
    builder.setupNameTable({"familyName": "Colour Test", "styleName": "Regular"})
    builder.setupOS2(sTypoAscender=880, sTypoDescender=-120, usWinAscent=880, usWinDescent=120)
    builder.setupPost()
    builder.font["COLR"] = buildCOLR({name: [(name, 0), ("layer", 1)] for name in CHARS})
    builder.font["CPAL"] = buildCPAL([[(0.2, 0.2, 0.2, 1.0), (0.9, 0.1, 0.1, 1.0)]])
    # fixed dates, so the file and its hash in ci/gate.py are the same on every run
    builder.font["head"].created = builder.font["head"].modified = 0
    builder.save(Path(__file__).with_name("colour-test.ttf"))


main()
